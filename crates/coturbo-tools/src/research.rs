//! `last30days`: what people said about a topic in the last days, from the
//! official public APIs of Hacker News, GitHub, Polymarket, dev.to and arXiv
//! (and Reddit, through the user's own Reddit API app), searched at the same
//! time and ranked by what people engaged with: points, reactions, comments,
//! money on the line. A native take on mvanhorn/last30days-skill (MIT).

use crate::web::{client, entities, percent_encode};
use crate::{Res, ToolOutput, arg};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub source: &'static str,
    pub title: String,
    pub url: String,
    /// Unix seconds.
    pub date: u64,
    /// Weighted log of the engagement counts; comparable inside one source.
    pub engagement: f64,
    /// The counts as a person reads them.
    pub stats: String,
    pub note: String,
}

pub const SOURCES: [&str; 6] = [
    "hackernews",
    "reddit",
    "github",
    "polymarket",
    "devto",
    "arxiv",
];

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn ln(n: f64) -> f64 {
    n.max(0.0).ln_1p()
}

fn num(v: &Value) -> f64 {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0.0)
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").trim().to_string()
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// `YYYY-MM-DD` of a Unix time.
pub fn ymd(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + i64::from(m <= 2))
}

/// Unix time of an ISO 8601 stamp (`2026-10-01T05:07:00Z`, offsets ignored).
pub fn iso_secs(s: &str) -> u64 {
    let n = |r: std::ops::Range<usize>| s.get(r).and_then(|x| x.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d)) = (n(0..4), n(5..7), n(8..10)) else {
        return 0;
    };
    let (h, mi, se) = (
        n(11..13).unwrap_or(0),
        n(14..16).unwrap_or(0),
        n(17..19).unwrap_or(0),
    );
    (days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se).max(0) as u64
}

fn short(n: f64) -> String {
    match n {
        n if n >= 1e6 => format!("{:.1}M", n / 1e6),
        n if n >= 1e4 => format!("{:.0}k", n / 1e3),
        n => format!("{n:.0}"),
    }
}

pub fn parse_hackernews(v: &Value) -> Vec<Item> {
    let hits = v["hits"].as_array().cloned().unwrap_or_default();
    hits.iter()
        .map(|h| {
            let (points, comments) = (num(&h["points"]), num(&h["num_comments"]));
            let id = text(&h["objectID"]);
            let thread = format!("https://news.ycombinator.com/item?id={id}");
            let link = text(&h["url"]);
            Item {
                source: "hackernews",
                title: text(&h["title"]),
                url: thread,
                date: h["created_at_i"].as_u64().unwrap_or(0),
                engagement: 0.6 * ln(points) + 0.4 * ln(comments),
                stats: format!("{} points, {} comments", short(points), short(comments)),
                note: link,
            }
        })
        .collect()
}

