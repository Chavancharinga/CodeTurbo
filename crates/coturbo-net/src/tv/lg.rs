//! LG webOS: the SSAP WebSocket (wss 3001, or ws 3000 on older models).
//! The first time, the TV asks to accept "CodeTurbo"; the client key it
//! gives is kept.

use super::{Action, Socket, load_secret, save_secret, websocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::net::IpAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const PERMISSIONS: &[&str] = &[
    "LAUNCH",
    "LAUNCH_WEBAPP",
    "APP_TO_APP",
    "CONTROL_AUDIO",
    "CONTROL_DISPLAY",
    "CONTROL_INPUT_JOYSTICK",
    "CONTROL_INPUT_MEDIA_PLAYBACK",
    "CONTROL_INPUT_TV",
    "CONTROL_POWER",
    "READ_APP_STATUS",
    "READ_CURRENT_CHANNEL",
    "READ_INPUT_DEVICE_LIST",
    "READ_INSTALLED_APPS",
    "READ_NETWORK_STATE",
    "READ_RUNNING_APPS",
    "READ_TV_CHANNEL_LIST",
    "READ_UPDATE_INFO",
    "READ_POWER_STATE",
    "CONTROL_TV_SCREEN",
    "CONTROL_MOUSE_AND_KEYBOARD",
    "READ_LGE_SDX",
    "READ_NOTIFICATIONS",
    "WRITE_NOTIFICATION_TOAST",
];

async fn connect(ip: IpAddr) -> Result<Socket, String> {
    let mut ws = match websocket(&format!("wss://{ip}:3001")).await {
        Ok(ws) => ws,
        Err(_) => websocket(&format!("ws://{ip}:3000")).await?,
    };
    let mut payload = json!({
        "forcePairing": false,
        "pairingType": "PROMPT",
        "manifest": {"manifestVersion": 1, "appVersion": "1.1", "permissions": PERMISSIONS,
                     "signed": {"appId": "com.codeturbo.remote", "vendorId": "com.codeturbo", "created": "20261016",
                                "localizedAppNames": {"": "CodeTurbo"}, "localizedVendorNames": {"": "CodeTurbo"},
                                "permissions": PERMISSIONS, "serial": "codeturbo"}}
    });
    if let Some(k) = load_secret("lg", ip) {
        payload["client-key"] = Value::String(k);
    }
    let reg = json!({"type": "register", "id": "register_0", "payload": payload});
    ws.send(Message::Text(reg.to_string().into()))
        .await
        .map_err(|e| e.to_string())?;
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
        match v["type"].as_str() {
            Some("registered") => {
                if let Some(k) = v["payload"]["client-key"].as_str() {
                    save_secret("lg", ip, k);
                }
                return Ok(ws);
            }
            Some("error") => {
                return Err(format!(
                    "the TV refused: {}",
                    v["error"].as_str().unwrap_or("not allowed")
                ));
            }
            _ => {}
        }
    }
}

/// One SSAP request; returns its payload.
async fn request(ip: IpAddr, uri: &str, payload: Value) -> Result<Value, String> {
    let mut ws = connect(ip).await?;
    let req = json!({"type": "request", "id": "req_1", "uri": uri, "payload": payload});
    ws.send(Message::Text(req.to_string().into()))
        .await
        .map_err(|e| e.to_string())?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while let Ok(Some(Ok(msg))) = tokio::time::timeout_at(deadline, ws.next()).await {
        let Message::Text(text) = msg else { continue };
        let v: Value = serde_json::from_str(&text).unwrap_or_default();
        if v["id"] == "req_1" {
            let _ = ws.close(None).await;
            if v["type"] == "error" {
                return Err(v["error"].as_str().unwrap_or("error").to_string());
            }
            return Ok(v["payload"].clone());
        }
    }
    Err("the TV did not answer the request".into())
}

fn app_id(app: &str) -> String {
    match super::app_alias(app).as_str() {
        "youtube" => "youtube.leanback.v4".into(),
        "netflix" => "netflix".into(),
        "primevideo" | "prime" | "amazonprimevideo" => "amazon".into(),
        "disney" | "disneyplus" => "com.disney.disneyplus-prod".into(),
        "spotify" => "spotify-beehive".into(),
        "browser" | "internet" => "com.webos.app.browser".into(),
        _ => app.trim().to_string(),
    }
}

pub(crate) async fn control(ip: IpAddr, action: &Action) -> Result<String, String> {
    match action {
        Action::Info => {
            let sw = request(
                ip,
                "ssap://com.webos.service.update/getCurrentSWInformation",
                json!({}),
            )
            .await?;
            let s = |k: &str| sw[k].as_str().unwrap_or("?").to_string();
            Ok(format!(
                "model {} · webOS {} · firmware {}",
                s("model_name"),
                s("product_name"),
                s("major_ver") + "." + &s("minor_ver")
            ))
        }
        Action::Apps => {
            let p = request(
                ip,
                "ssap://com.webos.applicationManager/listLaunchPoints",
                json!({}),
            )
            .await?;
            let names: Vec<String> = p["launchPoints"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a["title"].as_str().map(str::to_string))
                .collect();
            Ok(format!("{} apps: {}", names.len(), names.join(", ")))
        }
        Action::Launch { app, content } => {
            let mut payload = json!({"id": app_id(app)});
            if let Some(c) = content {
                payload["contentId"] = Value::String(c.clone());
            }
            request(ip, "ssap://system.launcher/launch", payload)
                .await
                .map(|_| format!("{app} opened"))
        }
        Action::PowerOff => request(ip, "ssap://system/turnOff", json!({}))
            .await
            .map(|_| "turned off".into()),
        Action::VolumeUp(n) => {
            for _ in 0..(*n).max(1) {
                request(ip, "ssap://audio/volumeUp", json!({})).await?;
            }
            Ok(format!("volume up {n}"))
        }
        Action::VolumeDown(n) => {
            for _ in 0..(*n).max(1) {
                request(ip, "ssap://audio/volumeDown", json!({})).await?;
            }
            Ok(format!("volume down {n}"))
        }
        Action::SetVolume(v) => request(ip, "ssap://audio/setVolume", json!({"volume": v}))
            .await
            .map(|_| format!("volume set to {v}")),
        Action::Mute => {
            let status = request(ip, "ssap://audio/getStatus", json!({})).await?;
            let muted = status["mute"].as_bool().unwrap_or(false);
            request(ip, "ssap://audio/setMute", json!({"mute": !muted}))
                .await
                .map(|_| {
                    if muted {
                        "unmuted".into()
                    } else {
                        "muted".into()
                    }
                })
        }
        Action::Key(k) => {
            let uri = match k.to_lowercase().as_str() {
                "play" => "ssap://media.controls/play",
                "pause" => "ssap://media.controls/pause",
                "stop" => "ssap://media.controls/stop",
                "channel_up" | "channel up" => "ssap://tv/channelUp",
                "channel_down" | "channel down" => "ssap://tv/channelDown",
                _ => {
                    return Err(format!(
                        "the LG remote here takes play, pause, stop and channel up/down; `{k}` needs the pointer socket"
                    ));
                }
            };
            request(ip, uri, json!({}))
                .await
                .map(|_| format!("{k} pressed"))
        }
        Action::PowerOn { .. } => unreachable!("handled before"),
    }
}
