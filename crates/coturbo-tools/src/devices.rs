//! `network_scan` and `device_control`: the devices on the user's own
//! network, found and driven natively (see `coturbo-net`).

use crate::{Res, ToolOutput, arg};
use serde_json::Value;
use std::time::Duration;

pub(crate) async fn network_scan(input: &Value) -> Res {
    let sweep = input["sweep"].as_bool().unwrap_or(false);
    let seconds = input["seconds"].as_u64().unwrap_or(3).clamp(1, 10);
    let inv = coturbo_net::scan(coturbo_net::ScanOptions { listen: Duration::from_secs(seconds), sweep }).await;
    Ok(ToolOutput::ok(coturbo_net::render(&inv)))
}

fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_object()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect())
        .unwrap_or_default()
}

pub(crate) async fn tv(input: &Value) -> Res {
    use coturbo_net::tv::Action;
    let ip: std::net::IpAddr = arg(input, "ip")?.parse().map_err(|_| "`ip` must be the TV's address, e.g. 192.168.1.40".to_string())?;
    let count = input["value"].as_u64().unwrap_or(1).clamp(1, 50) as u8;
    let action = match arg(input, "action")? {
        "info" => Action::Info,
        "apps" => Action::Apps,
        "launch" => Action::Launch { app: arg(input, "app")?.to_string(), content: input["content"].as_str().map(str::to_string) },
        "key" => Action::Key(arg(input, "key")?.to_string()),
        "power_off" => Action::PowerOff,
        "power_on" => Action::PowerOn { mac: input["mac"].as_str().map(str::to_string) },
        "volume_up" => Action::VolumeUp(count),
        "volume_down" => Action::VolumeDown(count),
        "set_volume" => Action::SetVolume(input["value"].as_u64().ok_or("set_volume needs `value` (0-100)")?.min(100) as u8),
        "mute" => Action::Mute,
        a => return Err(format!("unknown action `{a}`")),
    };
    Ok(ToolOutput::ok(coturbo_net::tv::control(ip, action).await?))
}

pub(crate) async fn device_control(input: &Value) -> Res {
    let out = match arg(input, "kind")? {
        "http" => {
            let method = input["method"].as_str().unwrap_or("GET");
            coturbo_net::http(method, arg(input, "url")?, &pairs(&input["headers"]), input["body"].as_str()).await?
        }
        "upnp" => coturbo_net::upnp_action(arg(input, "control_url")?, arg(input, "service_type")?, arg(input, "action")?, &pairs(&input["args"])).await?,
        "wake" => {
            let broadcast = match input["broadcast"].as_str() {
                Some(b) => Some(b.parse().map_err(|_| format!("{b} is not an IPv4 address"))?),
                None => None,
            };
            coturbo_net::wake(arg(input, "mac")?, broadcast).await?
        }
        k => return Err(format!("unknown kind `{k}`: use http, upnp or wake")),
    };
    Ok(ToolOutput::ok(out))
}
