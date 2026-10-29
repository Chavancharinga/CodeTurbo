//! `network_scan` and `device_control`: the devices on the user's own
//! network, found and driven natively (see `coturbo-net`).

use crate::{Res, ToolOutput, arg};
use serde_json::Value;
use std::time::Duration;

pub(crate) async fn network_scan(input: &Value) -> Res {
    let sweep = input["sweep"].as_bool().unwrap_or(false);
    let seconds = input["seconds"].as_u64().unwrap_or(3).clamp(1, 10);
    let inv = coturbo_net::scan(coturbo_net::ScanOptions {
        listen: Duration::from_secs(seconds),
        sweep,
    })
    .await;
    Ok(ToolOutput::ok(coturbo_net::render(&inv)))
}

fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str().map_or_else(|| v.to_string(), str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Any device: TVs, speakers, lights, plugs, printers, routers, MQTT.
pub(crate) async fn device(input: &Value) -> Res {
    use coturbo_net::home::{Act, parse_color};
    // An address, or a device known from an earlier scan by its name or MAC.
    let target = input["device"]
        .as_str()
        .ok_or("missing `device` (its name or address)")?;
    let (ip, mac) = coturbo_net::registry::resolve(target)
        .ok_or_else(|| format!("no known device `{target}`: run network_scan to find it (TVs must be on), then name it with name_device"))?;
    let value = input["value"].as_u64();
    let steps = value.unwrap_or(1).clamp(1, 50) as u8;
    let text = |k: &str| input[k].as_str().map(str::to_string);
    if let (Some(user), Some(password)) = (text("user"), text("password")) {
        coturbo_net::home::mqtt::set_login(ip, &user, &password);
    }
    let act = match arg(input, "action")? {
        "status" => Act::Status,
        "on" => Act::On,
        "off" => Act::Off,
        "toggle" => Act::Toggle,
        "brightness" => {
            Act::Brightness(value.ok_or("brightness needs `value` (0-100)")?.min(100) as u8)
        }
        "color" => {
            let c = text("color").ok_or("color needs `color` (#rrggbb or a colour name)")?;
            let (r, g, b) = parse_color(&c)
                .ok_or_else(|| format!("`{c}` is not a colour (#rrggbb or red, blue, warm...)"))?;
            Act::Color(r, g, b)
        }
        "volume" => Act::SetVolume(value.ok_or("volume needs `value` (0-100)")?.min(100) as u8),
        "volume_up" => Act::VolumeUp(steps),
        "volume_down" => Act::VolumeDown(steps),
        "mute" => Act::Mute,
        "play" => Act::Play,
        "pause" => Act::Pause,
        "stop" => Act::Stop,
        "next" => Act::Next,
        "previous" => Act::Previous,
        "launch" => Act::Launch {
            app: arg(input, "app")?.to_string(),
            content: text("content"),
        },
        "key" => Act::Key(arg(input, "key")?.to_string()),
        "apps" => Act::Apps,
        "publish" => Act::Publish {
            topic: arg(input, "topic")?.to_string(),
            payload: text("payload").unwrap_or_default(),
        },
        "listen" => Act::Listen {
            topic: text("topic").unwrap_or_else(|| "#".into()),
        },
        a => return Err(format!("unknown action `{a}`")),
    };
    Ok(ToolOutput::ok(
        coturbo_net::home::control(ip, mac, act, input["target"].as_str()).await?,
    ))
}

pub(crate) async fn device_control(input: &Value) -> Res {
    let out = match arg(input, "kind")? {
        "http" => {
            let method = input["method"].as_str().unwrap_or("GET");
            coturbo_net::http(
                method,
                arg(input, "url")?,
                &pairs(&input["headers"]),
                input["body"].as_str(),
            )
            .await?
        }
        "upnp" => {
            coturbo_net::upnp_action(
                arg(input, "control_url")?,
                arg(input, "service_type")?,
                arg(input, "action")?,
                &pairs(&input["args"]),
            )
            .await?
        }
        "wake" => {
            let broadcast = match input["broadcast"].as_str() {
                Some(b) => Some(
                    b.parse()
                        .map_err(|_| format!("{b} is not an IPv4 address"))?,
                ),
                None => None,
            };
            coturbo_net::wake(arg(input, "mac")?, broadcast).await?
        }
        k => return Err(format!("unknown kind `{k}`: use http, upnp or wake")),
    };
    Ok(ToolOutput::ok(out))
}

pub(crate) fn name_device(input: &Value) -> Res {
    let k = coturbo_net::registry::name(arg(input, "device")?, arg(input, "name")?)?;
    Ok(ToolOutput::ok(format!(
        "{} ({}, mac {}) is now called \"{}\"",
        k.ip,
        if k.what.is_empty() { "device" } else { &k.what },
        k.mac,
        k.name
    )))
}
