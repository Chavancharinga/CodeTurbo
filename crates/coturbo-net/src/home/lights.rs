//! Lights, plugs and switches, each in its own local protocol.

use super::{Act, http};
use serde_json::{Value, json};
use std::net::IpAddr;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

fn unsupported(what: &str, act: &Act) -> String {
    format!("{what} does not take {act:?}")
}

async fn get_json(url: String) -> Result<Value, String> {
    http()
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- Hue

/// Philips Hue bridge, API v1. The first time, the bridge's round button
/// must be pressed; the key it gives is kept.
pub(crate) async fn hue(ip: IpAddr, act: &Act, target: Option<&str>) -> Result<String, String> {
    let user = match crate::tv::load_secret("hue", ip) {
        Some(u) => u,
        None => {
            let r: Value = http()
                .post(format!("http://{ip}/api"))
                .json(&json!({"devicetype": "codeturbo#cli"}))
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            match r[0]["success"]["username"].as_str() {
                Some(u) => {
                    crate::tv::save_secret("hue", ip, u);
                    u.to_string()
                }
                None => return Err("press the round link button on the Hue bridge, then try again within 30 seconds".into()),
            }
        }
    };
    let base = format!("http://{ip}/api/{user}");
    let lights = get_json(format!("{base}/lights")).await?;
    let names: Vec<(String, String, bool, u64)> = lights
        .as_object()
        .into_iter()
        .flatten()
        .map(|(id, l)| {
            (
                id.clone(),
                l["name"].as_str().unwrap_or("?").to_string(),
                l["state"]["on"].as_bool().unwrap_or(false),
                l["state"]["bri"].as_u64().unwrap_or(0),
            )
        })
        .collect();
    if *act == Act::Status {
        let list: Vec<String> = names
            .iter()
            .map(|(_, n, on, bri)| {
                format!(
                    "{n}: {}",
                    if *on {
                        format!("on {}%", bri * 100 / 254)
                    } else {
                        "off".into()
                    }
                )
            })
            .collect();
        return Ok(format!("{} light(s): {}", list.len(), list.join(", ")));
    }
    let body = match act {
        Act::On => json!({"on": true}),
        Act::Off => json!({"on": false}),
        Act::Toggle => json!({"on": !names.iter().any(|(_, _, on, _)| *on)}),
        Act::Brightness(p) => json!({"on": *p > 0, "bri": (u64::from(*p) * 254 / 100).max(1)}),
        Act::Color(r, g, b) => {
            let (x, y) = rgb_to_xy(*r, *g, *b);
            json!({"on": true, "xy": [x, y]})
        }
        other => return Err(unsupported("a Hue light", other)),
    };
    // One light by name, or all of them (group 0).
    let url = match target {
        Some(t) => {
            let want = t.to_lowercase();
            let id = names
                .iter()
                .find(|(_, n, ..)| n.to_lowercase().contains(&want))
                .map(|(id, ..)| id.clone())
                .ok_or_else(|| format!("no Hue light called {t}"))?;
            format!("{base}/lights/{id}/state")
        }
        None => format!("{base}/groups/0/action"),
    };
    http()
        .put(url)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "{} {}",
        target.unwrap_or("all lights"),
        describe(act)
    ))
}

/// sRGB to the CIE xy the Hue API takes.
pub fn rgb_to_xy(r: u8, g: u8, b: u8) -> (f64, f64) {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c > 0.04045 {
            ((c + 0.055) / 1.055).powf(2.4)
        } else {
            c / 12.92
        }
    };
    let (r, g, b) = (lin(r), lin(g), lin(b));
    let x = r * 0.664_511 + g * 0.154_324 + b * 0.162_028;
    let y = r * 0.283_881 + g * 0.668_433 + b * 0.047_685;
    let z = r * 0.000_088 + g * 0.072_310 + b * 0.986_039;
    let sum = x + y + z;
    if sum == 0.0 {
        return (0.3227, 0.329);
    }
    (
        (x / sum * 10000.0).round() / 10000.0,
        (y / sum * 10000.0).round() / 10000.0,
    )
}

