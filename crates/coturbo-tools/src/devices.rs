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
