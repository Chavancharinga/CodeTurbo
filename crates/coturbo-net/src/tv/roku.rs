//! Roku: the External Control Protocol, plain HTTP on 8060.

use super::{Action, http};
use std::net::IpAddr;

fn tag<'a>(xml: &'a str, name: &str) -> &'a str {
    let open = format!("<{name}>");
    xml.find(&open)
        .and_then(|i| {
            let rest = &xml[i + open.len()..];
            rest.find(&format!("</{name}>")).map(|e| &rest[..e])
        })
        .unwrap_or("?")
}

async fn get(ip: IpAddr, path: &str) -> Result<String, String> {
    http()
        .get(format!("http://{ip}:8060/{path}"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())
}

async fn post(ip: IpAddr, path: &str) -> Result<(), String> {
    let r = http()
        .post(format!("http://{ip}:8060/{path}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.status().is_success() {
        Ok(())
    } else {
        Err(format!("the Roku answered {}", r.status()))
    }
}

fn key_name(key: &str) -> String {
    match key
        .trim()
        .to_lowercase()
        .replace([' ', '_', '-'], "")
        .as_str()
    {
        "ok" | "enter" | "select" => "Select".into(),
        "volumeup" => "VolumeUp".into(),
        "volumedown" => "VolumeDown".into(),
        "playpause" | "play" | "pause" => "Play".into(),
        other => {
            let mut c = other.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        }
    }
}

/// `(id, name)` of the installed channels.
async fn apps(ip: IpAddr) -> Result<Vec<(String, String)>, String> {
    let xml = get(ip, "query/apps").await?;
    Ok(xml
        .split("<app ")
        .skip(1)
        .filter_map(|a| {
            let id = a.split("id=\"").nth(1)?.split('"').next()?.to_string();
            let name = a.split('>').nth(1)?.split('<').next()?.to_string();
            Some((id, name))
        })
        .collect())
}

pub(crate) async fn control(ip: IpAddr, action: &Action) -> Result<String, String> {
    async fn press(ip: IpAddr, k: &str, n: u8) -> Result<(), String> {
        for _ in 0..n.max(1) {
            post(ip, &format!("keypress/{k}")).await?;
        }
        Ok(())
    }
    match action {
        Action::Info => {
            let x = get(ip, "query/device-info").await?;
            Ok(format!(
                "{} {} · software {} build {} · power {}",
                tag(&x, "vendor-name"),
                tag(&x, "model-name"),
                tag(&x, "software-version"),
                tag(&x, "software-build"),
                tag(&x, "power-mode")
            ))
        }
        Action::Apps => {
            let list: Vec<String> = apps(ip).await?.into_iter().map(|(_, n)| n).collect();
            Ok(format!("{} channels: {}", list.len(), list.join(", ")))
        }
        Action::Launch { app, content } => {
            let want = super::app_alias(app);
            let id = if want == "youtube" {
                "837".to_string()
            } else {
                apps(ip)
                    .await?
                    .into_iter()
                    .find(|(_, n)| super::app_alias(n).contains(&want))
                    .map(|(id, _)| id)
                    .ok_or_else(|| format!("{app} is not installed"))?
            };
            let query = content
                .as_ref()
                .map(|c| format!("?contentId={}", c.replace(' ', "%20")))
                .unwrap_or_default();
            post(ip, &format!("launch/{id}{query}"))
                .await
                .map(|_| format!("{app} opened"))
        }
        Action::Key(k) => press(ip, &key_name(k), 1)
            .await
            .map(|_| format!("{k} pressed")),
        Action::PowerOff => press(ip, "PowerOff", 1).await.map(|_| "turned off".into()),
        Action::VolumeUp(n) => press(ip, "VolumeUp", *n)
            .await
            .map(|_| format!("volume up {n}")),
        Action::VolumeDown(n) => press(ip, "VolumeDown", *n)
            .await
            .map(|_| format!("volume down {n}")),
        Action::Mute => press(ip, "VolumeMute", 1)
            .await
            .map(|_| "mute toggled".into()),
        Action::SetVolume(_) => Err("Roku only takes volume up and down steps".into()),
        Action::PowerOn { .. } => unreachable!("handled before"),
    }
}
