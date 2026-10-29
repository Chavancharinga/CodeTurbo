//! Google Cast (port 8009): what Android TV, Google TV and Chromecast
//! answer with no setup on the TV. Volume and mute, opening and closing
//! apps, play and pause, and what is playing. TLS with the device's own
//! certificate, length-prefixed protobuf `CastMessage`s carrying JSON.

use super::Action;
use serde_json::{Value, json};
use std::net::IpAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type Stream = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;

const CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
const HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
const RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
const MEDIA: &str = "urn:x-cast:com.google.cast.media";

fn varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn field_str(out: &mut Vec<u8>, field: u8, s: &str) {
    out.push(field << 3 | 2);
    varint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

/// A `CastMessage` (protocol v2.1, string payload), with its length prefix.
pub fn encode(source: &str, destination: &str, namespace: &str, payload: &str) -> Vec<u8> {
    let mut m = vec![0x08, 0x00];
    field_str(&mut m, 2, source);
    field_str(&mut m, 3, destination);
    field_str(&mut m, 4, namespace);
    m.extend_from_slice(&[0x28, 0x00]);
    field_str(&mut m, 6, payload);
    let mut framed = (m.len() as u32).to_be_bytes().to_vec();
    framed.extend(m);
    framed
}

/// `(namespace, payload)` of a `CastMessage` body (no length prefix).
pub fn decode(m: &[u8]) -> Option<(String, String)> {
    let (mut ns, mut payload) = (String::new(), String::new());
    let mut i = 0;
    let read_varint = |i: &mut usize| -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *m.get(*i)?;
            *i += 1;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    };
    while i < m.len() {
        let key = read_varint(&mut i)?;
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                read_varint(&mut i)?;
            }
            2 => {
                let len = read_varint(&mut i)? as usize;
                let bytes = m.get(i..i + len)?;
                i += len;
                match field {
                    4 => ns = String::from_utf8_lossy(bytes).into_owned(),
                    6 => payload = String::from_utf8_lossy(bytes).into_owned(),
                    _ => {}
                }
            }
            _ => return None,
        }
    }
    Some((ns, payload))
}

struct Session {
    stream: Stream,
    next_id: u64,
}

impl Session {
    async fn open(ip: IpAddr) -> Result<Session, String> {
        let tcp = tokio::time::timeout(Duration::from_secs(4), tokio::net::TcpStream::connect((ip, 8009)))
            .await
            .map_err(|_| "the TV did not answer on the Cast port".to_string())?
            .map_err(|e| format!("no Google Cast on {ip} ({e})"))?;
        let name = rustls::pki_types::ServerName::IpAddress(ip.into());
        let stream = tokio_rustls::TlsConnector::from(super::lan_tls()).connect(name, tcp).await.map_err(|e| e.to_string())?;
        let mut s = Session { stream, next_id: 1 };
        s.send("receiver-0", CONNECTION, json!({"type": "CONNECT"})).await?;
        Ok(s)
    }

    async fn send(&mut self, to: &str, ns: &str, payload: Value) -> Result<(), String> {
        self.stream.write_all(&encode("sender-0", to, ns, &payload.to_string())).await.map_err(|e| e.to_string())
    }

    async fn read(&mut self) -> Result<(String, Value), String> {
        let mut len = [0u8; 4];
        self.stream.read_exact(&mut len).await.map_err(|e| e.to_string())?;
        let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
        self.stream.read_exact(&mut body).await.map_err(|e| e.to_string())?;
        let (ns, payload) = decode(&body).ok_or("unreadable Cast message")?;
        Ok((ns, serde_json::from_str(&payload).unwrap_or(Value::Null)))
    }

    /// Sends a request and waits for the answer carrying its request id.
    async fn request(&mut self, to: &str, ns: &str, mut payload: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        payload["requestId"] = json!(id);
        self.send(to, ns, payload).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            let (ns, v) = tokio::time::timeout_at(deadline, self.read()).await.map_err(|_| "the TV did not answer".to_string())??;
            if ns == HEARTBEAT && v["type"] == "PING" {
                self.send("receiver-0", HEARTBEAT, json!({"type": "PONG"})).await?;
                continue;
            }
            if v["requestId"].as_u64() == Some(id) {
                if v["type"] == "INVALID_REQUEST" || v["type"] == "LAUNCH_ERROR" {
                    return Err(format!("the TV refused: {}", v["reason"].as_str().unwrap_or("invalid request")));
                }
                return Ok(v);
            }
        }
    }

    async fn status(&mut self) -> Result<Value, String> {
        Ok(self.request("receiver-0", RECEIVER, json!({"type": "GET_STATUS"})).await?["status"].clone())
    }

    /// Play or pause what the running app is playing.
    async fn media(&mut self, command: &str) -> Result<(), String> {
        let status = self.status().await?;
        let app = status["applications"].get(0).ok_or("nothing is playing")?;
        let transport = app["transportId"].as_str().ok_or("the app has no media channel")?.to_string();
        self.send(&transport, CONNECTION, json!({"type": "CONNECT"})).await?;
        let m = self.request(&transport, MEDIA, json!({"type": "GET_STATUS"})).await?;
        let session = m["status"].get(0).and_then(|s| s["mediaSessionId"].as_u64()).ok_or("nothing is playing in the app")?;
        self.request(&transport, MEDIA, json!({"type": command, "mediaSessionId": session})).await.map(|_| ())
    }
}

