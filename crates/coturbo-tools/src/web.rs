//! `web_search` and `web_fetch`: search the web and read a page as text,
//! without opening a browser.

use crate::{Res, ToolOutput, arg};
use serde_json::Value;
use std::time::Duration;

const MAX_PAGE: usize = 24_000;

pub(crate) fn client() -> Result<reqwest::Client, String> {
    coturbo_providers::dns::client_builder()
        .timeout(Duration::from_secs(20))
        .user_agent("Mozilla/5.0 (compatible; CodeTurbo; +https://github.com/Chavancharinga/CodeTurbo)")
        .build()
        .map_err(|e| e.to_string())
}

/// `%xx` and `+` decoded.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

pub(crate) fn entities(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&#x27;", "'").replace("&nbsp;", " ")
}

/// Visible text of an HTML page, with headings, list items and links kept.
pub fn html_to_text(html: &str) -> String {
    let lower = html.to_lowercase();
    let mut out = String::new();
    let mut i = 0;
    let bytes = html.as_bytes();
    let mut href: Option<String> = None;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let Some(end) = lower[i..].find('>').map(|e| i + e) else { break };
            let tag = &lower[i + 1..end];
            let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            // Skip what is never shown.
            if !tag.starts_with('/') && matches!(name.as_str(), "script" | "style" | "noscript" | "svg" | "template" | "head") {
                let close = format!("</{name}");
                i = lower[end..].find(&close).map_or(bytes.len(), |c| end + c);
                continue;
            }
            match (tag.starts_with('/'), name.as_str()) {
                (false, "h1" | "h2" | "h3" | "h4") => out.push_str(&format!("\n\n{} ", "#".repeat(name[1..].parse().unwrap_or(2)))),
                (false, "li") => out.push_str("\n- "),
                (false, "p" | "div" | "section" | "article" | "tr" | "br" | "table" | "header" | "footer" | "main") => out.push('\n'),
                (false, "a") => {
                    href = html[i..end].split("href=").nth(1).map(|h| h.trim_start_matches(['"', '\'']).split(['"', '\'', ' ']).next().unwrap_or("").to_string());
                }
                (true, "a") => {
                    if let Some(h) = href.take().filter(|h| h.starts_with("http")) {
                        out.push_str(&format!(" ({h})"));
                    }
                }
                _ => {}
            }
            i = end + 1;
            continue;
        }
        let next = lower[i..].find('<').map_or(bytes.len(), |n| i + n);
        out.push_str(&entities(&html[i..next]));
        i = next;
    }
    // Collapse the whitespace.
    let mut clean = String::new();
    for line in out.lines() {
        let l = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !l.is_empty() {
            clean.push_str(&l);
            clean.push('\n');
        } else if !clean.ends_with("\n\n") && !clean.is_empty() {
            clean.push('\n');
        }
    }
    clean.trim().to_string()
}

pub(crate) async fn fetch(input: &Value) -> Res {
    let url = arg(input, "url")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("`url` must start with http:// or https://".into());
    }
    let resp = client()?.get(url).send().await.map_err(|e| format!("could not reach {url}: {e}"))?;
    let status = resp.status();
    let kind = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let body = resp.text().await.map_err(|e| e.to_string())?;
    let text = if kind.contains("html") || body.trim_start().starts_with('<') { html_to_text(&body) } else { body };
    let mut text: String = text.chars().take(MAX_PAGE).collect();
    if text.len() >= MAX_PAGE {
        text.push_str("\n[... page cut here]");
    }
    let out = format!("{url} ({status})\n\n{text}");
    Ok(if status.is_success() { ToolOutput::ok(out) } else { ToolOutput::err(out) })
}

/// `(title, url, snippet)` of DuckDuckGo's HTML results.
pub fn parse_duckduckgo(html: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for block in html.split("result__a").skip(1) {
        let Some(href) = block.split("href=\"").nth(1).and_then(|h| h.split('"').next()) else { continue };
        let url = match href.split("uddg=").nth(1) {
            Some(u) => percent_decode(u.split('&').next().unwrap_or(u)),
            None => href.to_string(),
        };
        let title = block.split_once('>').and_then(|(_, t)| t.split("</a").next()).map(html_to_text).unwrap_or_default();
        let snippet = block.split("result__snippet").nth(1).and_then(|s| s.split_once('>')).and_then(|(_, s)| s.split("</a").next()).map(html_to_text).unwrap_or_default();
        if url.starts_with("http") && !url.contains("duckduckgo.com/y.js") {
            out.push((title, url, snippet));
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

pub(crate) async fn search(input: &Value) -> Res {
    let query = arg(input, "query")?;
    let results: Vec<(String, String, String)> = if let Ok(key) = std::env::var("BRAVE_API_KEY") {
        let v: Value = client()?
            .get(format!("https://api.search.brave.com/res/v1/web/search?q={}&count=8", percent_encode(query)))
            .header("X-Subscription-Token", key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        v["web"]["results"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| (r["title"].as_str().unwrap_or("").into(), r["url"].as_str().unwrap_or("").into(), html_to_text(r["description"].as_str().unwrap_or(""))))
            .collect()
    } else {
        let html = client()?
            .post("https://html.duckduckgo.com/html/")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!("q={}", percent_encode(query)))
            .send()
            .await
            .map_err(|e| format!("the search did not answer: {e}"))?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        parse_duckduckgo(&html)
    };
    if results.is_empty() {
        return Err(format!("no results for `{query}` (the search engine may be limiting requests; try again or rephrase)"));
    }
    let list: Vec<String> = results.iter().enumerate().map(|(i, (t, u, s))| format!("{}. {t}\n   {u}\n   {s}", i + 1)).collect();
    Ok(ToolOutput::ok(format!("Results for `{query}` (read one with web_fetch):\n{}", list.join("\n"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_become_readable_text() {
        let html = "<html><head><title>x</title><style>p{}</style></head><body><h1>Rust &amp; you</h1><script>var a=1</script><p>Read <a href=\"https://rust-lang.org\">the site</a>.</p><ul><li>one</li><li>two</li></ul></body></html>";
        let t = html_to_text(html);
        assert!(t.contains("# Rust & you"), "{t}");
        assert!(t.contains("the site (https://rust-lang.org)"), "{t}");
        assert!(t.contains("- one") && t.contains("- two"), "{t}");
        assert!(!t.contains("var a") && !t.contains("p{}"), "{t}");
    }

    #[test]
    fn reads_duckduckgo_results() {
        let html = r#"<a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdocs.rs%2Ftokio&amp;rut=x">Tokio <b>docs</b></a><a class="result__snippet" href="x">An async runtime.</a>"#;
        let r = parse_duckduckgo(html);
        assert_eq!(r, vec![("Tokio docs".to_string(), "https://docs.rs/tokio".to_string(), "An async runtime.".to_string())]);
        assert_eq!(percent_decode("a%20b+c"), "a b c");
    }
}
