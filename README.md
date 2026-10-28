<p align="center">
  <img src="docs/codeturbo.svg" alt="CodeTurbo" width="618">
</p>

<p align="center"><b>A terminal coding agent that ships code that builds, lints and passes its tests.</b></p>

<p align="center">
  <a href="https://github.com/Chavancharinga/CodeTurbo/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/Chavancharinga/CodeTurbo?style=flat-square&color=2F7BFF"></a>
  <a href="#license"><img alt="License: MIT or Apache-2.0" src="https://img.shields.io/badge/license-MIT%20or%20Apache--2.0-2F7BFF?style=flat-square"></a>
  <img alt="Rust 1.94+" src="https://img.shields.io/badge/rust-1.94%2B-2F7BFF?style=flat-square&logo=rust&logoColor=white">
  <img alt="100% Rust" src="https://img.shields.io/badge/100%25-Rust-2F7BFF?style=flat-square">
</p>

---

CodeTurbo is a coding agent for the terminal, written entirely in Rust. It
works with the model you already use and adds what a model cannot do alone: a
verification gate that refuses unfinished work, a design engine, reverse
engineering tools, a real browser it can drive, and an editing path that sends
far fewer tokens per task.

```sh
codeturbo
```

<p align="center">
  <img src="docs/screens/browser-windows.png" alt="Four GitHub pages open at the same time inside the terminal" width="900">
</p>

## Features

**Verified delivery**
Work is done only when the project compiles, lints clean and passes its
tests. A failing step goes back to the model for a fix, automatically. Removed
or ignored tests and silenced warnings are detected and rejected.

**Fewer tokens per task**
The agent reads and edits the exact symbol it needs instead of whole files,
analyses each change the moment it is made (Rust, JavaScript, TypeScript, CSS,
HTML, JSON, TOML) and uses provider prompt caching. Tokens, speed, context
usage, cost and rate limits stay visible the whole time.

**A browser inside the terminal**
The agent opens real pages, clicks, fills forms and tests menus while you watch
in a mini window. On terminals that draw images (Kitty, iTerm2, Sixel, Windows
Terminal) the page is a crisp picture with the agent's pointer and clicks drawn
on it; elsewhere it opens a real window beside the terminal or falls back to
blocks. To check a site with many pages, the agent opens up to six small
windows at once and drives them together.