/// Cast app ids of common apps.
fn app_id(app: &str) -> Option<&'static str> {
    Some(match super::app_alias(app).as_str() {
        "youtube" => "233637DE",
        "netflix" => "CA5E8412",
        "spotify" => "CC32E753",
        "backdrop" | "ambient" => "E8C28D3C",
        _ => return None,
    })
}

fn describe(status: &Value) -> String {
    let vol = &status["volume"];
    let level = vol["level"].as_f64().map(|l| format!("volume {}%", (l * 100.0).round())).unwrap_or_default();
    let muted = if vol["muted"].as_bool() == Some(true) { " (muted)" } else { "" };
    let app = status["applications"].get(0).map(|a| {
        let name = a["displayName"].as_str().unwrap_or("an app");
        match a["statusText"].as_str().filter(|t| !t.is_empty() && *t != name) {
            Some(t) => format!("showing {name}: {t}"),
            None => format!("showing {name}"),
        }
    });
    format!("{level}{muted} · {}", app.unwrap_or_else(|| "nothing open".into()))
}

pub(crate) async fn control(ip: IpAddr, action: &Action) -> Result<String, String> {
    let mut s = Session::open(ip).await?;
    let set_volume = |level: f64| json!({"type": "SET_VOLUME", "volume": {"level": level.clamp(0.0, 1.0)}});
    match action {
        Action::Info => {
            let eureka: Value = match super::http().get(format!("http://{ip}:8008/setup/eureka_info")).send().await {
                Ok(r) => r.json().await.unwrap_or(Value::Null),
                Err(_) => Value::Null,
            };
            let name = eureka["name"].as_str().map(|n| format!("\"{n}\" · ")).unwrap_or_default();
            Ok(format!("{name}{}", describe(&s.status().await?)))
        }
        Action::VolumeUp(n) | Action::VolumeDown(n) => {
            let now = s.status().await?["volume"]["level"].as_f64().unwrap_or(0.5);
            let step = 0.05 * f64::from((*n).max(1));
            let level = if matches!(action, Action::VolumeUp(_)) { now + step } else { now - step };
            s.request("receiver-0", RECEIVER, set_volume(level)).await?;
            Ok(format!("volume {}%", (level.clamp(0.0, 1.0) * 100.0).round()))
        }
        Action::SetVolume(v) => {
            s.request("receiver-0", RECEIVER, set_volume(f64::from(*v) / 100.0)).await?;
            Ok(format!("volume set to {v}%"))
        }
        Action::Mute => {
            let muted = s.status().await?["volume"]["muted"].as_bool().unwrap_or(false);
            s.request("receiver-0", RECEIVER, json!({"type": "SET_VOLUME", "volume": {"muted": !muted}})).await?;
            Ok(if muted { "unmuted".into() } else { "muted".into() })
        }
        Action::Launch { app, content } => {
            // A YouTube video or search goes through DIAL, which takes one.
            if content.is_some() || app_id(app).is_none() {
                return super::dial::launch(ip, app, content.as_deref()).await;
            }
            let id = app_id(app).expect("checked above");
            s.request("receiver-0", RECEIVER, json!({"type": "LAUNCH", "appId": id})).await?;
            Ok(format!("{app} opened"))
        }
        Action::Key(k) => match k.to_lowercase().replace([' ', '-'], "_").as_str() {
            "play" => s.media("PLAY").await.map(|_| "playing".into()),
            "pause" => s.media("PAUSE").await.map(|_| "paused".into()),
            "play_pause" => {
                let playing = s.media("PAUSE").await;
                match playing {
                    Ok(()) => Ok("paused".into()),
                    Err(_) => s.media("PLAY").await.map(|_| "playing".into()),
                }
            }
            "stop" | "home" | "back" => {
                let status = s.status().await?;
                let session = status["applications"].get(0).and_then(|a| a["sessionId"].as_str()).ok_or("nothing is open")?.to_string();
                s.request("receiver-0", RECEIVER, json!({"type": "STOP", "sessionId": session})).await?;
                Ok("app closed".into())
            }
            other => Err(format!(
                "over Google Cast this TV takes play, pause, stop, volume and opening apps; `{other}` needs ADB (Developer options > network debugging on the TV)"
            )),
        },
        Action::Apps => Err("Google Cast does not list installed apps; with ADB on (Developer options > network debugging) the full list is available".into()),
        Action::PowerOff => Err(
            "Google Cast cannot turn the TV off. Turn on Developer options > network debugging (ADB) on the TV for power, or use the TV's own Google Home routine."
                .into(),
        ),
        Action::PowerOn { .. } => unreachable!("handled before"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let framed = encode("sender-0", "receiver-0", RECEIVER, r#"{"type":"GET_STATUS","requestId":1}"#);
        let len = u32::from_be_bytes(framed[..4].try_into().unwrap()) as usize;
        assert_eq!(len, framed.len() - 4);
        let (ns, payload) = decode(&framed[4..]).unwrap();
        assert_eq!(ns, RECEIVER);
        assert_eq!(payload, r#"{"type":"GET_STATUS","requestId":1}"#);
    }

    #[test]
    fn long_payloads_use_multi_byte_lengths() {
        let big = "x".repeat(300);
        let framed = encode("a", "b", MEDIA, &big);
        assert_eq!(decode(&framed[4..]).unwrap().1, big);
    }

    #[test]
    fn describes_what_the_tv_is_doing() {
        let st = json!({"volume": {"level": 0.35, "muted": false}, "applications": [{"displayName": "YouTube", "statusText": "Lo-fi beats"}]});
        assert_eq!(describe(&st), "volume 35% · showing YouTube: Lo-fi beats");
        assert_eq!(app_id("YouTube"), Some("233637DE"));
    }
}
