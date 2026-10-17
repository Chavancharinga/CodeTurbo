//! Tools the agent can call, in two sets: `Naive` mirrors what today's coding
//! agents offer (whole-file reads, search/replace edits, raw build output);
//! `Ast` adds symbol-level reads/edits and compact compiler diagnostics.

mod ci;
mod devices;
mod fsops;
pub mod memory;
pub mod schedule;
pub mod skills;
mod web;
mod shell;

use coturbo_providers::ToolSpec;
use serde_json::{Value, json};
use coturbo_verify::lsp::LspManager;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Naive,
    Ast,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    /// Images for the model, `(media type, base64 data)`.
    pub images: Vec<(String, String)>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false, images: Vec::new() }
    }
    pub fn err(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true, images: Vec::new() }
    }
}

/// Capability tier of a tool call, least to most privileged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Read,
    Write,
    Exec,
}

/// Tools that change files; `preview` can show their diff before they run.
pub const EDIT_TOOLS: &[&str] = &["write_file", "edit_file", "replace_symbol", "insert_symbol"];

/// Fixed tier per tool. Unknown tools (MCP, third-party) are `Exec`.
pub fn tier(name: &str) -> Tier {
    match name {
        "read_file" | "read_symbol" | "list_files" | "grep" | "workspace_diagnostics" | "propose_plan" | "tasks" | "network_scan" | "tool_search" | "web_search" | "web_fetch" | "agent" | "recall" | "skill" | "lsp" | "ask_user" | "ci" | "jobs" => Tier::Read,
        "remember" | "forget" | "schedule" => Tier::Write,
        "name_device" => Tier::Write,
        n if EDIT_TOOLS.contains(&n) || n == "design_brief" => Tier::Write,
        "design_review" => Tier::Read,
        _ => Tier::Exec,
    }
}

const PREVIEW_MAX_LINES: usize = 200;

fn native_note(rel: &str, diags: &[coturbo_verify::Diag]) -> String {
    use coturbo_verify::diag::{Level, count, render_list};
    if diags.is_empty() {
        return format!("check {rel}: clean");
    }
    let (e, w) = (count(diags, Level::Error), count(diags, Level::Warning));
    format!("check {rel}: {e} error(s), {w} warning(s)\n{}", render_list(diags, 20))
}

type Res = Result<ToolOutput, String>;

pub struct Tools {
    root: PathBuf,
    mode: Mode,
    /// Language-server diagnostics (and formatting) on write; optional.
    lsp: Option<Arc<LspManager>>,
    /// Built-in Rust/JS/TS/CSS/JSON/TOML checks after every edit.
    native_checks: bool,
    /// Content hash of each file when the model last read it, to warn when
    /// a write would clobber a file that changed on disk since (conflict).
    seen: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    /// Where `browse` sends each picture of the page, so the screen can show
    /// the agent using it.
    browse_sink: Option<BrowseSink>,
    /// The browser `browse` keeps open between calls, so the page is still
    /// there for the next steps. It closes after a while with no use.
    /// One kept browser per window the user watches (see [`MAX_BROWSE_WINDOWS`]).
    browse_live: Arc<Vec<Arc<tokio::sync::Mutex<Option<LiveBrowser>>>>>,
    /// `browse` opens a real window the user watches, not one without a window.
    browse_window: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Commands left running in the background (servers), stopped on shutdown.
    background: std::sync::Mutex<Vec<shell::Job>>,
    /// External tool servers (MCP), connected after start.
    mcp: McpSlot,
    /// External tools the model loaded with tool_search; the rest are only
    /// named, so a pile of them costs few tokens.
    loaded: std::sync::Mutex<std::collections::BTreeSet<String>>,
}

/// With more external tools than this, they are named only, and loaded on demand.
const EAGER_EXTERNAL: usize = 8;

/// Where the MCP servers land once connected (they start in the background).
pub type McpSlot = Arc<std::sync::OnceLock<coturbo_mcp::Manager>>;

struct LiveBrowser {
    browser: coturbo_browse::Browser,
    last_used: std::time::Instant,
}

/// How long an unused browser stays open.
/// The `actions` list of the `browse` tool, as the model sees it.
fn browse_actions_schema() -> Value {
    json!({
    "type": "array",
    "description": "steps in order, at most 40",
    "items": {"type": "object", "properties": {
        "do": {"type": "string", "enum": ["open", "snapshot", "click", "fill", "press", "scroll", "wait", "back", "viewport", "text", "shot", "console", "network"], "description": "console: everything the page logged; network: every request with its status"},
        "url": {"type": "string", "description": "open: a project file (index.html), localhost:3000 or a full address"},
        "ref": {"type": "string", "description": "click, fill, text: the @e7 that snapshot gave"},
        "text": {"type": "string", "description": "click, text: the visible text of the element"},
        "selector": {"type": "string", "description": "click, fill, text: a CSS selector"},
        "label": {"type": "string", "description": "fill: the field's label or placeholder"},
        "value": {"type": "string", "description": "fill: what to type"},
        "key": {"type": "string", "description": "press: Enter, Tab, Escape, ArrowDown..."},
        "y": {"type": "integer", "description": "scroll: pixels, negative goes up"},
        "ms": {"type": "integer", "description": "wait: milliseconds, at most 10000"},
        "width": {"type": "integer"}, "height": {"type": "integer"}
    }, "required": ["do"]}
                        })
}

/// The most pages `browse` shows at once, each in its own small window.
pub const MAX_BROWSE_WINDOWS: usize = 6;
const BROWSER_IDLE: std::time::Duration = std::time::Duration::from_secs(120);

/// Receives the pictures of a `browse` session as they are taken.
pub type BrowseSink = Arc<dyn Fn(coturbo_browse::Frame) + Send + Sync>;

/// FNV-1a hash of file content, for cheap change detection.
pub(crate) fn content_hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Rust files longer than this are returned as a skeleton by `read_file` in Ast mode.
const SKELETON_MIN_LINES: usize = 60;

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

fn spec(name: &str, description: &str, schema: Value) -> ToolSpec {
    ToolSpec { name: name.into(), description: description.into(), input_schema: schema }
}

fn s(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}

fn arg<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key].as_str().ok_or_else(|| format!("missing string argument `{key}`"))
}

