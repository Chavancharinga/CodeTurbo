//! DIAL: the "open this app" protocol behind YouTube's cast button. Almost
//! every smart TV has it (Samsung, LG, Android TV, Fire TV, Roku), with no
//! pairing.

use super::http;
use std::net::IpAddr;

/// Where each family keeps its DIAL apps.
const BASES: &[(u16, &str)] = &[(8008, "/apps/"), (8080, "/ws/apps/"), (36866, "/apps/"), (8060, "/dial/"), (60000, "/apps/"), (56789, "/apps/")];

/// DIAL app names for common apps.
fn dial_name(app: &str) -> String {
    match super::app_alias(app).as_str() {
        "youtube" => "YouTube".into(),
        "netflix" => "Netflix".into(),
        "primevideo" | "prime" | "amazonprimevideo" => "AmazonInstantVideo".into(),
        "spotify" => "com.spotify.Spotify.TVv2".into(),
        _ => app.trim().to_string(),
    }
}

/// The base URL that answers for `app` on this TV.
async fn find(ip: IpAddr, app: &str) -> Option<String> {
    for (port, path) in BASES {
        let base = format!("http://{ip}:{port}{path}");
        if let Ok(r) = http().get(format!("{base}{app}")).send().await
            && r.status().is_success()
        {
            return Some(base);
        }
    }
    None
}

/// Opens `app` (with optional content, e.g. a YouTube video id or search).
pub(crate) async fn launch(ip: IpAddr, app: &str, content: Option<&str>) -> Result<String, String> {
    let name = dial_name(app);
    let base = find(ip, &name).await.ok_or_else(|| format!("this TV does not offer {name} over DIAL"))?;
    let body = match content {
        Some(c) if name == "YouTube" => {
            let id = c.rsplit(['=', '/']).next().unwrap_or(c);
            format!("v={id}")
        }
        Some(c) => c.to_string(),
        None => String::new(),
    };
    let r = http().post(format!("{base}{name}")).header("Content-Type", "text/plain; charset=utf-8").body(body).send().await.map_err(|e| e.to_string())?;
    if r.status().is_success() {
        Ok(format!("{name} opened"))
    } else {
        Err(format!("the TV answered {} to opening {name}", r.status()))
    }
}

/// Which common apps this TV can open over DIAL.
pub(crate) async fn info(ip: IpAddr) -> Result<String, String> {
    let mut can = Vec::new();
    for app in ["YouTube", "Netflix"] {
        if find(ip, app).await.is_some() {
            can.push(app);
        }
    }
    if can.is_empty() {
        return Err("no DIAL apps answered".into());
    }
    Ok(format!("can open over DIAL: {}", can.join(", ")))
}
