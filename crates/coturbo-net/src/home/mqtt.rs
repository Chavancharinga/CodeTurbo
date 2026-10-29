//! A small MQTT 3.1.1 client: publish a command, or listen to a topic for a
//! few seconds. Through a broker this reaches Zigbee (Zigbee2MQTT), and
//! Tasmota, Shelly and ESPHome devices set to MQTT. A login, when the broker
//! wants one, is kept per broker.

use super::Act;
use serde_json::Value;
use std::net::IpAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn remaining(out: &mut Vec<u8>, mut n: usize) {
    loop {
        let mut b = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            return;
        }
    }
}

fn string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn packet(header: u8, body: &[u8]) -> Vec<u8> {
    let mut p = vec![header];
    remaining(&mut p, body.len());
    p.extend_from_slice(body);
    p
}

pub fn connect_packet(client: &str, login: Option<(&str, &str)>) -> Vec<u8> {
    let mut b = Vec::new();
    string(&mut b, "MQTT");
    b.push(4);
    b.push(0x02 | if login.is_some() { 0xC0 } else { 0 });
    b.extend_from_slice(&30u16.to_be_bytes());
    string(&mut b, client);
    if let Some((u, p)) = login {
        string(&mut b, u);
        string(&mut b, p);
    }
    packet(0x10, &b)
}

pub fn publish_packet(topic: &str, payload: &str) -> Vec<u8> {
    let mut b = Vec::new();
    string(&mut b, topic);
    b.extend_from_slice(payload.as_bytes());
    packet(0x30, &b)
}

pub fn subscribe_packet(id: u16, topic: &str) -> Vec<u8> {
    let mut b = id.to_be_bytes().to_vec();
    string(&mut b, topic);
    b.push(0);
    packet(0x82, &b)
}

async fn read_packet(s: &mut TcpStream) -> Result<(u8, Vec<u8>), String> {
    let mut h = [0u8; 1];
    s.read_exact(&mut h).await.map_err(|e| e.to_string())?;
    let (mut len, mut mult) = (0usize, 1usize);
    loop {
        let mut b = [0u8; 1];
        s.read_exact(&mut b).await.map_err(|e| e.to_string())?;
        len += (b[0] & 0x7f) as usize * mult;
        if b[0] & 0x80 == 0 {
            break;
        }
        mult *= 128;
    }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).await.map_err(|e| e.to_string())?;
    Ok((h[0], body))
}

/// `(topic, payload)` of a PUBLISH packet's body.
pub fn parse_publish(header: u8, body: &[u8]) -> Option<(String, String)> {
    let tlen = u16::from_be_bytes([*body.first()?, *body.get(1)?]) as usize;
    let topic = String::from_utf8_lossy(body.get(2..2 + tlen)?).into_owned();
    let mut at = 2 + tlen;
    if (header >> 1) & 3 > 0 {
        at += 2; // packet id
    }
    Some((topic, String::from_utf8_lossy(body.get(at..)?).into_owned()))
}

/// Keeps a login for the broker at `ip`.
pub fn set_login(ip: IpAddr, user: &str, password: &str) {
    crate::tv::save_secret("mqtt", ip, &format!("{user}\n{password}"));
}

async fn session(ip: IpAddr) -> Result<TcpStream, String> {
    let mut s = tokio::time::timeout(Duration::from_secs(4), TcpStream::connect((ip, 1883)))
        .await
        .map_err(|_| "the broker did not answer".to_string())?
        .map_err(|e| e.to_string())?;
    let saved = crate::tv::load_secret("mqtt", ip);
    let login = saved.as_deref().and_then(|l| l.split_once('\n'));
    s.write_all(&connect_packet(&format!("codeturbo-{}", std::process::id()), login)).await.map_err(|e| e.to_string())?;
    let (h, body) = tokio::time::timeout(Duration::from_secs(4), read_packet(&mut s)).await.map_err(|_| "no answer from the broker".to_string())??;
    match (h, body.get(1)) {
        (0x20, Some(0)) => Ok(s),
        (0x20, Some(4 | 5)) => Err("the broker wants a login: give its user and password (they are kept on this computer)".into()),
        (_, code) => Err(format!("the broker refused the connection ({code:?})")),
    }
}

/// Zigbee2MQTT's device list, shortened.
fn summarize(topic: &str, payload: &str) -> String {
    if topic.ends_with("bridge/devices")
        && let Ok(Value::Array(devices)) = serde_json::from_str::<Value>(payload)
    {
        let names: Vec<String> = devices
            .iter()
            .filter(|d| d["type"] != "Coordinator")
            .map(|d| {
                let model = d["definition"]["description"].as_str().unwrap_or("");
                format!("{} ({model})", d["friendly_name"].as_str().unwrap_or("?"))
            })
            .collect();
        return format!("{} Zigbee device(s): {}", names.len(), names.join(", "));
    }
    let short: String = payload.chars().take(300).collect();
    if short.len() < payload.len() { format!("{short}…") } else { short }
}

pub(crate) async fn control(ip: IpAddr, act: &Act) -> Result<String, String> {
    match act {
        Act::Publish { topic, payload } => {
            let mut s = session(ip).await?;
            s.write_all(&publish_packet(topic, payload)).await.map_err(|e| e.to_string())?;
            s.write_all(&[0xE0, 0x00]).await.map_err(|e| e.to_string())?;
            Ok(format!("sent {payload} to {topic}"))
        }
        // Status: the Zigbee2MQTT device list, when there is one.
        Act::Listen { .. } | Act::Status => {
            let topic = match act {
                Act::Listen { topic } => topic.clone(),
                _ => "zigbee2mqtt/bridge/devices".to_string(),
            };
            let mut s = session(ip).await?;
            s.write_all(&subscribe_packet(1, &topic)).await.map_err(|e| e.to_string())?;
            let mut seen = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
            while seen.len() < 40 {
                let Ok(Ok((h, body))) = tokio::time::timeout_at(deadline, read_packet(&mut s)).await else { break };
                if h & 0xF0 == 0x30
                    && let Some((t, p)) = parse_publish(h, &body)
                {
                    seen.push(format!("{t}: {}", summarize(&t, &p)));
                }
            }
            let _ = s.write_all(&[0xE0, 0x00]).await;
            if seen.is_empty() {
                return Ok(format!("nothing arrived on {topic} in 4 seconds"));
            }
            Ok(seen.join("\n"))
        }
        other => Err(format!("an MQTT broker takes publish or listen, not {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_have_the_mqtt_shape() {
        let c = connect_packet("cli", None);
        assert_eq!(c[0], 0x10);
        assert_eq!(&c[2..8], &[0, 4, b'M', b'Q', b'T', b'T']);
        let p = publish_packet("zigbee2mqtt/lamp/set", r#"{"state":"ON"}"#);
        assert_eq!(p[0], 0x30);
        assert_eq!(parse_publish(p[0], &p[2..]).unwrap(), ("zigbee2mqtt/lamp/set".into(), r#"{"state":"ON"}"#.into()));
        let mut long = Vec::new();
        remaining(&mut long, 321);
        assert_eq!(long, vec![0xC1, 0x02]);
    }

    #[test]
    fn zigbee_device_lists_are_summarised() {
        let list = r#"[{"type":"Coordinator","friendly_name":"Coordinator"},{"type":"EndDevice","friendly_name":"sala lamp","definition":{"description":"Smart bulb"}}]"#;
        assert_eq!(summarize("zigbee2mqtt/bridge/devices", list), "1 Zigbee device(s): sala lamp (Smart bulb)");
    }
}