impl Tools {
    pub fn new(root: &Path, mode: Mode) -> Self {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        Self { root, mode, lsp: None, native_checks: false, seen: std::sync::Mutex::new(std::collections::HashMap::new()), browse_sink: None, browse_live: Arc::new((0..MAX_BROWSE_WINDOWS).map(|_| Arc::default()).collect()), browse_window: Arc::new(|| false), background: std::sync::Mutex::new(Vec::new()), mcp: Arc::default(), loaded: std::sync::Mutex::default() }
    }

    /// Makes `browse` open a real window beside the terminal, with the agent's
    /// pointer drawn on the page, so the user sees the actual page.
    pub fn with_browse_window(mut self, on: bool) -> Self {
        self.browse_window = Arc::new(move || on);
        self
    }

    /// Like [`Self::with_browse_window`], decided when the browser opens: the
    /// screen may only find out later what the terminal can draw.
    pub fn with_browse_window_when(mut self, decide: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.browse_window = Arc::new(decide);
        self
    }

    /// Shows the pages `browse` visits: every picture goes to `sink`.
    pub fn with_browse_sink(mut self, sink: BrowseSink) -> Self {
        self.browse_sink = Some(sink);
        self
    }

    /// Enables the built-in checks after every edit (no external tools).
    pub fn with_native_checks(mut self, on: bool) -> Self {
        self.native_checks = on;
        self
    }

    /// Enables diagnostics and formatting after every edit.
    pub fn with_lsp(mut self, lsp: Arc<LspManager>) -> Self {
        self.lsp = Some(lsp);
        self
    }

    /// Whether files written by `write_file`/`edit_file` are formatted
    /// (rustfmt for Rust, the built-in formatter for HTML).
    pub(crate) fn format_on_write(&self) -> bool {
        self.lsp.as_ref().is_some_and(|l| l.format_on_write())
    }