fn describe(act: &Act) -> String {
    match act {
        Act::On => "on".into(),
        Act::Off => "off".into(),
        Act::Toggle => "toggled".into(),
        Act::Brightness(p) => format!("at {p}%"),
        Act::Color(r, g, b) => format!("set to #{r:02x}{g:02x}{b:02x}"),
        other => format!("{other:?}"),
    }
}

// ---------------------------------------------------------------- Shelly

/// Shelly, Gen1 (`/relay/0`, `/light/0`) and Gen2+ (RPC). `target` is the
/// relay or channel number.
pub(crate) async fn shelly(
    ip: IpAddr,
    gen2: bool,
    act: &Act,
    target: Option<&str>,
) -> Result<String, String> {
    let id: u32 = target.and_then(|t| t.trim().parse().ok()).unwrap_or(0);
    if gen2 {
        let status = get_json(format!("http://{ip}/rpc/Shelly.GetStatus")).await?;
        let light = status.get(format!("light:{id}")).is_some();
        let comp = if light { "Light" } else { "Switch" };
        let q = match act {
            Act::Status => {
                let part = &status[format!("{}:{id}", comp.to_lowercase())];
                let power = part["apower"]
                    .as_f64()
                    .map(|w| format!(", {w} W"))
                    .unwrap_or_default();
                return Ok(format!(
                    "{comp} {id} {}{power}",
                    if part["output"].as_bool() == Some(true) {
                        "on"
                    } else {
                        "off"
                    }
                ));
            }
            Act::On => format!("{comp}.Set?id={id}&on=true"),
            Act::Off => format!("{comp}.Set?id={id}&on=false"),
            Act::Toggle => format!("{comp}.Toggle?id={id}"),
            Act::Brightness(p) if light => format!("Light.Set?id={id}&on=true&brightness={p}"),
            other => return Err(unsupported("this Shelly", other)),
        };
        http()
            .get(format!("http://{ip}/rpc/{q}"))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        return Ok(format!("{comp} {id} {}", describe(act)));
    }
    let status = get_json(format!("http://{ip}/status")).await?;
    let light = status["lights"].as_array().is_some_and(|l| !l.is_empty());
    let path = if light { "light" } else { "relay" };
    let q = match act {
        Act::Status => {
            let part = &status[format!("{path}s")][id as usize];
            let power = status["meters"][id as usize]["power"]
                .as_f64()
                .map(|w| format!(", {w} W"))
                .unwrap_or_default();
            return Ok(format!(
                "{path} {id} {}{power}",
                if part["ison"].as_bool() == Some(true) {
                    "on"
                } else {
                    "off"
                }
            ));
        }
        Act::On => "turn=on".to_string(),
        Act::Off => "turn=off".to_string(),
        Act::Toggle => "turn=toggle".to_string(),
        Act::Brightness(p) if light => format!("turn=on&brightness={p}"),
        other => return Err(unsupported("this Shelly", other)),
    };
    http()
        .get(format!("http://{ip}/{path}/{id}?{q}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!("{path} {id} {}", describe(act)))
}

// ---------------------------------------------------------------- Tasmota

pub(crate) async fn tasmota(ip: IpAddr, act: &Act) -> Result<String, String> {
    let cmnd = match act {
        Act::Status => "Status 0".to_string(),
        Act::On => "Power On".into(),
        Act::Off => "Power Off".into(),
        Act::Toggle => "Power Toggle".into(),
        Act::Brightness(p) => format!("Dimmer {p}"),
        Act::Color(r, g, b) => format!("Color {r:02X}{g:02X}{b:02X}"),
        other => return Err(unsupported("Tasmota", other)),
    };
    let v = get_json(format!("http://{ip}/cm?cmnd={}", cmnd.replace(' ', "%20"))).await?;
    if *act == Act::Status {
        let name = v["Status"]["DeviceName"]
            .as_str()
            .or(v["Status"]["FriendlyName"][0].as_str())
            .unwrap_or("Tasmota");
        let power = v["StatusSTS"]["POWER"].as_str().unwrap_or("?");
        let watts = v["StatusSNS"]["ENERGY"]["Power"]
            .as_f64()
            .map(|w| format!(", {w} W"))
            .unwrap_or_default();
        return Ok(format!("{name}: {power}{watts}"));
    }
    Ok(describe(act))
}

// ---------------------------------------------------------------- WLED

pub(crate) async fn wled(ip: IpAddr, act: &Act) -> Result<String, String> {
    let body = match act {
        Act::Status => {
            let (info, state) = (
                get_json(format!("http://{ip}/json/info")).await?,
                get_json(format!("http://{ip}/json/state")).await?,
            );
            return Ok(format!(
                "{} · {} · brightness {}%",
                info["name"].as_str().unwrap_or("WLED"),
                if state["on"].as_bool() == Some(true) {
                    "on"
                } else {
                    "off"
                },
                state["bri"].as_u64().unwrap_or(0) * 100 / 255
            ));
        }
        Act::On => json!({"on": true}),
        Act::Off => json!({"on": false}),
        Act::Toggle => json!({"on": "t"}),
        Act::Brightness(p) => json!({"on": *p > 0, "bri": u64::from(*p) * 255 / 100}),
        Act::Color(r, g, b) => json!({"on": true, "seg": [{"col": [[r, g, b]]}]}),
        other => return Err(unsupported("WLED", other)),
    };
    http()
        .post(format!("http://{ip}/json/state"))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(describe(act))
}

// ---------------------------------------------------------------- Kasa

/// TP-Link Kasa's "autokey" cipher: each byte XORed with the previous
/// cipher byte, starting from 171.
pub fn kasa_cipher(plain: &[u8]) -> Vec<u8> {
    let mut key = 171u8;
    plain
        .iter()
        .map(|&b| {
            key ^= b;
            key
        })
        .collect()
}

pub fn kasa_decipher(cipher: &[u8]) -> Vec<u8> {
    let mut key = 171u8;
    cipher
        .iter()
        .map(|&c| {
            let p = key ^ c;
            key = c;
            p
        })
        .collect()
}

async fn kasa_send(ip: IpAddr, request: &Value) -> Result<Value, String> {
    let mut s = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::net::TcpStream::connect((ip, 9999)),
    )
    .await
    .map_err(|_| "the plug did not answer".to_string())?
    .map_err(|e| e.to_string())?;
    let body = kasa_cipher(request.to_string().as_bytes());
    let mut msg = (body.len() as u32).to_be_bytes().to_vec();
    msg.extend(body);
    s.write_all(&msg).await.map_err(|e| e.to_string())?;
    let mut len = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(4), s.read_exact(&mut len))
        .await
        .map_err(|_| "no answer".to_string())?
        .map_err(|e| e.to_string())?;
    let mut reply = vec![0u8; u32::from_be_bytes(len) as usize];
    s.read_exact(&mut reply).await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&kasa_decipher(&reply)).map_err(|e| e.to_string())
}

