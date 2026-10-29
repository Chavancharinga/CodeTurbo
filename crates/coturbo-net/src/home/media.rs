//! UPnP/DLNA media renderers (Sonos, most speakers, receivers and smart
//! TVs) and UPnP internet gateways (routers).

use super::Act;

fn service<'a>(controls: &'a [(String, String)], name: &str) -> Option<(&'a str, &'a str)> {
    controls.iter().find(|(t, _)| t.contains(name)).map(|(t, u)| (t.as_str(), u.as_str()))
}

fn args(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The text of `<tag>` in a SOAP answer.
pub fn soap_value(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(xml[start..end].trim().to_string())
}

async fn call(controls: &[(String, String)], service_name: &str, action: &str, a: &[(&str, &str)]) -> Result<String, String> {
    let (kind, url) = service(controls, service_name).ok_or_else(|| format!("this player has no {service_name} service"))?;
    crate::upnp_action(url, kind, action, &args(a)).await
}

pub(crate) async fn renderer(controls: &[(String, String)], act: &Act) -> Result<String, String> {
    let instance = [("InstanceID", "0")];
    let volume_now = || async {
        let r = call(controls, "RenderingControl", "GetVolume", &[("InstanceID", "0"), ("Channel", "Master")]).await?;
        soap_value(&r, "CurrentVolume").and_then(|v| v.parse::<i64>().ok()).ok_or_else(|| "the player did not say its volume".to_string())
    };
    match act {
        Act::Status => {
            let state = call(controls, "AVTransport", "GetTransportInfo", &instance).await.ok().and_then(|r| soap_value(&r, "CurrentTransportState"));
            let media = call(controls, "AVTransport", "GetPositionInfo", &instance).await.ok();
            let title = media.as_deref().and_then(|r| soap_value(r, "TrackMetaData")).and_then(|m| {
                let m = m.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
                soap_value(&m, "dc:title")
            });
            let vol = volume_now().await.map(|v| format!(" · volume {v}")).unwrap_or_default();
            Ok(format!(
                "{}{}{vol}",
                state.map(|s| s.to_lowercase().replace('_', " ")).unwrap_or_else(|| "state unknown".into()),
                title.map(|t| format!(": {t}")).unwrap_or_default()
            ))
        }
        Act::Play => call(controls, "AVTransport", "Play", &[("InstanceID", "0"), ("Speed", "1")]).await.map(|_| "playing".into()),
        Act::Pause => call(controls, "AVTransport", "Pause", &instance).await.map(|_| "paused".into()),
        Act::Stop => call(controls, "AVTransport", "Stop", &instance).await.map(|_| "stopped".into()),
        Act::Next => call(controls, "AVTransport", "Next", &instance).await.map(|_| "next track".into()),
        Act::Previous => call(controls, "AVTransport", "Previous", &instance).await.map(|_| "previous track".into()),
        Act::SetVolume(v) => {
            let v = v.to_string();
            call(controls, "RenderingControl", "SetVolume", &[("InstanceID", "0"), ("Channel", "Master"), ("DesiredVolume", &v)]).await.map(|_| format!("volume {v}"))
        }
        Act::VolumeUp(n) | Act::VolumeDown(n) => {
            let step = i64::from((*n).max(1)) * 3;
            let now = volume_now().await?;
            let v = if matches!(act, Act::VolumeUp(_)) { now + step } else { now - step }.clamp(0, 100).to_string();
            call(controls, "RenderingControl", "SetVolume", &[("InstanceID", "0"), ("Channel", "Master"), ("DesiredVolume", &v)]).await.map(|_| format!("volume {v}"))
        }
        Act::Mute => {
            let r = call(controls, "RenderingControl", "GetMute", &[("InstanceID", "0"), ("Channel", "Master")]).await?;
            let muted = soap_value(&r, "CurrentMute").is_some_and(|m| m == "1" || m.eq_ignore_ascii_case("true"));
            let want = if muted { "0" } else { "1" };
            call(controls, "RenderingControl", "SetMute", &[("InstanceID", "0"), ("Channel", "Master"), ("DesiredMute", want)])
                .await
                .map(|_| if muted { "unmuted".into() } else { "muted".into() })
        }
        Act::On | Act::Off | Act::Toggle => Err("a UPnP player cannot be switched on or off over the network; use its own app or Wake-on-LAN".into()),
        other => Err(format!("a UPnP player does not take {other:?}")),
    }
}

pub(crate) async fn router(controls: &[(String, String)], act: &Act) -> Result<String, String> {
    if *act != Act::Status {
        return Err("only the router's status is read here; change its settings on its own web page".into());
    }
    let wan = if service(controls, "WANIPConnection").is_some() { "WANIPConnection" } else { "WANPPPConnection" };
    let ip = call(controls, wan, "GetExternalIPAddress", &[]).await.ok().and_then(|r| soap_value(&r, "NewExternalIPAddress"));
    let status = call(controls, wan, "GetStatusInfo", &[]).await.ok();
    let link = status.as_deref().and_then(|r| soap_value(r, "NewConnectionStatus"));
    let uptime = status.as_deref().and_then(|r| soap_value(r, "NewUptime")).and_then(|s| s.parse::<u64>().ok());
    if ip.is_none() && link.is_none() {
        return Err("the router does not share its status over UPnP".into());
    }
    Ok(format!(
        "internet {} · public address {}{}",
        link.unwrap_or_else(|| "?".into()).to_lowercase(),
        ip.unwrap_or_else(|| "?".into()),
        uptime.map(|s| format!(" · up for {}h{:02}m", s / 3600, s % 3600 / 60)).unwrap_or_default()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_values_from_soap_answers() {
        let xml = "<s:Envelope><s:Body><u:GetVolumeResponse><CurrentVolume>23</CurrentVolume></u:GetVolumeResponse></s:Body></s:Envelope>";
        assert_eq!(soap_value(xml, "CurrentVolume").as_deref(), Some("23"));
        assert_eq!(soap_value(xml, "Missing"), None);
    }
}
