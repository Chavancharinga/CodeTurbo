//! O que o agente pode fazer no navegador, como pedido pela ferramenta
//! `browse`: abrir uma página, listar o que dá para clicar, clicar, preencher,
//! apertar teclas, rolar, esperar, voltar, mudar o tamanho da janela, ler o
//! texto da página e tirar uma foto.

use serde_json::Value;

/// How an element is named: by the reference `snapshot` gave it (`@e7`), by
/// its visible text, or by a CSS selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Ref(String),
    Text(String),
    Selector(String),
}

impl Target {
    /// The target as the page's script understands it.
    pub fn to_json(&self) -> Value {
        match self {
            Target::Ref(r) => serde_json::json!({"ref": r.trim_start_matches('@')}),
            Target::Text(t) => serde_json::json!({"text": t}),
            Target::Selector(s) => serde_json::json!({"selector": s}),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Target::Ref(r) => format!("@{}", r.trim_start_matches('@')),
            Target::Text(t) => format!("\"{t}\""),
            Target::Selector(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Open(String),
    Snapshot,
    Click(Target),
    Fill(Target, String),
    Press(String),
    Scroll(i64),
    Wait(u64),
    Back,
    Viewport(u32, u32),
    /// The page's text, all of it or only that of an element.
    Text(Option<Target>),
    Shot,
    /// Everything the page wrote to its console so far.
    Console,
    /// Every request the page made so far, with its status.
    Network,
}

fn target(a: &Value) -> Result<Target, String> {
    if let Some(r) = a["ref"].as_str().filter(|s| !s.trim().is_empty()) {
        return Ok(Target::Ref(r.trim().to_string()));
    }
    if let Some(t) = a["text"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .filter(|_| a["do"] != "fill")
    {
        return Ok(Target::Text(t.trim().to_string()));
    }
    if let Some(s) = a["selector"].as_str().filter(|s| !s.trim().is_empty()) {
        return Ok(Target::Selector(s.trim().to_string()));
    }
    if let Some(l) = a["label"].as_str().filter(|s| !s.trim().is_empty()) {
        return Ok(Target::Text(l.trim().to_string()));
    }
    Err(
        "name the target: ref (from the snapshot), text (the visible text) or selector (CSS)"
            .into(),
    )
}

impl Action {
    /// One action from its JSON (`{"do": "click", "text": "Preços"}`).
    pub fn parse(a: &Value) -> Result<Action, String> {
        let kind = a["do"]
            .as_str()
            .or_else(|| a["action"].as_str())
            .ok_or("every action needs \"do\"")?;
        Ok(match kind {
            "open" | "goto" | "navigate" => Action::Open(
                a["url"]
                    .as_str()
                    .filter(|u| !u.trim().is_empty())
                    .ok_or("open precisa de url")?
                    .trim()
                    .to_string(),
            ),
            "snapshot" | "map" => Action::Snapshot,
            "click" => Action::Click(target(a)?),
            "fill" | "type" => {
                let text = a["value"]
                    .as_str()
                    .or_else(|| a["text"].as_str())
                    .ok_or("fill precisa do texto em \"value\"")?
                    .to_string();
                // The text to type is `value`; the field is named by ref, selector or label.
                let mut named = a.clone();
                if let Some(o) = named.as_object_mut() {
                    o.remove("text");
                }
                Action::Fill(target(&named)?, text)
            }
            "press" | "key" => Action::Press(
                a["key"]
                    .as_str()
                    .filter(|k| !k.is_empty())
                    .ok_or("press precisa de key")?
                    .to_string(),
            ),
            "scroll" => Action::Scroll(a["y"].as_i64().unwrap_or(600)),
            "wait" => Action::Wait(a["ms"].as_u64().unwrap_or(500).min(10_000)),
            "back" => Action::Back,
            "viewport" => Action::Viewport(
                a["width"].as_u64().unwrap_or(1000).clamp(240, 3000) as u32,
                a["height"].as_u64().unwrap_or(650).clamp(240, 3000) as u32,
            ),
            "text" | "read" => Action::Text(target(a).ok()),
            "shot" | "screenshot" => Action::Shot,
            "console" | "logs" => Action::Console,
            "network" | "requests" => Action::Network,
            other => return Err(format!("unknown action: {other}")),
        })
    }

    /// The whole list; the error names the step that is wrong.
    pub fn parse_all(list: &Value) -> Result<Vec<Action>, String> {
        let items = list
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or("actions must be a list with at least one action")?;
        if items.len() > 40 {
            return Err("at most 40 actions per call".into());
        }
        items
            .iter()
            .enumerate()
            .map(|(i, a)| Action::parse(a).map_err(|e| format!("action {}: {e}", i + 1)))
            .collect()
    }

    /// A short phrase for the mini window's caption.
    pub fn caption(&self) -> String {
        match self {
            Action::Open(u) => format!("opening {u}"),
            Action::Snapshot => "reading the page".into(),
            Action::Click(t) => format!("clicking {}", t.describe()),
            Action::Fill(t, v) => format!("filling {} with \"{v}\"", t.describe()),
            Action::Press(k) => format!("key {k}"),
            Action::Scroll(y) => format!("scrolling {y}px"),
            Action::Wait(ms) => format!("waiting {ms} ms"),
            Action::Back => "going back".into(),
            Action::Viewport(w, h) => format!("window {w}×{h}"),
            Action::Text(_) => "reading the text".into(),
            Action::Shot => "page photo".into(),
            Action::Console => "reading the console".into(),
            Action::Network => "reading the requests".into(),
        }
    }
}