pub(crate) async fn kasa(ip: IpAddr, act: &Act) -> Result<String, String> {
    let info = kasa_send(ip, &json!({"system": {"get_sysinfo": {}}})).await?;
    let sys = &info["system"]["get_sysinfo"];
    let bulb = sys.get("light_state").is_some();
    const LIGHT: &str = "smartlife.iot.smartbulb.lightingservice";
    let light = |state: Value| json!({LIGHT: {"transition_light_state": state}});
    let request = match act {
        Act::Status => {
            let on = if bulb {
                sys["light_state"]["on_off"].as_u64() == Some(1)
            } else {
                sys["relay_state"].as_u64() == Some(1)
            };
            return Ok(format!(
                "{} ({}) · {}",
                sys["alias"].as_str().unwrap_or("Kasa"),
                sys["model"].as_str().unwrap_or("?"),
                if on { "on" } else { "off" }
            ));
        }
        Act::On | Act::Off | Act::Toggle => {
            let now = if bulb {
                sys["light_state"]["on_off"].as_u64() == Some(1)
            } else {
                sys["relay_state"].as_u64() == Some(1)
            };
            let want = match act {
                Act::On => true,
                Act::Off => false,
                _ => !now,
            };
            if bulb {
                light(json!({"on_off": u8::from(want)}))
            } else {
                json!({"system": {"set_relay_state": {"state": u8::from(want)}}})
            }
        }
        Act::Brightness(p) if bulb => light(json!({"on_off": 1, "brightness": p})),
        other => return Err(unsupported("this Kasa device", other)),
    };
    kasa_send(ip, &request).await?;
    Ok(describe(act))
}

