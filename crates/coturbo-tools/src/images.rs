//! `find_images`: real photos of the subject for a page, with free licences,
//! from Wikimedia Commons. Each one is saved in the project with its credit.

use crate::web::{client, entities, percent_encode};
use crate::{Res, ToolOutput, arg};
use serde_json::Value;
use std::path::Path;

/// Width the photos are downloaded at: sharp on a laptop, light on a phone.
const WIDTH: u32 = 1600;
/// Smaller originals are thumbnails and icons, not photos.
const MIN_SIDE: u64 = 900;
const MAX_BYTES: usize = 3 * 1024 * 1024;

/// Text with the HTML Commons puts in its metadata removed.
fn plain(html: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in html.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    entities(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn slug(s: &str) -> String {
    let s: String = s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    s.split('-').filter(|p| !p.is_empty()).take(5).collect::<Vec<_>>().join("-")
}

/// One photo as Commons describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub url: String,
    pub width: u64,
    pub height: u64,
    pub alt: String,
    pub author: String,
    pub licence: String,
    pub page: String,
}

/// The photos in a Commons search answer: large raster images with a free
/// licence, the best first.
pub fn parse(answer: &Value) -> Vec<Found> {
    let mut pages: Vec<&Value> = answer["query"]["pages"].as_object().map(|m| m.values().collect()).unwrap_or_default();
    pages.sort_by_key(|p| p["index"].as_u64().unwrap_or(u64::MAX));
    pages
        .into_iter()
        .filter_map(|p| {
            let i = &p["imageinfo"][0];
            let mime = i["mime"].as_str()?;
            let (w, h) = (i["width"].as_u64()?, i["height"].as_u64()?);
            if !matches!(mime, "image/jpeg" | "image/png" | "image/webp") || w.min(h) < MIN_SIDE {
                return None;
            }
            let meta = &i["extmetadata"];
            let field = |k: &str| meta[k]["value"].as_str().map(plain).unwrap_or_default();
            let licence = field("LicenseShortName");
            let lower = licence.to_lowercase();
            if !(lower.contains("cc") || lower.contains("public domain") || lower.contains("pd")) || lower.contains("-nc") || lower.contains("-nd") {
                return None;
            }
            let title = p["title"].as_str().unwrap_or("").trim_start_matches("File:");
            let title = title.rsplit_once('.').map_or(title, |(t, _)| t).replace('_', " ");
            let mut alt = field("ImageDescription");
            if alt.is_empty() || alt.len() > 160 {
                alt = title;
            }
            Some(Found {
                url: i["thumburl"].as_str().or(i["url"].as_str())?.to_string(),
                width: i["thumbwidth"].as_u64().unwrap_or(w),
                height: i["thumbheight"].as_u64().unwrap_or(h),
                alt,
                author: field("Artist"),
                licence,
                page: i["descriptionurl"].as_str().unwrap_or("").to_string(),
            })
        })
        .collect()
}

pub(crate) async fn find(root: &Path, input: &Value) -> Res {
    let query = arg(input, "query")?;
    let count = input["count"].as_u64().unwrap_or(8).clamp(1, 12) as usize;
    let http = client()?;
    let url = format!(
        "https://commons.wikimedia.org/w/api.php?action=query&format=json&generator=search&gsrnamespace=6&gsrlimit=40\
         &gsrsearch={}&prop=imageinfo&iiprop=url|size|mime|extmetadata&iiurlwidth={WIDTH}",
        percent_encode(&format!("{query} filetype:bitmap"))
    );
    let answer: Value = http.get(&url).send().await.map_err(|e| format!("Wikimedia Commons: {e}"))?.json().await.map_err(|e| format!("Wikimedia Commons: {e}"))?;
    let found = parse(&answer);
    if found.is_empty() {
        return Ok(ToolOutput::ok(format!("no free photo found for \"{query}\"; try other words (in English, the subject in two or three words)")));
    }
    let dir = root.join("assets/images");
    std::fs::create_dir_all(&dir).map_err(|e| format!("assets/images: {e}"))?;
    let base = slug(query);
    let mut saved = Vec::new();
    let mut credits = String::new();
    for f in found {
        if saved.len() >= count {
            break;
        }
        let Ok(resp) = http.get(&f.url).send().await else { continue };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(bytes) = resp.bytes().await else { continue };
        if bytes.len() > MAX_BYTES || bytes.len() < 10_000 {
            continue;
        }
        let ext = if f.url.to_lowercase().contains(".png") { "png" } else if f.url.to_lowercase().contains(".webp") { "webp" } else { "jpg" };
        let name = format!("{base}-{}.{ext}", saved.len() + 1);
        if std::fs::write(dir.join(&name), &bytes).is_err() {
            continue;
        }
        let by = if f.author.is_empty() { String::new() } else { format!(" by {}", f.author) };
        credits.push_str(&format!("- {name}: {}{by}, {} ({})\n", f.alt, f.licence, f.page));
        saved.push(format!("assets/images/{name} {}x{} ({}) alt: \"{}\"{by}", f.width, f.height, if f.width >= f.height { "landscape" } else { "portrait" }, f.alt));
    }
    if saved.is_empty() {
        return Ok(ToolOutput::ok(format!("photos for \"{query}\" were found but none could be downloaded; try again or other words")));
    }
    let path = dir.join("CREDITS.md");
    let mut all = std::fs::read_to_string(&path).unwrap_or_else(|_| "# Image credits\n\nFrom Wikimedia Commons, under the licences named.\n\n".into());
    all.push_str(&credits);
    std::fs::write(&path, all).map_err(|e| format!("assets/images/CREDITS.md: {e}"))?;
    Ok(ToolOutput::ok(format!(
        "saved {} photo(s); credits in assets/images/CREDITS.md. Use them large (the opening, a gallery or bento tiles, beside the teacher or the product) \
         with object-fit: cover and aspect-ratio, write the alt in the page's language from what each shows, and credit CC BY / BY-SA authors \
         in the footer (\"Photos: author, licence\").\n{}",
        saved.len(),
        saved.join("\n")
    )))
}
