//! JavaScript and Electron apps: package, modules, IPC channels, routes,
//! network endpoints and the code that relates to the question, with
//! file:line evidence. Works on folders, `.asar` archives and bundles,
//! minified or not.

use regex::Regex;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex")
}

static IMPORT: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?:import\s[^'"]*?from\s*|import\s*\(\s*|require\s*\(\s*)["']([^"'./][^"']*)["']"#));
static IPC: LazyLock<Regex> =
    LazyLock::new(|| re(r#"ipc(?:Main|Renderer)\s*\.\s*(handle|on|invoke|send|sendSync|once)\s*\(\s*["'`]([^"'`]+)["'`]"#));
static ROUTE: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?:path|route|\.(?:get|post|put|delete|patch))\s*[:(]\s*["'`](/[A-Za-z0-9_\-/:.*]*)["'`]"#));
static ENDPOINT: LazyLock<Regex> = LazyLock::new(|| re(r#"["'`](https?://[^"'`\s]{6,120})["'`]"#));

/// Report on a set of JS files `(path, text)`; `package` is package.json.
pub fn inspect_files(files: &[(String, String)], package: Option<&str>, question: &str) -> String {
    let mut r = String::new();
    if let Some(p) = package.and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok()) {
        let name = p["name"].as_str().unwrap_or("?");
        let main = p["main"].as_str().unwrap_or("?");
        let deps: Vec<String> = ["dependencies", "devDependencies"]
            .iter()
            .flat_map(|k| p[*k].as_object().into_iter().flat_map(|o| o.keys().cloned()))
            .take(40)
            .collect();
        r.push_str(&format!("Package: {name} (main: {main})\nDependencies: {}\n", deps.join(", ")));
    }
    let total: usize = files.iter().map(|(_, t)| t.len()).sum();
    r.push_str(&format!("JS files: {} ({} KB)\n", files.len(), total / 1024));
    let mut imports: BTreeMap<String, usize> = BTreeMap::new();
    let mut ipc: BTreeMap<String, String> = BTreeMap::new();
    let mut routes: BTreeMap<String, String> = BTreeMap::new();
    let mut endpoints: BTreeMap<String, String> = BTreeMap::new();
    let terms = crate::terms(question);
    let mut hits = Vec::new();
    for (path, text) in files {
        let line_of = |off: usize| text[..off].matches('\n').count() + 1;
        for c in IMPORT.captures_iter(text) {
            let m = c[1].split('/').take(if c[1].starts_with('@') { 2 } else { 1 }).collect::<Vec<_>>().join("/");
            *imports.entry(m).or_insert(0) += 1;
        }
        for c in IPC.captures_iter(text) {
            let at = format!("{path}:{}", line_of(c.get(0).expect("match").start()));
            ipc.entry(format!("{} ({})", &c[2], &c[1])).or_insert(at);
        }
        for c in ROUTE.captures_iter(text) {
            let at = format!("{path}:{}", line_of(c.get(0).expect("match").start()));
            routes.entry(c[1].to_string()).or_insert(at);
        }
        for c in ENDPOINT.captures_iter(text) {
            let url = c[1].to_string();
            if !url.contains("w3.org") && !url.contains("schema.org") {
                let at = format!("{path}:{}", line_of(c.get(0).expect("match").start()));
                endpoints.entry(url).or_insert(at);
            }
        }
        if !terms.is_empty() && hits.len() < 30 {
            let lower = text.to_lowercase();
            // A few per file, so one huge bundle does not take every slot.
            let mut here = 0;
            for t in &terms {
                let mut from = 0;
                while let Some(i) = lower[from..].find(t.as_str()) {
                    let at = from + i;
                    let (mut a, mut b) = (at.saturating_sub(80), (at + t.len() + 80).min(text.len()));
                    while !text.is_char_boundary(a) {
                        a -= 1;
                    }
                    while !text.is_char_boundary(b) {
                        b += 1;
                    }
                    let snippet = text[a..b].replace(['\n', '\r'], " ");
                    hits.push(format!("  {path}:{}: …{}…", line_of(at), snippet.trim()));
                    from = at + t.len();
                    here += 1;
                    if hits.len() >= 30 || here >= 4 {
                        break;
                    }
                }
            }
        }
    }
    let mut top: Vec<(String, usize)> = imports.into_iter().collect();
    top.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    if !top.is_empty() {
        r.push_str(&format!(
            "Modules used most: {}\n",
            top.iter().take(20).map(|(m, n)| format!("{m}×{n}")).collect::<Vec<_>>().join(", ")
        ));
    }
    let list = |title: &str, m: &BTreeMap<String, String>, r: &mut String| {
        if !m.is_empty() {
            r.push_str(&format!("{title}:\n"));
            for (k, at) in m.iter().take(25) {
                r.push_str(&format!("  {k}  [{at}]\n"));
            }
        }
    };
    list("IPC channels (Electron)", &ipc, &mut r);
    list("Routes", &routes, &mut r);
    list("Network endpoints", &endpoints, &mut r);
    if !terms.is_empty() {
        r.push_str(&format!("Code matching {:?} ({} shown):\n{}\n", terms, hits.len(), hits.join("\n")));
    }
    r.push_str("Unknowns: runtime behavior (values at run time, user flows) is not observed here; read the files cited to confirm.");
    r
}

fn is_js(p: &str) -> bool {
    let p = p.to_lowercase();
    [".js", ".mjs", ".cjs", ".jsx", ".ts", ".tsx"].iter().any(|e| p.ends_with(e)) && !p.ends_with(".d.ts") && !p.ends_with(".map")
}

/// A folder: an app's source, an unpacked Electron app, or its install
/// folder (its `.asar` archives are read too).
pub fn inspect_dir(dir: &Path, question: &str) -> String {
    let mut files = Vec::new();
    let mut package = None;
    let mut asars = Vec::new();
    let mut bytes = 0usize;
    for e in ignore::WalkBuilder::new(dir).hidden(false).build().filter_map(Result::ok) {
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = e.path().strip_prefix(dir).unwrap_or(e.path()).to_string_lossy().replace('\\', "/");
        if rel.contains("node_modules/") {
            continue;
        }
        if rel == "package.json" {
            package = std::fs::read_to_string(e.path()).ok();
        } else if rel.ends_with(".asar") {
            asars.push(e.path().to_path_buf());
        } else if is_js(&rel) && files.len() < 400 && bytes < 40 << 20
            && let Ok(t) = std::fs::read_to_string(e.path())
        {
            bytes += t.len();
            files.push((rel, t));
        }
    }
    let mut r = inspect_files(&files, package.as_deref(), question);
    for a in asars.iter().take(2) {
        if let Ok(data) = std::fs::read(a)
            && let Ok(rep) = inspect_asar(&data, question)
        {
            r.push_str(&format!("\n\n== {} ==\n{rep}", a.display()));
        }
    }
    r
}

/// Files in an Electron `.asar` archive: `(path, bytes)`, unpacked ones skipped.
pub fn asar_files(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let u32_at = |o: usize| -> Result<usize, String> {
        data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).ok_or_else(|| "arquivo .asar curto demais".to_string())
    };
    let header_size = u32_at(4)?;
    let json_len = u32_at(12)?;
    let json = data.get(16..16 + json_len).ok_or("cabeçalho .asar inválido")?;
    let header: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("cabeçalho .asar inválido: {e}"))?;
    let base = 8 + header_size;
    let mut out = Vec::new();
    fn walk(node: &serde_json::Value, prefix: &str, data: &[u8], base: usize, out: &mut Vec<(String, Vec<u8>)>) {
        let Some(files) = node["files"].as_object() else { return };
        for (name, entry) in files {
            let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if entry["files"].is_object() {
                walk(entry, &path, data, base, out);
            } else if entry["unpacked"].as_bool() != Some(true) {
                let size = entry["size"].as_u64().unwrap_or(0) as usize;
                let offset = entry["offset"].as_str().and_then(|o| o.parse::<usize>().ok()).unwrap_or(0);
                if let Some(bytes) = data.get(base + offset..base + offset + size) {
                    out.push((path, bytes.to_vec()));
                }
            }
        }
    }
    walk(&header, "", data, base, &mut out);
    Ok(out)
}

/// An Electron `.asar` archive.
pub fn inspect_asar(data: &[u8], question: &str) -> Result<String, String> {
    let entries = asar_files(data)?;
    let package = entries.iter().find(|(p, _)| p == "package.json").map(|(_, b)| String::from_utf8_lossy(b).into_owned());
    let files: Vec<(String, String)> = entries
        .into_iter()
        .filter(|(p, _)| is_js(p) && !p.contains("node_modules/"))
        .take(400)
        .map(|(p, b)| (p, String::from_utf8_lossy(&b).into_owned()))
        .collect();
    Ok(format!("Electron archive (.asar)\n{}", inspect_files(&files, package.as_deref(), question)))
}
