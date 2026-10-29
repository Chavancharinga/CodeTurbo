//! Websites: the page rendered in the installed browser (its scripts run),
//! then its structure, technology, design system and screenshots.

use crate::Report;
use coturbo_design::{extract, render};
use regex::Regex;
use std::path::Path;

fn host(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split(['/', '?', '#']).next().unwrap_or("site").replace(':', "_")
}

fn absolute(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        let origin: String = base.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
        if href.starts_with('/') { format!("{origin}{href}") } else { format!("{}/{href}", base.trim_end_matches(|c| c != '/').trim_end_matches('/')) }
    }
}

/// What the rendered HTML says about structure and technology.
pub fn describe(dom: &str) -> String {
    let get = |p: &str| Regex::new(p).expect("valid regex");
    let title = get(r"(?is)<title[^>]*>(.*?)</title>").captures(dom).map(|c| c[1].trim().to_string()).unwrap_or_default();
    let mut r = format!("Title: {title}\n");
    let heads: Vec<String> = get(r"(?is)<h([1-3])[^>]*>(.*?)</h[1-3]>")
        .captures_iter(dom)
        .take(25)
        .map(|c| {
            let text = get(r"<[^>]+>").replace_all(&c[2], "");
            format!("{}h{} {}", "  ".repeat(c[1].parse::<usize>().unwrap_or(1) - 1), &c[1], text.split_whitespace().collect::<Vec<_>>().join(" "))
        })
        .collect();
    if !heads.is_empty() {
        r.push_str(&format!("Outline:\n{}\n", heads.join("\n")));
    }
    let landmarks: Vec<String> = get(r#"(?i)<(header|nav|main|section|footer|aside)\b([^>]*)>"#)
        .captures_iter(dom)
        .take(20)
        .map(|c| {
            let id = get(r#"id="([^"]+)""#).captures(&c[2]).map(|m| format!("#{}", &m[1])).unwrap_or_default();
            format!("{}{id}", c[1].to_lowercase())
        })
        .collect();
    if !landmarks.is_empty() {
        r.push_str(&format!("Sections in order: {}\n", landmarks.join(" → ")));
    }
    let tech: Vec<&str> = [
        ("__NEXT_DATA__", "Next.js"),
        ("data-reactroot", "React"),
        ("_reactListening", "React"),
        ("__NUXT__", "Nuxt"),
        ("data-v-", "Vue"),
        ("ng-version", "Angular"),
        ("data-svelte", "Svelte"),
        ("wp-content", "WordPress"),
        ("cdn.shopify.com", "Shopify"),
        ("webflow", "Webflow"),
        ("framer", "Framer"),
        ("tailwind", "Tailwind"),
    ]
    .iter()
    .filter(|(k, _)| dom.contains(k))
    .map(|(_, v)| *v)
    .collect();
    if !tech.is_empty() {
        r.push_str(&format!("Technology hints: {}\n", tech.join(", ")));
    }
    let scripts = get(r#"(?i)<script[^>]+src="([^"]+)""#).captures_iter(dom).count();
    r.push_str(&format!("Scripts loaded: {scripts}; forms: {}; images: {}\n", dom.matches("<form").count(), dom.matches("<img").count()));
    r
}

/// Inspects a website at `url`.
pub async fn inspect(root: &Path, url: &str, question: &str) -> Result<Report, String> {
    let browser = render::find_browser().ok_or("no browser found (Edge or Chrome)")?;
    let dom = render::dump_dom(&browser, url).await?;
    if dom.trim().is_empty() {
        return Err(format!("{url} não carregou"));
    }
    let mut r = format!("Website {url}\n{}", describe(&dom));
    // Its stylesheets, for the design system.
    let hrefs: Vec<String> = Regex::new(r#"(?i)<link[^>]+rel="stylesheet"[^>]*href="([^"]+)"|<link[^>]+href="([^"]+)"[^>]*rel="stylesheet""#)
        .expect("valid regex")
        .captures_iter(&dom)
        .filter_map(|c| c.get(1).or(c.get(2)).map(|m| absolute(url, &m.as_str().replace("&amp;", "&"))))
        .take(5)
        .collect();
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build().map_err(|e| e.to_string())?;
    let mut files = vec![("page.html".to_string(), dom.clone())];
    for h in &hrefs {
        if let Ok(resp) = client.get(h).send().await
            && let Ok(css) = resp.text().await
        {
            files.push((format!("{}.css", host(h)), css));
        }
    }
    let system = extract::from_files(&files);
    if !system.is_empty() {
        r.push_str(&format!("Design system:\n{}\n", extract::summary(&system)));
    }
    let terms = crate::terms(question);
    if !terms.is_empty() {
        let text = Regex::new(r"(?s)<script.*?</script>|<style.*?</style>|<[^>]+>").expect("valid regex").replace_all(&dom, " ");
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut hits = Vec::new();
        for (i, w) in words.iter().enumerate() {
            if terms.iter().any(|t| w.to_lowercase().contains(t)) && hits.len() < 12 {
                hits.push(format!("  …{}…", words[i.saturating_sub(10)..(i + 10).min(words.len())].join(" ")));
            }
        }
        r.push_str(&format!("Text matching {:?}:\n{}\n", terms, hits.join("\n")));
    }
    let dir = root.join(".codeturbo/rea");
    let stem = host(url);
    let desk = dir.join(format!("{stem}.png"));
    let narrow = dir.join(format!("{stem}-estreito.png"));
    let (a, b) = tokio::join!(render::screenshot(&browser, url, &desk, render::DESKTOP), render::screenshot(&browser, url, &narrow, (520, 900)));
    let mut images = Vec::new();
    for (res, p) in [(a, desk), (b, narrow)] {
        if res.is_ok() {
            r.push_str(&format!("Screenshot: {}\n", p.display()));
            images.push(p);
        }
    }
    r.push_str("Unknowns: content behind login, interactions and server code are not observed.");
    Ok(Report { text: r, images })
}