/// Reddit's official API (`oauth.reddit.com/search`) listing.
pub fn parse_reddit(v: &Value) -> Vec<Item> {
    let posts = v["data"]["children"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    posts
        .iter()
        .map(|c| &c["data"])
        .map(|p| {
            let (score, comments) = (num(&p["score"]), num(&p["num_comments"]));
            let ratio = p["upvote_ratio"].as_f64().unwrap_or(0.8);
            Item {
                source: "reddit",
                title: text(&p["title"]),
                url: format!("https://www.reddit.com{}", text(&p["permalink"])),
                date: num(&p["created_utc"]) as u64,
                engagement: 0.55 * ln(score) + 0.35 * ln(comments) + 0.1 * ratio * 5.0,
                stats: format!(
                    "r/{}, {} upvotes, {} comments",
                    text(&p["subreddit"]),
                    short(score),
                    short(comments)
                ),
                note: text(&p["selftext"]).chars().take(200).collect(),
            }
        })
        .collect()
}

pub fn parse_github_issues(v: &Value) -> Vec<Item> {
    let items = v["items"].as_array().cloned().unwrap_or_default();
    items
        .iter()
        .map(|i| {
            let (reactions, comments) = (num(&i["reactions"]["total_count"]), num(&i["comments"]));
            let repo = text(&i["repository_url"])
                .trim_start_matches("https://api.github.com/repos/")
                .to_string();
            let kind = if i["pull_request"].is_object() {
                "PR"
            } else {
                "issue"
            };
            Item {
                source: "github",
                title: text(&i["title"]),
                url: text(&i["html_url"]),
                date: iso_secs(&text(&i["created_at"])),
                engagement: 0.6 * ln(reactions) + 0.4 * ln(comments),
                stats: format!(
                    "{kind} in {repo}, {} reactions, {} comments",
                    short(reactions),
                    short(comments)
                ),
                note: String::new(),
            }
        })
        .collect()
}

pub fn parse_github_repos(v: &Value) -> Vec<Item> {
    let items = v["items"].as_array().cloned().unwrap_or_default();
    items
        .iter()
        .map(|r| {
            let stars = num(&r["stargazers_count"]);
            Item {
                source: "github",
                title: text(&r["full_name"]),
                url: text(&r["html_url"]),
                date: iso_secs(&text(&r["created_at"])),
                engagement: ln(stars),
                stats: format!("new repository, {} stars", short(stars)),
                note: text(&r["description"]),
            }
        })
        .collect()
}

/// The odds of one market: `Yes 12%`.
fn odds(market: &Value) -> String {
    let list = |k: &str| -> Vec<String> {
        match &market[k] {
            Value::String(s) => serde_json::from_str::<Vec<Value>>(s)
                .unwrap_or_default()
                .iter()
                .map(|x| {
                    x.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| x.to_string())
                })
                .collect(),
            Value::Array(a) => a
                .iter()
                .map(|x| {
                    x.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| x.to_string())
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    let (names, prices) = (list("outcomes"), list("outcomePrices"));
    names
        .iter()
        .zip(prices.iter())
        .take(2)
        .map(|(n, p)| format!("{n} {:.0}%", p.parse::<f64>().unwrap_or(0.0) * 100.0))
        .collect::<Vec<_>>()
        .join(" / ")
}

pub fn parse_polymarket(v: &Value) -> Vec<Item> {
    let events = v["events"].as_array().cloned().unwrap_or_default();
    events
        .iter()
        .filter(|e| !e["closed"].as_bool().unwrap_or(false))
        .map(|e| {
            let month = num(&e["volume1mo"]).max(num(&e["volume24hr"]));
            let markets = e["markets"].as_array().cloned().unwrap_or_default();
            let top = markets
                .iter()
                .filter(|m| !m["closed"].as_bool().unwrap_or(false))
                .max_by(|a, b| num(&a["volume"]).total_cmp(&num(&b["volume"])));
            let (question, price) = top
                .map(|m| (text(&m["question"]), odds(m)))
                .unwrap_or_default();
            Item {
                source: "polymarket",
                title: text(&e["title"]),
                url: format!("https://polymarket.com/event/{}", text(&e["slug"])),
                date: iso_secs(&text(&e["updatedAt"])),
                engagement: ln(month) / 2.0,
                stats: format!(
                    "${} traded this month{}",
                    short(month),
                    if price.is_empty() {
                        String::new()
                    } else {
                        format!(", {question}: {price}")
                    }
                ),
                note: String::new(),
            }
        })
        .collect()
}

pub fn parse_devto(v: &Value) -> Vec<Item> {
    let rows = v.as_array().cloned().unwrap_or_default();
    rows.iter()
        .map(|a| {
            let (reactions, comments) =
                (num(&a["public_reactions_count"]), num(&a["comments_count"]));
            Item {
                source: "devto",
                title: text(&a["title"]),
                url: text(&a["url"]),
                date: iso_secs(&text(&a["published_at"])),
                engagement: 0.7 * ln(reactions) + 0.3 * ln(comments),
                stats: format!(
                    "{} reactions, {} comments",
                    short(reactions),
                    short(comments)
                ),
                note: text(&a["description"]),
            }
        })
        .collect()
}

fn tag<'a>(block: &'a str, name: &str) -> &'a str {
    let open = format!("<{name}");
    let Some(start) = block
        .find(&open)
        .and_then(|i| block[i..].find('>').map(|j| i + j + 1))
    else {
        return "";
    };
    let end = block[start..]
        .find(&format!("</{name}>"))
        .map_or(block.len(), |e| start + e);
    &block[start..end]
}

/// arXiv's Atom feed. Papers have no engagement: they rank on relevance and
/// how new they are.
pub fn parse_arxiv(xml: &str) -> Vec<Item> {
    xml.split("<entry>")
        .skip(1)
        .map(|e| Item {
            source: "arxiv",
            title: entities(
                &tag(e, "title")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            url: tag(e, "id").trim().to_string(),
            date: iso_secs(tag(e, "published").trim()),
            engagement: 0.0,
            stats: "new paper".into(),
            note: entities(
                &tag(e, "summary")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            )
            .chars()
            .take(220)
            .collect(),
        })
        .collect()
}

const STOP: [&str; 24] = [
    "the", "a", "an", "and", "or", "of", "for", "to", "in", "on", "with", "what", "how", "is",
    "are", "about", "vs", "do", "does", "people", "say", "think", "new", "best",
];

fn terms(q: &str) -> Vec<String> {
    q.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '+' && c != '#')
        .filter(|t| t.chars().count() >= 2 && !STOP.contains(t))
        .map(str::to_string)
        .collect()
}

/// Share of the topic's words found in the item.
pub fn relevance(topic: &str, item: &Item) -> f64 {
    let t = terms(topic);
    if t.is_empty() {
        return 1.0;
    }
    let hay = format!("{} {}", item.title, item.note).to_lowercase();
    t.iter().filter(|w| hay.contains(w.as_str())).count() as f64 / t.len() as f64
}

/// Dependency bumps and other pull requests opened by bots: nobody said them.
fn automated(title: &str) -> bool {
    let t = title.to_lowercase();
    [
        "update rust crate",
        "update dependency",
        "update module",
        "chore(deps",
        "build(deps",
        "fix(deps",
        "bump ",
        "[snyk]",
        "renovate",
        "dependabot",
    ]
    .iter()
    .any(|p| t.starts_with(p) || (p.len() > 6 && t.contains(p)))
}

/// Off-topic and out-of-window items dropped, then ranked: engagement inside
/// its own source (so a 40-point HN story and a $1M market compare), how
/// much of the topic it is about, and how recent it is.
pub fn rank(topic: &str, mut items: Vec<Item>, since: u64) -> Vec<(f64, Item)> {
    items.retain(|i| {
        !i.title.is_empty() && !automated(&i.title) && (i.date == 0 || i.date >= since)
    });
    let mut seen = std::collections::HashSet::new();
    items.retain(|i| seen.insert(i.url.clone()) && seen.insert(i.title.to_lowercase()));
    let span = now().saturating_sub(since).max(1) as f64;
    let mut out: Vec<(f64, Item)> = items
        .into_iter()
        .filter_map(|i| {
            let rel = relevance(topic, &i);
            let fresh = if i.date == 0 {
                0.5
            } else {
                1.0 - (now().saturating_sub(i.date) as f64 / span).min(1.0)
            };
            (rel >= 0.5).then_some((0.35 * rel + 0.15 * fresh, i))
        })
        .collect();
    // Engagement against the best of the same source.
    for src in SOURCES {
        let best = out
            .iter()
            .filter(|(_, i)| i.source == src)
            .map(|(_, i)| i.engagement)
            .fold(0.0, f64::max);
        for (score, i) in out.iter_mut().filter(|(_, i)| i.source == src) {
            *score += 0.5 * if best > 0.0 { i.engagement / best } else { 0.3 };
        }
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    // No source takes more than 40% of the first 30.
    let mut taken: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let (mut head, mut tail) = (Vec::new(), Vec::new());
    for x in out {
        let n = taken.entry(x.1.source).or_default();
        if head.len() < 30 && *n < 12 {
            *n += 1;
            head.push(x);
        } else {
            tail.push(x);
        }
    }
    head.extend(tail);
    head
}

async fn get_json(req: reqwest::RequestBuilder) -> Result<Value, String> {
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    resp.json().await.map_err(|e| e.to_string())
}

async fn hackernews(c: &reqwest::Client, q: &str, since: u64) -> Result<Vec<Item>, String> {
    let url = format!(
        "https://hn.algolia.com/api/v1/search?query={}&tags=story&numericFilters=created_at_i%3E{since}&hitsPerPage=30",
        percent_encode(q)
    );
    Ok(parse_hackernews(&get_json(c.get(url)).await?))
}

async fn reddit(c: &reqwest::Client, q: &str, days: u64) -> Result<Vec<Item>, String> {
    let (Ok(id), Ok(secret)) = (
        std::env::var("REDDIT_CLIENT_ID"),
        std::env::var("REDDIT_CLIENT_SECRET"),
    ) else {
        return Err("skipped: create a Reddit API app (reddit.com/prefs/apps, type script) and set REDDIT_CLIENT_ID and REDDIT_CLIENT_SECRET".into());
    };
    let token = get_json(
        c.post("https://www.reddit.com/api/v1/access_token")
            .basic_auth(id, Some(secret))
            .form(&[("grant_type", "client_credentials")]),
    )
    .await?;
    let token = token["access_token"]
        .as_str()
        .ok_or("Reddit refused the API app's credentials")?
        .to_string();
    let t = match days {
        0..=1 => "day",
        2..=7 => "week",
        8..=31 => "month",
        _ => "year",
    };
    let url = format!(
        "https://oauth.reddit.com/search?q={}&sort=relevance&t={t}&limit=40&raw_json=1",
        percent_encode(q)
    );
    Ok(parse_reddit(
        &get_json(c.get(url).bearer_auth(token)).await?,
    ))
}

async fn github(c: &reqwest::Client, q: &str, since: u64) -> Result<Vec<Item>, String> {
    let gh = |url: String| {
        let r = c.get(url).header("Accept", "application/vnd.github+json");
        match std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")) {
            Ok(t) => r.bearer_auth(t),
            Err(_) => r,
        }
    };
    let day = ymd(since);
    let issues = format!(
        "https://api.github.com/search/issues?q={}&sort=reactions&order=desc&per_page=20",
        percent_encode(&format!("{q} in:title created:>{day}"))
    );
    let repos = format!(
        "https://api.github.com/search/repositories?q={}&sort=stars&order=desc&per_page=10",
        percent_encode(&format!("{q} created:>{day}"))
    );
    let (a, b) = futures_util::join!(get_json(gh(issues)), get_json(gh(repos)));
    let mut out = a.map(|v| parse_github_issues(&v)).unwrap_or_default();
    out.extend(b.map(|v| parse_github_repos(&v))?);
    Ok(out)
}

async fn polymarket(c: &reqwest::Client, q: &str) -> Result<Vec<Item>, String> {
    let url = format!(
        "https://gamma-api.polymarket.com/public-search?q={}&events_status=active&keep_closed_markets=0",
        percent_encode(q)
    );
    Ok(parse_polymarket(&get_json(c.get(url)).await?))
}

async fn devto(c: &reqwest::Client, q: &str, days: u64) -> Result<Vec<Item>, String> {
    let url = format!(
        "https://dev.to/api/articles/search?q={}&top={days}&per_page=30",
        percent_encode(q)
    );
    Ok(parse_devto(&get_json(c.get(url)).await?))
}

async fn arxiv(c: &reqwest::Client, q: &str) -> Result<Vec<Item>, String> {
    let query = terms(q)
        .iter()
        .map(|t| format!("all:{t}"))
        .collect::<Vec<_>>()
        .join("+AND+");
    let url = format!(
        "https://export.arxiv.org/api/query?search_query={query}&sortBy=submittedDate&sortOrder=descending&max_results=15"
    );
    let resp = c.get(url).send().await.map_err(|e| e.to_string())?;
    Ok(parse_arxiv(&resp.text().await.map_err(|e| e.to_string())?))
}

pub(crate) async fn tool(input: &Value) -> Res {
    let topic = arg(input, "topic")?.trim().to_string();
    let days = input["days"].as_u64().unwrap_or(30).clamp(1, 365);
    let since = now().saturating_sub(days * 86_400);
    let wanted: Vec<String> = input["sources"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let on = |s: &str| wanted.is_empty() || wanted.iter().any(|w| w == s);
    let c = client()?;
    let skip = || async { Err::<Vec<Item>, String>("not asked".into()) };
    let (hn, rd, gh, pm, dv, ax) = futures_util::join!(
        async {
            if on("hackernews") {
                hackernews(&c, &topic, since).await
            } else {
                skip().await
            }
        },
        async {
            if on("reddit") {
                reddit(&c, &topic, days).await
            } else {
                skip().await
            }
        },
        async {
            if on("github") {
                github(&c, &topic, since).await
            } else {
                skip().await
            }
        },
        async {
            if on("polymarket") {
                polymarket(&c, &topic).await
            } else {
                skip().await
            }
        },
        async {
            if on("devto") {
                devto(&c, &topic, days).await
            } else {
                skip().await
            }
        },
        async {
            if on("arxiv") {
                arxiv(&c, &topic).await
            } else {
                skip().await
            }
        },
    );
    let mut all = Vec::new();
    let mut status = Vec::new();
    for (name, r) in [
        ("hackernews", hn),
        ("reddit", rd),
        ("github", gh),
        ("polymarket", pm),
        ("devto", dv),
        ("arxiv", ax),
    ] {
        match r {
            Ok(v) => {
                status.push(format!("{name}: {} found", v.len()));
                all.extend(v);
            }
            Err(e) if e == "not asked" => {}
            Err(e) => status.push(format!("{name}: {e}")),
        }
    }
    let ranked = rank(&topic, all, since);
    let mut out = format!(
        "What people said about \"{topic}\" since {} ({days} days).\nSources: {}.\n",
        ymd(since),
        status.join("; ")
    );
    if ranked.is_empty() {
        out.push_str(
            "\nNothing on topic in this window. Try fewer or broader words, or more days.",
        );
        return Ok(ToolOutput::ok(out));
    }
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for (_, i) in &ranked {
        *counts.entry(i.source).or_default() += 1;
    }
    out.push_str(&format!(
        "On topic: {}.\n",
        counts
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    for (n, (score, i)) in ranked.iter().take(30).enumerate() {
        out.push_str(&format!(
            "\n{}. [{}] {} ({}; {}; score {:.2})\n   {}",
            n + 1,
            i.source,
            i.title,
            ymd(i.date),
            i.stats,
            score,
            i.url
        ));
        if !i.note.is_empty() {
            out.push_str(&format!(
                "\n   {}",
                i.note.chars().take(200).collect::<String>()
            ));
        }
    }
    out.push_str(
        "\n\nWrite the brief from these results only: what people are saying and doing, the strongest signals (most engaged first), where they disagree, \
         and what the money on Polymarket expects. Link every claim to its item. Name the sources that were skipped or failed, and do not fill their gap from memory.",
    );
    Ok(ToolOutput::ok(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dates_round_trip() {
        let t = iso_secs("2026-10-28T12:26:48Z");
        assert_eq!(ymd(t), "2026-10-28");
        assert_eq!(ymd(0), "1970-01-01");
        assert_eq!(iso_secs("2000-03-01"), 951_868_800);
    }

    #[test]
    fn sources_parse_and_rank_by_engagement() {
        let t = now() - 86_400;
        let hn = parse_hackernews(&json!({"hits": [
            {"title": "Rust async is finally good", "url": "https://x.dev/a", "points": 900, "num_comments": 300, "created_at_i": t, "objectID": "1"},
            {"title": "Bump tokio in Rust async app", "points": 1, "num_comments": 0, "created_at_i": t, "objectID": "4"},
            {"title": "Rust async notes", "url": null, "points": 3, "num_comments": 0, "created_at_i": t, "objectID": "2"},
            {"title": "Cooking pasta", "points": 5000, "num_comments": 10, "created_at_i": t, "objectID": "3"}
        ]}));
        assert_eq!(hn[0].url, "https://news.ycombinator.com/item?id=1");
        let pm = parse_polymarket(
            &json!({"events": [{"title": "Will Rust async ship in 2027?", "slug": "rust-async", "volume1mo": 2_500_000.0, "updatedAt": "2026-10-28T00:00:00Z",
            "markets": [{"question": "Ships?", "volume": "10", "outcomes": "[\"Yes\",\"No\"]", "outcomePrices": "[\"0.12\",\"0.88\"]"}]}]}),
        );
        assert!(
            pm[0].stats.contains("$2.5M") && pm[0].stats.contains("Yes 12% / No 88%"),
            "{}",
            pm[0].stats
        );
        let gh = parse_github_issues(
            &json!({"items": [{"title": "Rust async traits stabilised", "html_url": "https://github.com/r/r/issues/1",
            "repository_url": "https://api.github.com/repos/rust-lang/rust", "reactions": {"total_count": 400}, "comments": 80, "created_at": "2026-10-20T00:00:00Z"}]}),
        );
        assert!(gh[0].stats.contains("rust-lang/rust"));
        let ax = parse_arxiv(
            "<feed><entry><id>http://arxiv.org/abs/1</id><published>2026-10-27T00:00:00Z</published><title>Rust async\n  runtimes</title><summary>We study.</summary></entry></feed>",
        );
        assert_eq!(ax[0].title, "Rust async runtimes");

        let mut all = hn;
        all.extend(pm);
        all.extend(gh);
        all.extend(ax);
        let ranked = rank("rust async", all, now() - 30 * 86_400);
        let titles: Vec<&str> = ranked.iter().map(|(_, i)| i.title.as_str()).collect();
        assert!(!titles.contains(&"Cooking pasta"), "off topic is dropped");
        assert!(
            !titles.iter().any(|t| t.starts_with("Bump")),
            "bot bumps are dropped"
        );
        assert_eq!(
            ranked
                .iter()
                .find(|(_, i)| i.source == "hackernews")
                .unwrap()
                .1
                .title,
            "Rust async is finally good"
        );
        let pos = |t: &str| titles.iter().position(|x| *x == t).unwrap();
        assert!(
            pos("Rust async is finally good") < pos("Rust async notes"),
            "{titles:?}"
        );
    }

    /// Hits the real APIs: `cargo test -p coturbo-tools live_research -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_research() {
        let out = tool(&json!({"topic": "rust async", "days": 30}))
            .await
            .unwrap()
            .content;
        println!("{out}");
        assert!(out.contains("hackernews:"));
    }
}