**Design engine**
Pages and interfaces are reviewed on desktop and phone layouts against design
rules (overlaps, cut-off content, tap targets, spacing, consistency). After
every edit the inspector also runs the static gates of
[Hallmark](https://github.com/Nutlope/hallmark): the purple gradient, the
eyebrow over every heading, tracked capitals, `transition: all`, content
hidden until a script runs, invented metrics, a form that says thanks
without sending anything, and more. A design that fails is refused, with the
evidence.

**Sensing through WiFi**
People disturb the radio waves that fill a room. Type `/sense` (or ask) and
a full-screen view shows, live, whether someone is moving or still, with
the signal over the last minute; `x` closes it. On a PC or a phone (Termux,
with Termux:API) it reads the device's own WiFi signal, which shows movement
near the path to the router. With an ESP32 node running CSI firmware
(the frame [RuView](https://github.com/ruvnet/RuView)'s nodes send, UDP port
5005) it reads every subcarrier, and finds the breathing rate of a person
who is still. Readings are estimates: not a camera, not a medical device.

**What people said this month**
`last30days` searches Hacker News, GitHub, Polymarket, dev.to, arXiv and
Reddit (with your Reddit API app) at the same time, keeps what is on topic
and ranks it by engagement: points, reactions, comments and money traded.
The agent then writes a brief that links every claim. Based on
[last30days](https://github.com/mvanhorn/last30days-skill).

**Built-in reverse engineering**
The agent studies how a website, a JavaScript or Electron app, a native binary
(including x86/x64 disassembly of the relevant functions), an Android APK or a
SQLite database works, with file-and-line evidence, before building something
similar. All in Rust, with no external tools.

**Devices on your network**
The agent finds what is on your local network and recognises each device by
kind, brand and model, from every clue the network gives: Bonjour/mDNS and
UPnP announcements, the router's name for it, the maker of its network card
(IEEE registry) and its web page. Each device is then controlled locally in
its own protocol, with no cloud:

| Devices | Protocols | Control |
| --- | --- | --- |
| TVs | Samsung, LG, Android TV / Google TV, Roku, Fire TV, DIAL | power, volume, keys, apps, YouTube/Netflix |
| Speakers and players | Google Cast, Sonos and UPnP/DLNA | play, pause, next, volume, what is playing |
| Lights, plugs, switches | Philips Hue, Shelly, Tasmota, WLED, TP-Link Kasa, Yeelight | on, off, brightness, colour, power use |
| Printers | IPP | state, ink and toner levels |
| Routers | UPnP | internet status, public address |
| Zigbee and more | MQTT (Zigbee2MQTT, Tasmota, Shelly, ESPHome) | publish commands, read what devices report |
| Computers | Wake-on-LAN | switch on |

Devices are remembered by name ("living room TV"), so a TV that is off can
still be switched on. Type `/devices` to pick a device and act on it with a
click, or just ask. No hub or home-automation server is needed, control
reaches private addresses only, and the agent's actions are approved by you
first.

**Any provider, found automatically**
CodeTurbo detects what is on your machine and lists only that: the official
Claude Code, Codex and Antigravity CLIs (so your own plan is used), a running
Ollama or LM Studio, and any provider you have a key for. Anthropic, OpenAI,
Gemini, OpenRouter, Groq, DeepSeek, Mistral, xAI, Together and any
OpenAI-compatible server are supported.

**An agent that scales with the job**
- **MCP**: connect external tool servers (GitHub, Figma, databases, Slack...)
  from `config.toml` or the project's `.mcp.json` (the same file Claude Code
  reads). Many tools are only named until needed, to keep prompts small.
- **Subagents**: hand research, planning or review to fresh agents that run
  in parallel; a `general` subagent works in its own git worktree and hands
  back a branch.
- **Memory between sessions**: who you are, how you like to work and the
  project's decisions, remembered per project.
- **Hooks, skills and custom commands**: your own commands before and after
  tools, skill packs (`SKILL.md`) and `/commands` from Markdown files,
  including the ones already in `~/.claude`. Hallmark, last30days and RuView
  ship built in, with their mechanisms running natively.
- **Small requests**: a short system prompt, and the tools for particular
  jobs (browser, devices, reverse engineering, research, sensing...) only
  named until the agent loads one, so each request carries about half the
  fixed tokens it used to.
- **Parallel reads**: the files, searches and pages asked in one reply are
  read at the same time.
- **`/review`**: the changes split by area and reviewed by several agents at
  once; only the findings that hold up are reported.
- **Background jobs**: servers and watchers keep running; their new output is
  read and they are stopped on demand.
- **Every command of a chain is approved**: allowing `git` never lets
  `git diff && rm -rf build` through as one command.
- **A guard for hard-to-undo commands** (`rm -rf` of a variable, forced push,
  `git reset --hard`, shutting the computer down...): they always ask first,
  in any approval mode.
- **The browser's console and requests**, like in its developer tools.
- **Web search and page reading**, **scheduled tasks** ("every day at 23:00"),
  **CI follow-up** through `gh`, **questions with options**, **PDFs,
  notebooks and images**, and **code navigation** through the language server.

**Control over every action**
Approval modes, per-tool and per-command rules, every change shown before it
is applied, and a read-only plan mode that you approve before anything is
modified. A folder must be trusted once before CodeTurbo touches it.

**One native executable**
No Node.js, no Python, no external services. Code analysis is built in;
language servers already installed on the machine are used when present.
Updates are one command.

## In action

Everything below is the real screen, recorded while the agent worked on a real
GitHub account and on CodeTurbo's own repository. Nothing is a mockup.

**The code appears as it is written,** in a box beside the pages the agent keeps
open.

<p align="center"><img src="docs/screens/live-code.png" alt="Code being written live in a box, next to two real GitHub pages" width="900"></p>

**A debugger that points at the exact line.** When a check fails, the failing
lines are shown with the column and the reason, the snail turns yellow, and the
fix starts on its own.

<p align="center"><img src="docs/screens/debugger.png" alt="The debugger showing two lint failures with their lines, the FIX bar starting" width="900"></p>

**The verification gate at full speed.** The snail goes flat out while lint and
tests load, bar by bar.

<p align="center"><img src="docs/screens/turbo-gate.png" alt="The verification bars loading, with the tests at 91 percent" width="900"></p>

**Done means verified.** The snail turns green only when everything passed.

<p align="center"><img src="docs/screens/done.png" alt="All steps complete, verified, the snail is green" width="900"></p>

## Install

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/Chavancharinga/CodeTurbo/main/install.ps1 | iex
```

Linux, macOS and Android (Termux):

```sh
curl -fsSL https://raw.githubusercontent.com/Chavancharinga/CodeTurbo/main/install.sh | sh
```

From source, with Rust 1.94 or newer:

```sh
cargo install --locked --git https://github.com/Chavancharinga/CodeTurbo coturbo-cli
```

## Quick start

Open a terminal in your project and run:

```sh
codeturbo
```

On first run CodeTurbo shows the providers it found on your machine and lets you
connect another with an API key. Nothing is preconfigured. Choices are saved
for the next sessions.

To run one task without the interface:

```sh
codeturbo run "add email validation to the signup form"
```

### Commands

| Command      | What it does                                            |
| ------------ | ------------------------------------------------------- |
| `/provider`  | choose the provider: API, local server or your own plan |
| `/model`     | choose among the provider's models                      |
| `/key`       | set the provider API key                                |
| `/approval`  | choose when CodeTurbo asks before acting                |
| `/plan`      | read-only plan mode until you approve the plan          |
| `/usage`     | tokens and cost of the session                          |
| `/sense`     | sense movement and breathing through WiFi (`x` closes)  |
| `/devices`   | find the devices on your network and control them       |
| `/undo`      | undo the changes of the last task                       |
| `/retry`     | run the last task again                                 |
| `/rule`      | pin a rule the model must always follow                 |
| `/copy`      | copy the last answer                                    |
| `/export`    | save the conversation as Markdown                       |
| `/compact`   | shrink old results and free context                     |
| `/clear`     | start a new conversation                                |
| `/update`    | install the latest version                              |
| `/help`      | list commands and shortcuts                             |
| `/exit`      | quit                                                    |

### Providers and keys

- **Your plan, no key:** choose Claude Code, Codex or Antigravity in
  `/provider`. CodeTurbo talks to the official tool already installed and signed
  in on your machine, which uses your plan as each company's terms allow, while
  the tools, the design engine and the verification stay CodeTurbo's.
- **Local models:** Ollama and LM Studio need no key and cost nothing to run.
- **Hosted models:** set a key with `/key` or with `ANTHROPIC_API_KEY`,
  `OPENAI_API_KEY`, `GEMINI_API_KEY`, `OPENROUTER_API_KEY`, `GROQ_API_KEY`,
  `DEEPSEEK_API_KEY`, `MISTRAL_API_KEY`, `XAI_API_KEY` or `TOGETHER_API_KEY`.
  Keys are stored only on your computer, outside any project.

### Project conventions

Write coding style, folders that must not change and usual commands in
`CODETURBO.md` at the project root. CodeTurbo follows it in every session and
also reads `AGENTS.md` and `CLAUDE.md` when present.

### Configuration

Preferences live in `%APPDATA%\codeturbo\config.toml` on Windows and
`~/.config/codeturbo/config.toml` on Linux and macOS.

| Variable                       | Effect                                                         |
| ------------------------------ | -------------------------------------------------------------- |
| `CODETURBO_BROWSER`            | `window` or `headless`: how the agent's browser opens          |
| `CODETURBO_GRAPHICS`           | `off` keeps the mini browser in blocks instead of an image     |
| `CODETURBO_NO_UPDATE_CHECK`    | turns the daily update notice off                              |
| `REDDIT_CLIENT_ID`, `REDDIT_CLIENT_SECRET` | adds Reddit to `last30days` (a "script" app from reddit.com/prefs/apps) |
| `GITHUB_TOKEN`                 | raises GitHub's search limits for `last30days`                 |

## Updating

CodeTurbo checks once a day for a newer release and tells you when it opens.

```sh
codeturbo update
```

## Status

Under active development. Releases and notes are on the
[releases page](https://github.com/Chavancharinga/CodeTurbo/releases).

## License

The built-in skills keep their own MIT licences, in their folders:
Hallmark (Hallmark contributors) and last30days (Matt Van Horn).

Licensed, at your option, under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

Unless you state otherwise, any contribution you submit for inclusion in the
project is licensed the same way, without any additional terms or conditions.
