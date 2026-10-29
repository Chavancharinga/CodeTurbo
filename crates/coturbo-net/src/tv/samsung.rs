//! Samsung Tizen TVs: device info over HTTP (8001), the remote over a
//! WebSocket (wss 8002). The first connection shows "Allow" on the TV; the
//! token it gives is kept so it does not ask again.

use super::{Action, Socket, http, load_secret, save_secret, websocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::net::IpAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

pub(crate) async fn info(ip: IpAddr) -> Result<Value, String> {
    let v: Value = http()
        .get(format!("http://{ip}:8001/api/v2/"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if v["device"].is_object() {
        Ok(v["device"].clone())
    } else {
        Err("not a Samsung TV".into())
    }
}

/// Well-known Tizen app ids.
fn app_id(name: &str) -> Option<&'static str> {
    Some(match super::app_alias(name).as_str() {
        "youtube" => "111299001912",
        "netflix" => "3201907018807",
        "primevideo" | "prime" | "amazonprimevideo" => "3201910019365",
        "disney" | "disneyplus" => "3201901017640",
        "spotify" => "3201606009684",
        "hbomax" | "max" => "3202301029760",
        "appletv" => "3201807016597",
        "twitch" => "3202203026841",
        "browser" | "internet" => "org.tizen.browser",
        _ => return None,
    })
}

/// Samsung key names for the generic ones.
pub(crate) fn key_code(key: &str) -> String {
    let k = key.trim().to_uppercase().replace([' ', '-'], "_");
    let k = k.strip_prefix("KEY_").unwrap_or(&k).to_string();
    let mapped = match k.as_str() {
        "OK" | "SELECT" => "ENTER",
        "BACK" => "RETURN",
        "PLAY_PAUSE" => "PLAY",
        "VOLUME_UP" => "VOLUP",
        "VOLUME_DOWN" => "VOLDOWN",
        "CHANNEL_UP" => "CHUP",
        "CHANNEL_DOWN" => "CHDOWN",
        "SETTINGS" => "MENU",
        other => other,
    };
    format!("KEY_{mapped}")
}

fn remote_key(code: &str) -> Message {
    Message::Text(
        json!({"method": "ms.remote.control", "params": {"Cmd": "Click", "DataOfCmd": code, "Option": "false", "TypeOfRemote": "SendRemoteKey"}})
            .to_string()
            .into(),
    )
}

/// Connects to the remote; the first time, the TV asks the user to allow it.
async fn connect(ip: IpAddr) -> Result<Socket, String> {
    use base64::Engine;
    let name = base64::engine::general_purpose::STANDARD.encode("CodeTurbo");
    let token = load_secret("samsung", ip)
        .map(|t| format!("&token={t}"))
        .unwrap_or_default();
    let url = format!("wss://{ip}:8002/api/v2/channels/samsung.remote.control?name={name}{token}");
    let mut ws = match websocket(&url).await {
        Ok(ws) => ws,
        // Older models only have the plain socket.
        Err(_) => {
            websocket(&format!(
                "ws://{ip}:8001/api/v2/channels/samsung.remote.control?name={name}"
            ))
            .await?
        }
    };
    // Up to 30 s for someone to press Allow on the TV.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next())
            .await
            .map_err(|_| {
                "the TV did not answer: accept \"CodeTurbo\" on the TV screen and try again"
                    .to_string()
            })?
            .ok_or("the TV closed the connection")?
            .map_err(|e| e.to_string())?;
        let Message::Text(text) = msg else { continue };
        let v: Value = serde_json::from_str(&text).unwrap_or_default();
        match v["event"].as_str() {
            Some("ms.channel.connect") => {
                if let Some(t) = v["data"]["token"].as_str() {
                    save_secret("samsung", ip, t);
                }
                return Ok(ws);
            }
            Some("ms.channel.unauthorized") => {
                return Err(
                    "the TV refused: allow \"CodeTurbo\" in the TV's Device Connection Manager"
                        .into(),
                );
            }
            _ => {}
        }
    }
}

async fn press(ip: IpAddr, codes: &[String]) -> Result<(), String> {
    let mut ws = connect(ip).await?;
    for code in codes {
        ws.send(remote_key(code)).await.map_err(|e| e.to_string())?;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let _ = ws.close(None).await;
    Ok(())
}

async fn installed(ip: IpAddr) -> Result<Vec<(String, String)>, String> {
    let mut ws = connect(ip).await?;
    let ask = json!({"method": "ms.channel.emit", "params": {"event": "ed.installedApp.get", "to": "host"}});
    ws.send(Message::Text(ask.to_string().into()))
        .await
        .map_err(|e| e.to_string())?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while let Ok(Some(Ok(msg))) = tokio::time::timeout_at(deadline, ws.next()).await {
        let Message::Text(text) = msg else { continue };
        let v: Value = serde_json::from_str(&text).unwrap_or_default();
        if v["event"] == "ed.installedApp.get" {
            let _ = ws.close(None).await;
            return Ok(v["data"]["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| {
                    Some((
                        a["name"].as_str()?.to_string(),
                        a["appId"].as_str()?.to_string(),
                    ))
                })
                .collect());
        }
    }
    Err("the TV did not send its app list".into())
}

async fn launch(ip: IpAddr, app: &str) -> Result<String, String> {
    let id = match app_id(app) {
        Some(id) => id.to_string(),
        None => {
            let want = super::app_alias(app);
            installed(ip)
                .await?
                .into_iter()
                .find(|(name, id)| super::app_alias(name).contains(&want) || *id == app)
                .map(|(_, id)| id)
                .ok_or_else(|| format!("{app} is not installed on this TV"))?
        }
    };
    // The REST call works on most models; the remote socket on the rest.
    let rest = http()
        .post(format!("http://{ip}:8001/api/v2/applications/{id}"))
        .send()
        .await;
    if rest.as_ref().is_ok_and(|r| r.status().is_success()) {
        return Ok(format!("{app} opened"));
    }
    let mut ws = connect(ip).await?;
    let msg = json!({"method": "ms.channel.emit", "params": {"event": "ed.apps.launch", "to": "host", "data": {"appId": id, "action_type": "DEEP_LINK"}}});
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .map_err(|e| e.to_string())?;
    let _ = ws.close(None).await;
    Ok(format!("{app} opened"))
}

pub(crate) async fn control(ip: IpAddr, action: &Action) -> Result<String, String> {
    match action {
        Action::Info => {
            let d = info(ip).await?;
            let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
            Ok(format!(
                "{} · model {} · {} · power {} · MAC {} · firmware {}",
                s("name"),
                s("modelName"),
                s("OS"),
                s("PowerState"),
                s("wifiMac"),
                d["firmwareVersion"]
                    .as_str()
                    .or(d["FirmwareVersion"].as_str())
                    .unwrap_or("not reported"),
            ))
        }
        Action::Apps => {
            let apps = installed(ip).await?;
            let list: Vec<String> = apps.iter().map(|(n, _)| n.clone()).collect();
            Ok(format!("{} apps: {}", list.len(), list.join(", ")))
        }
        Action::Launch { app, .. } => launch(ip, app).await,
        Action::Key(k) => press(ip, &[key_code(k)])
            .await
            .map(|_| format!("{} pressed", key_code(k))),
        Action::PowerOff => press(ip, &["KEY_POWER".into()])
            .await
            .map(|_| "turned off".into()),
        Action::VolumeUp(n) => press(ip, &vec!["KEY_VOLUP".to_string(); (*n).max(1) as usize])
            .await
            .map(|_| format!("volume up {n}")),
        Action::VolumeDown(n) => press(ip, &vec!["KEY_VOLDOWN".to_string(); (*n).max(1) as usize])
            .await
            .map(|_| format!("volume down {n}")),
        Action::Mute => press(ip, &["KEY_MUTE".into()])
            .await
            .map(|_| "mute toggled".into()),
        Action::SetVolume(_) => {
            Err("Samsung TVs only take volume up and down steps over the network".into())
        }
        Action::PowerOn { .. } => unreachable!("handled before"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_keys_become_samsung_keys() {
        assert_eq!(key_code("ok"), "KEY_ENTER");
        assert_eq!(key_code("back"), "KEY_RETURN");
        assert_eq!(key_code("volume up"), "KEY_VOLUP");
        assert_eq!(key_code("KEY_HOME"), "KEY_HOME");
        assert_eq!(app_id("YouTube"), Some("111299001912"));
        assert_eq!(app_id("Disney+"), Some("3201901017640"));
    }
}