// ---------------------------------------------------------------- Yeelight

/// Yeelight bulbs on TCP 55443 ("LAN control" on in the Yeelight app).
pub(crate) async fn yeelight(ip: IpAddr, act: &Act) -> Result<String, String> {
    let (method, params) = match act {
        Act::Status => ("get_prop", json!(["power", "bright", "rgb", "name"])),
        Act::On => ("set_power", json!(["on", "smooth", 300])),
        Act::Off => ("set_power", json!(["off", "smooth", 300])),
        Act::Toggle => ("toggle", json!([])),
        Act::Brightness(p) => ("set_bright", json!([(*p).clamp(1, 100), "smooth", 300])),
        Act::Color(r, g, b) => (
            "set_rgb",
            json!([
                (u32::from(*r) << 16) | (u32::from(*g) << 8) | u32::from(*b),
                "smooth",
                300
            ]),
        ),
        other => return Err(unsupported("a Yeelight", other)),
    };
    let s = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::net::TcpStream::connect((ip, 55443)),
    )
    .await
    .map_err(|_| "the bulb did not answer".to_string())?
    .map_err(|e| format!("{e}: turn on LAN control in the Yeelight app"))?;
    let (read, mut write) = s.into_split();
    write
        .write_all(
            format!(
                "{}\r\n",
                json!({"id": 1, "method": method, "params": params})
            )
            .as_bytes(),
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut lines = tokio::io::BufReader::new(read).lines();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while let Ok(Ok(Some(line))) = tokio::time::timeout_at(deadline, lines.next_line()).await {
        let v: Value = serde_json::from_str(&line).unwrap_or_default();
        if v["id"] != 1 {
            continue;
        }
        if let Some(e) = v["error"]["message"].as_str() {
            return Err(format!("the bulb refused: {e}"));
        }
        if *act == Act::Status {
            let r = &v["result"];
            return Ok(format!(
                "{} · {} · brightness {}%",
                r[3].as_str()
                    .filter(|n| !n.is_empty())
                    .unwrap_or("Yeelight"),
                r[0].as_str().unwrap_or("?"),
                r[1].as_str().unwrap_or("?")
            ));
        }
        return Ok(describe(act));
    }
    Err("the bulb did not confirm".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kasa_cipher_round_trips() {
        let plain = br#"{"system":{"get_sysinfo":{}}}"#;
        let c = kasa_cipher(plain);
        assert_eq!(c[0], 171 ^ b'{');
        assert_eq!(kasa_decipher(&c), plain);
    }

    #[test]
    fn colours_become_hue_xy() {
        let (x, y) = rgb_to_xy(255, 0, 0);
        assert!(
            (x - 0.7006).abs() < 0.01 && (y - 0.2993).abs() < 0.01,
            "{x} {y}"
        );
        assert_eq!(rgb_to_xy(0, 0, 0), (0.3227, 0.329));
    }
}