    /// Where the MCP servers go once connected; tasks after that see their tools.
    pub fn mcp_slot(&self) -> McpSlot {
        self.mcp.clone()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Tool definitions, sorted by name so the request prefix is byte-stable.
    pub fn specs(&self) -> Vec<ToolSpec> {
        let read_desc = match self.mode {
            Mode::Naive => "Read a file. Images (png, jpg, gif, webp) are shown to you; PDFs and Jupyter notebooks come as text.",
            Mode::Ast => "Read a file (images are shown to you; PDFs and notebooks come as text). Large .rs, .js and .ts files return a skeleton (bodies elided as `{ … }`); use read_symbol (Rust) for a body or full=true for everything.",
        };
        let mut v = vec![
            spec("read_file", read_desc, obj(json!({"path": s("relative path"), "full": {"type": "boolean"}}), &["path"])),
            spec("write_file", "Create or overwrite a file.", obj(json!({"path": s("relative path"), "content": s("full file content")}), &["path", "content"])),
            spec("edit_file", "Replace the unique occurrence of `old` with `new` in a file.", obj(json!({"path": s("relative path"), "old": s("exact text, must match once"), "new": s("replacement")}), &["path", "old", "new"])),
            spec("list_files", "List files (respects .gitignore).", obj(json!({"path": s("relative dir, default .")}), &[])),
            spec("grep", "Regex search; returns path:line: text.", obj(json!({"pattern": s("regex"), "path": s("relative dir or file, default .")}), &["pattern"])),
            spec(
                "bash",
                "Run a shell command in the project root and get its output. A command that keeps running (a web server, a watcher, `npm run dev`, `python -m http.server`) must use background=true: it keeps running, you get its first output after a few seconds, and it stops when the session ends. Never start a server without it: the call would wait until it times out.",
                obj(json!({"command": s("command"), "background": {"type": "boolean", "description": "leave it running (servers, watchers)"}}), &["command"]),
            ),
            spec(
                "design_brief",
                "Call before writing any user interface (HTML, CSS, JSX, Vue, Svelte). Describe the design in a few words; returns the locked design tokens (written to DESIGN.md) that every style must use.",
                obj(
                    json!({
                        "subject": s("what the product or page is about"),
                        "audience": s("who it is for"),
                        "tone": s("tone in a few words, e.g. editorial and calm, playful, technical"),
                        "signature": s("the one distinctive visual element of this interface"),
                        "brand_colors": {"type": "array", "items": {"type": "string"}, "description": "the brand's existing colors as #rrggbb, most important first; omit to detect them from the project's files"},
                        "new_palette": {"type": "boolean", "description": "true only if the user asked to replace the brand colors"}
                    }),
                    &["subject", "audience", "tone", "signature"],
                ),
            ),
            spec(
                "design_review",
                "Render an HTML page in the browser and get its screenshot plus measurements, to critique the design as a user would see it. Use after building a page.",
                obj(json!({"path": s("relative .html path")}), &["path"]),
            ),
            spec(
                "investigate",
                "Reverse-engineer how existing software does something, from evidence (read-only): a website URL (rendered, with structure, technology, design system and screenshots), a local app folder, an Electron .asar, a JS bundle, or a native binary (exe/dll/so/dylib/wasm: imports, exports, strings). Use it to learn from a reference before building something similar. When recreating an app with an interface, reproduce the real layout faithfully — its navigation, lists, master-detail panels, primary actions and states — grounded in what the report shows, not a simplified demo. For an app whose UI lives in resources (e.g. Electron/CEF), also investigate that UI folder, not only the launcher binary. Symbols and strings do not show the look: pair it with observe_app to see the running program.",
                obj(json!({"target": s("URL, or path to a folder or file (absolute paths allowed)"), "question": s("what you want to understand, e.g. how search works")}), &["target"]),
            ),
            spec(
                "observe_app",
                "Look at a program running: opens an executable, captures its window as a screenshot (you see the image) and closes it. Use it on the REFERENCE app before recreating it, to see its real layout, density, controls and states instead of guessing from symbol names; and on the app YOU built, to compare it side by side with the reference and fix what differs. Windows only. Maximizes the window by default so the whole interface shows.",
                obj(
                    json!({
                        "target": s("path to the .exe (absolute, or relative to the project) or a command such as mspaint.exe"),
                        "title": s("optional text the window title must contain"),
                        "seconds": json!({"type": "integer", "description": "how long to wait for the window to appear (default 8)"}),
                        "maximize": json!({"type": "boolean", "description": "maximize the window (default true)"})
                    }),
                    &["target"],
                ),
            ),
            spec(
                "browse",
                "Use a page like a person would, in a real browser the user watches in a small window: open it, then `snapshot` to list what can be clicked (@e1, @e2...), click with a real mouse, fill fields, press keys, scroll, go back, resize to a phone, read text, take a photo. Actions run in order and stop at the first one that fails. It tells you what a person would notice: a click that lands on something else, console errors, images and files that do not load. After building a page or app, go through EVERY menu, link, button and form with it, on desktop and at phone size, and fix what fails. To check several pages at the same time (the pages of a site, or desktop next to phone), pass `windows`: each entry has its own `window` number and `actions`, and each page appears in its own small square in the user's view. Choose how many you need, from 1 to 6; the pages share the same space, so use more only when they help.",
                obj(
                    json!({
                        "actions": browse_actions_schema(),
                        "window": json!({"type": "integer", "description": "with `actions`: which small window shows this page, 0 to 5 (default 0)"}),
                        "windows": json!({"type": "array", "description": "several pages at once, at most 6: each has its own window number and actions", "items": {"type": "object", "properties": {"window": {"type": "integer"}, "actions": browse_actions_schema()}, "required": ["actions"]}})
                    }),
                    &[],
                ),
            ),
            spec(
                "tasks",
                "The user's live checklist for this job. At the start of any job with three or more steps, call it once with the FULL list (the first step `doing`, the rest `pending`); call it again whenever a step starts or finishes, marking each `pending`, `doing` or `done`. Keep titles short, in the user's language. It touches no files and costs almost nothing; put it in the same turn as your other tool calls.",
                obj(
                    json!({"items": json!({
                        "type": "array",
                        "description": "every step, in order",
                        "items": {"type": "object", "properties": {
                            "title": {"type": "string", "description": "what the step does, a few words"},
                            "status": {"type": "string", "enum": ["pending", "doing", "done"]}
                        }, "required": ["title", "status"]}
                    })}),
                    &["items"],
                ),
            ),
            spec(
                "remember",
                "Save something worth knowing in later sessions of this project: who the user is (user), how they want you to work, including corrections and what they approved (feedback), goals, deadlines and decisions not visible in the code (project), or where things live outside the code (reference). One fact per memory; reuse a name to update it. Do not save what the code or git already record, or what only matters to this conversation.",
                obj(
                    json!({
                        "name": s("short kebab-case name"),
                        "type": {"type": "string", "enum": ["user", "feedback", "project", "reference"]},
                        "description": s("one line, used to decide when it is relevant"),
                        "content": s("the fact; for feedback and project add **Why:** and **How to apply:**")
                    }),
                    &["name", "type", "description", "content"],
                ),
            ),
            spec(
                "jobs",
                "The commands running in the background (started by bash with background=true): list them, read what one printed since you last looked (output), or stop one (stop). Use it to watch a server's log or a long build.",
                obj(json!({"action": {"type": "string", "enum": ["list", "output", "stop"]}, "id": {"type": "integer", "description": "output/stop: the job id from list"}}), &[]),
            ),
            spec(
                "ci",
                "GitHub Actions of the current branch through the gh CLI: status (the latest runs), logs (the failed steps of a run, the newest failure by default), wait (until the newest run ends, then its result and failed steps). After pushing, wait for CI and fix what fails.",
                obj(json!({"action": {"type": "string", "enum": ["status", "logs", "wait"]}, "branch": s("default: the current branch"), "run": {"type": "integer", "description": "logs: a run id"}}), &[]),
            ),
            spec(
                "ask_user",
                "Ask the user a question only they can answer (a preference, a choice between real alternatives, a missing fact), with 2 to 4 short options to pick; they can also type their own answer. Do not ask what you can find out or decide yourself.",
                obj(json!({"question": s("the question"), "options": {"type": "array", "items": {"type": "string"}, "description": "2 to 4 short options"}}), &["question"]),
            ),
            spec(
                "lsp",
                "Navigate code through the project's language server: where a symbol is defined (definition), every place it is used (references), or its type and docs (hover). Give the file, the line where the symbol appears and the symbol's text. Faster and more exact than grep for code.",
                obj(
                    json!({
                        "action": {"type": "string", "enum": ["definition", "references", "hover"]},
                        "path": s("relative file path"),
                        "line": {"type": "integer", "description": "1-based line where the symbol appears"},
                        "symbol": s("the symbol's text on that line")
                    }),
                    &["action", "path", "line", "symbol"],
                ),
            ),
            spec(
                "skill",
                "Load an installed skill: its instructions for a particular kind of work, and the list of its files (read one with `file`). Load it before starting a task that matches its description.",
                obj(json!({"name": s("the skill's name"), "file": s("optional: one of the skill's files")}), &["name"]),
            ),
            spec(
                "schedule",
                "Run a task later or on a repeat, by itself, while CodeTurbo is open in this project: \"every day at 23:00 turn off the living room TV\", \"in 20m run the tests\". action add (when + task), list, or cancel (id). The task is written as the user would ask it.",
                obj(
                    json!({
                        "action": {"type": "string", "enum": ["add", "list", "cancel"]},
                        "when": s("in 20m, in 2h, every 30m, every 2h, at 23:00, every day at 23:00"),
                        "task": s("what to do then, as a request"),
                        "id": {"type": "integer", "description": "cancel: the id from list"}
                    }),
                    &[],
                ),
            ),
            spec("recall", "Read one remembered fact in full (by name), or the list of all of them.", obj(json!({"name": s("memory name; omit for the list")}), &[])),
            spec("forget", "Delete a memory that turned out wrong or no longer applies.", obj(json!({"name": s("memory name")}), &["name"])),
            spec(
                "agent",
                "Hand a self-contained piece of work to a subagent with its own fresh context; you get back only its report, which keeps your context small. Call it several times in the same reply to run them at the same time. kind: explore (find how something works in the code, the default), plan (a step-by-step plan), review (look for real defects) only read; general does the work in its own git worktree (a copy of the project) and hands back a branch to merge. The prompt must stand alone: the subagent knows nothing of this conversation.",
                obj(
                    json!({
                        "description": s("a few words, shown to the user"),
                        "prompt": s("the whole task, with every detail the subagent needs"),
                        "kind": {"type": "string", "enum": ["explore", "plan", "review", "general"]}
                    }),
                    &["description", "prompt"],
                ),
            ),
            spec(
                "web_search",
                "Search the web; returns titles, addresses and snippets. Use it for current facts, documentation, error messages and libraries, then read the best result with web_fetch.",
                obj(json!({"query": s("what to search for")}), &["query"]),
            ),
            spec(
                "web_fetch",
                "Read a web page as plain text (headings, paragraphs, lists, links), without opening a browser. For documentation, articles, API references and results from web_search. Use browse instead to interact with a page or see how it looks.",
                obj(json!({"url": s("full http(s) address")}), &["url"]),
            ),
            spec(
                "network_scan",
                "List the devices on the user's local network (read-only): TVs, speakers, printers, cameras, lights, plugs, routers, computers. Finds them by Bonjour/mDNS, UPnP and the ARP table; sweep=true also checks common ports on every address of the local subnet (slower, finds silent devices). Returns each device's address, MAC, names, services, UPnP actions with their control URLs, and open ports. Use it before device_control.",
                obj(json!({"sweep": {"type": "boolean", "description": "also probe common ports on the whole local /24 (about 10 s)"}, "seconds": {"type": "integer", "description": "how long to listen for announcements, 1-10 (default 3)"}}), &[]),
            ),
            spec(
                "name_device",
                "Give a device on the network a name the user can use later (\"living room TV\"), so `tv` and the user can refer to it by name, even when it is off. `device` is its current address or MAC from network_scan.",
                obj(json!({"device": s("address or MAC"), "name": s("the name, in the user's words")}), &["device", "name"]),
            ),
            spec(
                "device",
                "Control any device on the local network by its name or address; its protocol is found from what it answers. TVs (Samsung, LG, Android TV / Google TV, Roku, Fire TV): on, off, volume, mute, play/pause, launch (YouTube, Netflix...), key, apps, status (model, version). Speakers and players (Google Cast, Sonos and UPnP/DLNA): play, pause, stop, next, previous, volume, mute, status (what is playing). Lights, plugs and switches (Philips Hue, Shelly, Tasmota, WLED, TP-Link Kasa, Yeelight): on, off, toggle, brightness, color, status (with power use when measured); `target` picks one light, relay or channel. Printers: status (state, ink and toner levels). Routers: status (internet, public address). MQTT brokers (Zigbee2MQTT and others): publish (topic, payload) and listen (topic). The first time, some devices ask to accept on their screen (TVs) or a button press (Hue bridge): tell the user. For anything else, device_control reaches the device's own API.",
                obj(
                    json!({
                        "device": s("its name (e.g. living room TV), address or MAC"),
                        "action": {"type": "string", "enum": ["status", "on", "off", "toggle", "brightness", "color", "volume", "volume_up", "volume_down", "mute", "play", "pause", "stop", "next", "previous", "launch", "key", "apps", "publish", "listen"]},
                        "value": {"type": "integer", "description": "brightness or volume 0-100, or volume steps"},
                        "color": s("color: #rrggbb or a name (red, blue, warm...)"),
                        "target": s("one light (by name), relay or channel inside the device"),
                        "app": s("launch: YouTube, Netflix, Spotify... or an app id"),
                        "content": s("launch: what to play, e.g. a YouTube video id or search"),
                        "key": s("key: home, back, up, down, left, right, ok, ..."),
                        "topic": s("MQTT topic, e.g. zigbee2mqtt/living room lamp/set"),
                        "payload": s("MQTT payload, e.g. {\"state\": \"ON\"}"),
                        "user": s("MQTT broker user, when it needs a login"),
                        "password": s("MQTT broker password")
                    }),
                    &["device", "action"],
                ),
            ),
            spec(
                "device_control",
                "Act on a device of the user's LOCAL network (private addresses only; the user approves each call). kind=http: a request to the device's own API (e.g. Shelly http://ip/relay/0?turn=on, Tasmota http://ip/cm?cmnd=Power%20TOGGLE, Hue, ESPHome, a router page); kind=upnp: a UPnP action on a control URL from network_scan (e.g. AVTransport Play/Pause/Stop with args {\"InstanceID\": \"0\", \"Speed\": \"1\"}, RenderingControl SetVolume with {\"InstanceID\": \"0\", \"Channel\": \"Master\", \"DesiredVolume\": \"20\"}); kind=wake: Wake-on-LAN to a MAC. Read first (GET, status) before changing anything, and say what you will do.",
                obj(
                    json!({
                        "kind": {"type": "string", "enum": ["http", "upnp", "wake"]},
                        "method": s("http: GET, POST, PUT... (default GET)"),
                        "url": s("http: full URL on the local network"),
                        "headers": {"type": "object", "description": "http: extra headers"},
                        "body": s("http: request body"),
                        "control_url": s("upnp: the control URL from network_scan"),
                        "service_type": s("upnp: e.g. urn:schemas-upnp-org:service:AVTransport:1"),
                        "action": s("upnp: action name, e.g. Play"),
                        "args": {"type": "object", "description": "upnp: action arguments"},
                        "mac": s("wake: the MAC address"),
                        "broadcast": s("wake: broadcast address, default 255.255.255.255")
                    }),
                    &["kind"],
                ),
            ),
            spec("propose_plan", "Plan mode only: submit .codeturbo/plans/<slug>-plan.md for the user's approval.", obj(json!({"slug": s("plan slug")}), &["slug"])),
        ];
        if self.mode == Mode::Ast {
            let sym = s("symbol path, e.g. `Point::new` or `utils::parse`");
            v.extend([
                spec("read_symbol", "Read one item (fn, struct, impl method, ...) of a .rs file.", obj(json!({"path": s("relative .rs path"), "symbol": sym.clone()}), &["path", "symbol"])),
                spec("replace_symbol", "Replace one item of a .rs file with new code (doc comments included); file is rustfmt'd.", obj(json!({"path": s("relative .rs path"), "symbol": sym.clone(), "code": s("complete new item")}), &["path", "symbol", "code"])),
                spec("insert_symbol", "Insert a new item right after an existing one in a .rs file; file is rustfmt'd.", obj(json!({"path": s("relative .rs path"), "after": sym, "code": s("complete new item")}), &["path", "after", "code"])),
                spec("workspace_diagnostics", "Type-check the whole project (every language found: cargo check, tsc, go build, pyright); returns errors and warnings as path:line:col with the source line.", obj(json!({}), &[])),
            ]);
        }
        v.sort_by(|a, b| a.name.cmp(&b.name));
        // External servers' tools come after, in a stable order. Many of them
        // are only named, behind tool_search, until the model asks for one.
        if let Some(m) = self.mcp.get() {
            let all = m.tools();
            let loaded = self.loaded.lock().map(|l| l.clone()).unwrap_or_default();
            let eager = all.len() <= EAGER_EXTERNAL;
            let mut ext: Vec<ToolSpec> = all
                .iter()
                .filter(|t| eager || loaded.contains(&t.qualified()))
                .map(|t| spec(&t.qualified(), &format!("[{} server] {}", t.server, t.description), t.input_schema.clone()))
                .collect();
            ext.sort_by(|a, b| a.name.cmp(&b.name));
            v.extend(ext);
            let mut waiting: Vec<String> = all.iter().map(|t| t.qualified()).filter(|q| !eager && !loaded.contains(q)).collect();
            waiting.sort();
            if !waiting.is_empty() {
                v.push(spec(
                    "tool_search",
                    &format!(
                        "Load external tools before calling them: only their names are listed here, to save tokens. Pass exact names (select) or words to search their names and descriptions; their full definitions are available from the next step on. Not loaded yet: {}",
                        waiting.join(", ")
                    ),
                    obj(json!({"query": s("words to search, or select:name1,name2 for exact names")}), &["query"]),
                ));
            }
        }
        v
    }

    /// `lsp`: definition, references or hover through the language server.
    async fn lsp_navigate(&self, input: &Value) -> Res {
        let lsp = self.lsp.as_ref().ok_or("no language server is configured; use grep and read_file")?;
        let rel = arg(input, "path")?.replace('\\', "/");
        let line = input["line"].as_u64().ok_or("missing `line`")? as u32;
        lsp.navigate(arg(input, "action")?, &rel, line, arg(input, "symbol")?).await.map(ToolOutput::ok)
    }

    /// `tool_search`: marks the matching external tools as loaded.
    fn tool_search(&self, input: &Value) -> Res {
        let query = arg(input, "query")?.trim().to_lowercase();
        let m = self.mcp.get().ok_or("no external tool servers are connected")?;
        let found: Vec<&coturbo_mcp::Tool> = if let Some(names) = query.strip_prefix("select:") {
            let want: Vec<&str> = names.split(',').map(str::trim).collect();
            m.tools().iter().filter(|t| want.contains(&t.qualified().to_lowercase().as_str())).collect()
        } else {
            let words: Vec<&str> = query.split_whitespace().collect();
            let mut scored: Vec<(usize, &coturbo_mcp::Tool)> = m
                .tools()
                .iter()
                .map(|t| {
                    let hay = format!("{} {}", t.qualified(), t.description).to_lowercase();
                    (words.iter().filter(|w| hay.contains(*w)).count(), t)
                })
                .filter(|(n, _)| *n > 0)
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            scored.into_iter().take(8).map(|(_, t)| t).collect()
        };
        if found.is_empty() {
            return Err(format!("no external tool matches `{query}`"));
        }
        if let Ok(mut l) = self.loaded.lock() {
            l.extend(found.iter().map(|t| t.qualified()));
        }
        let list: Vec<String> = found.iter().map(|t| format!("{}: {}", t.qualified(), t.description.chars().take(160).collect::<String>())).collect();
        Ok(ToolOutput::ok(format!("Loaded; call them from the next step:\n{}", list.join("\n"))))
    }

    /// Runs `actions` in window `slot`'s browser, kept open for `browse`,
    /// opening one if there is none (or the old one stopped answering).
    async fn run_live(&self, slot: usize, path: &Path, actions: &[coturbo_browse::Action], mut on_frame: impl FnMut(coturbo_browse::Frame) + Send) -> Result<coturbo_browse::Report, String> {
        let cell = self.browse_live[slot].clone();
        let mut guard = cell.lock().await;
        for attempt in 0..2 {
            if guard.is_none() {
                // A real window beside the terminal is only for the first page.
                let visible = slot == 0 && (self.browse_window)();
                let browser = coturbo_browse::Browser::open_with(path, (880, 560), visible).await?;
                *guard = Some(LiveBrowser { browser, last_used: std::time::Instant::now() });
                self.watch_browser(slot);
            }
            let live = guard.as_mut().expect("opened above");
            match live.browser.run(&self.root, actions, &mut on_frame).await {
                Ok(report) => {
                    live.last_used = std::time::Instant::now();
                    return Ok(report);
                }
                Err(e) => {
                    // A browser that stopped answering is put away; one fresh try.
                    if let Some(l) = guard.take() {
                        l.browser.close().await;
                    }
                    if attempt == 1 {
                        return Err(e);
                    }
                }
            }
        }
        Err("the browser did not open".into())
    }

    /// Closes window `slot`'s browser once it has gone unused for a while.
    fn watch_browser(&self, slot: usize) {
        let live = self.browse_live[slot].clone();
        // A window someone is watching stays longer than one nobody sees.
        let idle = if (self.browse_window)() { BROWSER_IDLE * 3 } else { BROWSER_IDLE };
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                let mut guard = live.lock().await;
                match guard.as_ref() {
                    None => break,
                    Some(l) if l.last_used.elapsed() >= idle => {
                        if let Some(l) = guard.take() {
                            drop(guard);
                            l.browser.close().await;
                        }
                        break;
                    }
                    Some(_) => {}
                }
            }
        });
    }

    /// Closes the browsers `browse` keeps open (when the session ends).
    pub async fn shutdown(&self) {
        let running: Vec<shell::Job> = self.background.lock().map(|mut b| std::mem::take(&mut *b)).unwrap_or_default();
        for mut job in running {
            let _ = job.child.kill().await;
        }
        for cell in self.browse_live.iter() {
            let live = cell.lock().await.take();
            if let Some(l) = live {
                l.browser.close().await;
            }
        }
    }

    /// Drives real browsers through `actions`, showing every step: one page,
    /// or up to six at once, each in its own small window.
    async fn browse(&self, input: &Value) -> Res {
        use coturbo_browse::{Action, resolve_url};
        // `windows: [{window, actions}]` opens several pages together; plain
        // `actions` (and an optional `window`) drives one.
        let jobs: Vec<(usize, Vec<Action>)> = match input["windows"].as_array() {
            Some(list) if !list.is_empty() => {
                if list.len() > MAX_BROWSE_WINDOWS {
                    return Err(format!("at most {MAX_BROWSE_WINDOWS} windows at once"));
                }
                let mut jobs = Vec::new();
                for (i, w) in list.iter().enumerate() {
                    let slot = w["window"].as_u64().map_or(i, |n| n as usize);
                    jobs.push((slot, Action::parse_all(&w["actions"]).map_err(|e| format!("window {}: {e}", i + 1))?));
                }
                jobs
            }
            _ => vec![(input["window"].as_u64().unwrap_or(0) as usize, Action::parse_all(&input["actions"])?)],
        };
        let mut seen = std::collections::HashSet::new();
        for (slot, actions) in &jobs {
            if *slot >= MAX_BROWSE_WINDOWS || !seen.insert(*slot) {
                return Err(format!("window numbers are 0 to {} and each is used once per call", MAX_BROWSE_WINDOWS - 1));
            }
            // A project file must stay inside the project, like every other tool.
            for a in actions {
                if let Action::Open(u) = a {
                    let url = resolve_url(&self.root, u);
                    if let Some(p) = url.strip_prefix("file:///").or_else(|| url.strip_prefix("file://")) {
                        // `file:///C:/x` is `C:/x`; `file:///tmp/x` is `/tmp/x`.
                        let p = p.replace("%20", " ");
                        let path = std::path::PathBuf::from(if cfg!(windows) { p } else { format!("/{p}") });
                        let root = self.root.canonicalize().unwrap_or_else(|_| self.root.clone());
                        let inside = path.canonicalize().is_ok_and(|c| c.starts_with(&root));
                        if !inside {
                            return Err(format!("{u}: the page must be inside the project"));
                        }
                    }
                }
            }
        }
        let browser = coturbo_design::render::find_browser().ok_or_else(coturbo_design::render::no_browser_help)?;
        // Every window works at the same time, each reporting its own steps.
        let runs = jobs.iter().map(|(slot, actions)| {
            let slot = *slot;
            let sink = self.browse_sink.clone();
            let browser = &browser;
            async move {
                let report = self
                    .run_live(slot, browser, actions, move |mut f| {
                        f.window = slot;
                        if let Some(s) = &sink {
                            s(f);
                        }
                    })
                    .await;
                (slot, actions.len(), report)
            }
        });
        let results = futures_util::future::join_all(runs).await;
        let many = results.len() > 1;
        let mut text = String::new();
        let mut images = Vec::new();
        for (slot, steps, report) in results {
            let report = report?;
            if many {
                text.push_str(&format!("window {slot}: "));
            }
            text.push_str(&format!("browser session, {steps} step(s):\n{}", report.log.join("\n")));
            if report.problems.is_empty() {
                text.push_str("\n\nNothing wrong was noticed.\n\n");
            } else {
                text.push_str("\n\nWhat a person would notice is wrong (fix these):\n");
                text.push_str(&report.problems.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"));
                text.push_str("\n\n");
            }
            if let Some(png) = report.shots.last() {
                use base64::Engine;
                images.push(("image/png".to_string(), base64::engine::general_purpose::STANDARD.encode(png)));
            }
        }
        let mut out = ToolOutput::ok(text.trim_end().to_string());
        out.images = images;
        Ok(out)
    }

    /// The checklist is shown by the interface; here it is only checked and
    /// acknowledged, so the model learns when it sent something malformed.
    fn tasks(input: &Value) -> Res {
        let items = input["items"].as_array().filter(|a| !a.is_empty()).ok_or("items must be a non-empty list of {title, status}")?;
        if items.len() > 12 {
            return Err("at most 12 steps: group the small ones".into());
        }
        let mut done = 0;
        for (i, it) in items.iter().enumerate() {
            let status = it["status"].as_str().unwrap_or("");
            if it["title"].as_str().is_none_or(|t| t.trim().is_empty()) || !["pending", "doing", "done"].contains(&status) {
                return Err(format!("step {} needs a title and a status of pending, doing or done", i + 1));
            }
            done += usize::from(status == "done");
        }
        Ok(ToolOutput::ok(format!("checklist updated: {done}/{} done", items.len())))
    }

    /// Opens a program, captures its window and returns the screenshot.
    async fn observe_app(&self, input: &Value) -> Res {
        let raw = arg(input, "target")?;
        // A path that exists under the project is made absolute; a bare
        // command (`mspaint.exe`) is left for the system to resolve.
        let local = self.root.join(raw);
        let target = if !Path::new(raw).is_absolute() && local.is_file() { local.to_string_lossy().into_owned() } else { raw.to_string() };
        let seconds = input["seconds"].as_u64().unwrap_or(8);
        let maximize = input["maximize"].as_bool().unwrap_or(true);
        let dir = self.root.join(".codeturbo/observe");
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let shot = dir.join(format!("{}-{stamp}.png", coturbo_rea::observe::process_name(&target).replace(' ', "_")));
        let title = input["title"].as_str().filter(|t| !t.is_empty());
        let info = coturbo_rea::observe::observe(&target, title, seconds, maximize, &shot).await?.info;
        let mut out = ToolOutput::ok(format!(
            "Captured the window of {target}: {info}. Screenshot saved at {}. Look at it as a demanding designer: \
             the exact arrangement of regions, the density of controls, how groups are labelled, the colours, \
             the type and the states of the controls. Describe what you see before building or fixing.",
            self.display(&shot)
        ));
        if let Ok(png) = std::fs::read(&shot) {
            use base64::Engine;
            out.images.push(("image/png".into(), base64::engine::general_purpose::STANDARD.encode(png)));
        }
        Ok(out)
    }

    /// Compiles the brief into tokens and locks them in `DESIGN.md`.
    fn design_brief(&self, input: &Value) -> Res {
        use coturbo_design::{compiler, contract};
        let given: Vec<String> =
            input["brand_colors"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let fresh = input["new_palette"].as_bool() == Some(true);
        // A redesign starts from the design system the project already has.
        let existing = if fresh { Default::default() } else { coturbo_design::extract::scan(&self.root) };
        let brand = if fresh { Vec::new() } else if !given.is_empty() { given } else { existing.brand() };
        let brief = compiler::Brief {
            subject: arg(input, "subject")?.to_string(),
            audience: arg(input, "audience")?.to_string(),
            tone: arg(input, "tone")?.to_string(),
            signature: arg(input, "signature")?.to_string(),
            brand,
            fonts: existing.fonts.iter().take(2).cloned().collect(),
            radii: existing.radii.clone(),
        };
        let tokens = compiler::compile(&brief);
        let path = self.root.join(contract::FILE);
        std::fs::write(&path, contract::markdown(&tokens)).map_err(|e| format!("{}: {e}", path.display()))?;
        // Write a ready stylesheet so every page just links it and starts
        // with margins, focus, responsiveness and motion already right.
        let kit_css = format!(
            "/* Generated by CodeTurbo: tokens + base kit. Include it in every page:\n   <link rel=\"stylesheet\" href=\"codeturbo-kit.css\"> */\n{}\n\n{}\n",
            contract::css(&tokens),
            coturbo_design::kit::base_css(&tokens)
        );
        std::fs::write(self.root.join("codeturbo-kit.css"), kit_css).map_err(|e| format!("codeturbo-kit.css: {e}"))?;
        // A visual catalogue the user can open in a browser.
        let _ = std::fs::write(self.root.join("design-preview.html"), contract::preview_html(&tokens));
        let kept = if existing.is_empty() && brief.brand.is_empty() {
            String::new()
        } else if !existing.is_empty() {
            format!(
                " This project already has a design system, kept and organized (do not replace it):\n{}\n",
                coturbo_design::extract::summary(&existing)
            )
        } else {
            format!(
                " Brand colors kept and organized: {} (primary = main brand color for fills, primary-ink = the same hue safe for text, \
                 gray-* = neutral scale for hierarchy). Do not drift from the brand.",
                brief.brand.join(", ")
            )
        };
        Ok(ToolOutput::ok(format!(
            "wrote {} and codeturbo-kit.css (tone: {}).{kept}\n\
             Design boldly and make this interface unmistakably its own — the kit is a starting point to build ON, not a mold to fit into. \
             Link it in every page's <head>: {}\n\
             and <link rel=\"stylesheet\" href=\"codeturbo-kit.css\">. It handles side margins, focus, fluid type and motion so you don't have to. \
             Build on the tokens (var(--…)). The design check BLOCKS only real defects (unreadable contrast, overlap, content cut off, \
             not responsive, no side margin, broken images) and the tell-tale generic-AI look (default fonts like Inter/Roboto, raw Tailwind \
             colors, emoji used as icons). Everything else — layout, shapes, density, repetition — is your creative call; a toolbar of \
             identical buttons or a dense app panel is fine. Avoid the stock marketing-page shape unless it truly fits. \
             Finish it: every icon an inline SVG drawn for this project (one stroke and grid, currentColor), motion where it helps \
             (hover and press, transitions, loading, enter and leave, with prefers-reduced-motion), and every function working, \
             with its empty, loading and error states.\n\
             Layout idea: {}.\nSignature element to build around: {}.\n\
             A composition you could follow or diverge from (make it yours):\n{}\n\
             Classes: .container (centered, with side padding), .section, .grid (responsive), .cluster, .btn/.btn--primary/.btn--secondary, .panel, .card (hover lift), .tag.\n\
             Animate sections: add class `reveal` and this script once per page: \
             <script>addEventListener('DOMContentLoaded',()=>{{const o=new IntersectionObserver(es=>es.forEach(e=>e.isIntersecting&&e.target.classList.add('in')),{{threshold:0.12}});document.querySelectorAll('.reveal').forEach(el=>o.observe(el))}})</script> \
             Also `float`, `link-underline`, `data-animate=\"rise\"`.\n\
             For tables on small screens, wrap them so they stack or fit — never let content run off the edge.\n\
             Project ornament (inline SVG, colored by `color`, keep it behind content with the .ornament class):\n{}\nSection divider:\n{}\n\
             No stock imagery: draw product screens and illustrations in HTML/CSS or SVG.",
            contract::FILE,
            tokens.tone.name(),
            contract::font_link(&tokens).replace('\n', " "),
            tokens.layout,
            brief.signature,
            coturbo_design::kit::blueprint(&tokens).iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect::<Vec<_>>().join("\n"),
            coturbo_design::kit::ornament_svg(&tokens),
            coturbo_design::kit::divider_svg(&tokens)
        )))
    }

    /// Renders a page: what the rules and measurements say, and the screenshot.
    async fn design_review(&self, input: &Value) -> Res {
        use coturbo_design::{contract::Contract, fiscal, render};
        let rel = arg(input, "path")?;
        let abs = self.resolve(rel)?;
        let text = std::fs::read_to_string(&abs).map_err(|e| format!("{rel}: {e}"))?;
        let rel = self.display(&abs);
        let mut findings = fiscal::check(&rel, &text, Contract::load(&self.root).as_ref());
        let browser = render::find_browser().ok_or_else(coturbo_design::render::no_browser_help)?;
        let stem = abs.file_stem().map_or("page".into(), |s| s.to_string_lossy().into_owned());
        let shot = self.root.join(".codeturbo/design").join(format!("{stem}.png"));
        let page = render::measure(&browser, &abs, &shot).await?;
        findings.extend(render::judge(&page));
        let phone = render::mobile_shot(&shot);
        let mut out = ToolOutput::ok(format!(
            "{}\nScreenshots attached: desktop 1440px ({}) and phone 390px ({}). Look at them as a demanding designer: \
             anything overlapping, cramped, cut off, misaligned or scrolling sideways? Does it look generic? \
             Is the signature element the first thing you see? Fix every problem, keeping DESIGN.md, then call design_review again.",
            fiscal::render(&rel, &findings),
            self.display(&shot),
            self.display(&phone)
        ));
        for p in [&shot, &phone] {
            if let Ok(png) = std::fs::read(p) {
                use base64::Engine;
                out.images.push(("image/png".into(), base64::engine::general_purpose::STANDARD.encode(png)));
            }
        }
        Ok(out)
    }

    /// Resolves a model-supplied path, refusing anything outside the root.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let p = Path::new(rel);
        let rel = if p.is_absolute() {
            p.strip_prefix(&self.root).map_err(|_| format!("path outside project: {rel}"))?.to_path_buf()
        } else {
            p.to_path_buf()
        };
        let mut out = PathBuf::new();
        for c in rel.components() {
            match c {
                Component::Normal(x) => out.push(x),
                Component::CurDir => {}
                Component::ParentDir => {
                    if !out.pop() {
                        return Err(format!("path outside project: {}", rel.display()));
                    }
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(format!("path outside project: {}", rel.display()));
                }
            }
        }
        Ok(self.root.join(out))
    }

    pub(crate) fn display(&self, p: &Path) -> String {
        p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().replace('\\', "/")
    }

    /// Remembers the content the model just saw for `rel`.
    pub(crate) fn record_read(&self, rel: &str, content: &str) {
        if let Ok(mut m) = self.seen.lock() {
            m.insert(rel.to_string(), content_hash(content));
        }
    }

    /// Whether overwriting `rel` (current on-disk `content`) would clobber a
    /// change made since the model last read it, or a file it never read.
    pub(crate) fn write_conflict(&self, rel: &str, current: &str) -> bool {
        match self.seen.lock().ok().and_then(|m| m.get(rel).copied()) {
            Some(h) => h != content_hash(current),
            None => true,
        }
    }

    /// Unified diff an edit tool would apply, without writing. `None` for
    /// non-edit tools; an error message if the edit itself would fail.
    pub fn preview(&self, name: &str, input: &Value) -> Option<Result<String, String>> {
        if !EDIT_TOOLS.contains(&name) {
            return None;
        }
        Some(self.plan_edit(name, input).map(|(path, old, new)| {
            let rel = self.display(&path);
            let diff = similar::TextDiff::from_lines(&old, &new)
                .unified_diff()
                .context_radius(3)
                .header(&format!("a/{rel}"), &format!("b/{rel}"))
                .to_string();
            let total = diff.lines().count();
            if total <= PREVIEW_MAX_LINES {
                return diff;
            }
            let mut out: String = diff.lines().take(PREVIEW_MAX_LINES).map(|l| format!("{l}
")).collect();
            out.push_str(&format!("[{} more diff lines]
", total - PREVIEW_MAX_LINES));
            out
        }))
    }

    /// Runs a tool. Failures come back as `is_error` outputs, never panics.
    pub async fn call(&self, name: &str, input: &Value) -> ToolOutput {
        let ast_only = matches!(name, "read_symbol" | "replace_symbol" | "insert_symbol" | "workspace_diagnostics");
        if ast_only && self.mode != Mode::Ast {
            return ToolOutput::err(format!("unknown tool: {name}"));
        }
        if name.starts_with("mcp__")
            && let Some(m) = self.mcp.get()
            && let Some(r) = m.call(name, input.clone()).await
        {
            return match r {
                Ok(o) => ToolOutput { content: if o.text.trim().is_empty() { "(no output)".into() } else { o.text }, is_error: o.is_error, images: o.images },
                Err(e) => ToolOutput::err(e),
            };
        }
        let r: Res = match name {
            "read_file" => self.read_file(input),
            "write_file" => self.write_file(input),
            "edit_file" => self.edit_file(input),
            "list_files" => self.list_files(input),
            "grep" => self.grep(input),
            "bash" => self.bash(input).await,
            "read_symbol" => self.read_symbol(input),
            "replace_symbol" => self.replace_symbol(input),
            "insert_symbol" => self.insert_symbol(input),
            "workspace_diagnostics" => {
                let (text, clean) = coturbo_verify::workspace::render(&coturbo_verify::workspace::check(&self.root).await);
                Ok(if clean { ToolOutput::ok(text) } else { ToolOutput::err(text) })
            }
            "propose_plan" => Err("plan mode is not active".into()),
            "design_brief" => self.design_brief(input),
            "design_review" => self.design_review(input).await,
            "investigate" => {
                let question = input["question"].as_str().unwrap_or("");
                match coturbo_rea::investigate(&self.root, arg(input, "target").unwrap_or(""), question).await {
                    Ok(report) => {
                        let mut out = ToolOutput::ok(report.text);
                        if let Some(shot) = report.images.first()
                            && let Ok(png) = std::fs::read(shot)
                        {
                            use base64::Engine;
                            out.images.push(("image/png".into(), base64::engine::general_purpose::STANDARD.encode(png)));
                        }
                        Ok(out)
                    }
                    Err(e) => Err(e),
                }
            }
            "observe_app" => self.observe_app(input).await,
            "tasks" => Self::tasks(input),
            "browse" => self.browse(input).await,
            "network_scan" => devices::network_scan(input).await,
            "device_control" => devices::device_control(input).await,
            "device" => devices::device(input).await,
            "tool_search" => self.tool_search(input),
            "web_search" => web::search(input).await,
            "remember" => memory::remember(&self.root, input),
            "skill" => skills::load(&self.root, input),
            "lsp" => self.lsp_navigate(input).await,
            "ci" => ci::tool(&self.root, input).await,
            "jobs" => self.jobs(input).await,
            "schedule" => schedule::tool(&self.root, input),
            "recall" => memory::recall(&self.root, input),
            "forget" => memory::forget(&self.root, input),
            "web_fetch" => web::fetch(input).await,
            "name_device" => devices::name_device(input),
            _ => Err(format!("unknown tool: {name}")),
        };
        let mut out = r.unwrap_or_else(ToolOutput::err);
        // An empty result reads as a failure to the model, and some providers refuse it.
        if out.content.trim().is_empty() && out.images.is_empty() {
            out.content = match name {
                "list_files" => "(no files)",
                "grep" => "(no matches)",
                _ => "(no output)",
            }
            .into();
        }
        if self.native_checks
            && !out.is_error
            && EDIT_TOOLS.contains(&name)
            && let Some(p) = input["path"].as_str()
            && let Ok(abs) = self.resolve(p)
            && let Ok(text) = std::fs::read_to_string(&abs)
        {
            let rel = self.display(&abs);
            if let Some(diags) = coturbo_verify::native::check(&rel, &text) {
                out.content.push('\n');
                out.content.push_str(&native_note(&rel, &diags));
            }
            if coturbo_design::fiscal::is_ui_file(&rel) {
                let contract = coturbo_design::contract::Contract::load(&self.root);
                let checked = coturbo_design::with_linked_css(&self.root, &rel, &text);
                let findings = coturbo_design::fiscal::check(&rel, &checked, contract.as_ref());
                out.content.push('\n');
                out.content.push_str(&coturbo_design::fiscal::render(&rel, &findings));
            }
        }
        if let Some(lsp) = &self.lsp {
            if !out.is_error
                && EDIT_TOOLS.contains(&name)
                && let Some(p) = input["path"].as_str()
                && let Ok(abs) = self.resolve(p)
            {
                let notes = lsp.after_write(&self.display(&abs)).await;
                if notes.formatted {
                    out.content.push_str(" (formatted)");
                }
                if !notes.text.is_empty() {
                    out.content.push('\n');
                    out.content.push_str(&notes.text);
                }
            }
            for late in lsp.drain_late() {
                out.content.push('\n');
                out.content.push_str(&late);
            }
        }
        out
    }
}
