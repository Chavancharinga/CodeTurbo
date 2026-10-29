//! Every kind of device on the local network, controlled locally the way
//! Home Assistant's local integrations do, with no cloud and no hub: each
//! device is recognised by what it answers, and spoken to in its own
//! protocol.
//!
//! - Lights, plugs, switches: Philips Hue, Shelly (Gen1/Gen2), Tasmota,
//!   WLED, TP-Link Kasa, Yeelight.
//! - Speakers and players: Google Cast (Nest, Chromecast), UPnP/DLNA
//!   renderers (Sonos, most speakers and receivers).
//! - Printers: IPP status and ink/toner levels.
//! - Routers: UPnP internet gateway status.
//! - Zigbee and the rest through MQTT (Zigbee2MQTT, Tasmota, Shelly...).
//! - TVs: [`crate::tv`].

mod ipp;
mod lights;
mod media;
pub mod mqtt;

use crate::Device;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;

/// One thing to do, for any kind of device. Which ones a device takes
/// depends on what it is; the others are refused with a reason.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    Status,
    On,
    Off,
    Toggle,
    /// Percent, 0-100.
    Brightness(u8),
    Color(u8, u8, u8),
    SetVolume(u8),
    VolumeUp(u8),
    VolumeDown(u8),
    Mute,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    Launch {
        app: String,
        content: Option<String>,
    },
    Key(String),
    Apps,
    /// MQTT: send `payload` to `topic`.
    Publish {
        topic: String,
        payload: String,
    },
    /// MQTT: what arrives on `topic` (wildcards allowed) for a few seconds.
    Listen {
        topic: String,
    },
}

/// The protocol a device answers in.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Tv(crate::tv::Brand),
    /// Google Cast speaker or display (no screen remote).
    Cast,
    Hue,
    Shelly {
        gen2: bool,
    },
    Tasmota,
    Wled,
    Kasa,
    Yeelight,
    /// UPnP/DLNA media renderer: `(service type, control URL)` of its services.
    Renderer(Vec<(String, String)>),
    Printer,
    /// UPnP internet gateway: its services.
    Router(Vec<(String, String)>),
    Mqtt,
    Unknown,
}

impl Kind {
    pub fn label(&self) -> &'static str {
        match self {
            Kind::Tv(_) => "TV",
            Kind::Cast => "Google Cast speaker/display",
            Kind::Hue => "Philips Hue bridge",
            Kind::Shelly { .. } => "Shelly",
            Kind::Tasmota => "Tasmota",
            Kind::Wled => "WLED light",
            Kind::Kasa => "TP-Link Kasa",
            Kind::Yeelight => "Yeelight",
            Kind::Renderer(_) => "UPnP/DLNA player",
            Kind::Printer => "printer",
            Kind::Router(_) => "router",
            Kind::Mqtt => "MQTT broker",
            Kind::Unknown => "unknown device",
        }
    }

    /// The actions this kind takes, for menus and for the model.
    pub fn actions(&self) -> &'static [&'static str] {
        match self {
            Kind::Tv(_) => &[
                "status", "on", "off", "volume", "mute", "play", "pause", "launch", "key", "apps",
            ],
            Kind::Cast => &[
                "status", "volume", "mute", "play", "pause", "stop", "launch",
            ],
            Kind::Hue | Kind::Wled | Kind::Yeelight => {
                &["status", "on", "off", "toggle", "brightness", "color"]
            }
            Kind::Shelly { .. } | Kind::Tasmota | Kind::Kasa => {
                &["status", "on", "off", "toggle", "brightness"]
            }
            Kind::Renderer(_) => &[
                "status", "play", "pause", "stop", "next", "previous", "volume", "mute",
            ],
            Kind::Printer | Kind::Router(_) => &["status"],
            Kind::Mqtt => &["publish", "listen"],
            Kind::Unknown => &["status"],
        }
    }
}

/// The devices of the last scan, to know a device's UPnP services without
/// scanning again.
static LAST: Mutex<Vec<Device>> = Mutex::new(Vec::new());

pub(crate) fn remember_scan(devices: &[Device]) {
    if let Ok(mut l) = LAST.lock() {
        *l = devices.to_vec();
    }
}

fn last_seen(ip: IpAddr) -> Option<Device> {
    LAST.lock().ok()?.iter().find(|d| d.ip == Some(ip)).cloned()
}

pub(crate) fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap_or_default()
}

async fn json_at(url: String) -> Option<serde_json::Value> {
    let r = http().get(url).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.json().await.ok()
}

async fn open(ip: IpAddr, port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(700),
            tokio::net::TcpStream::connect((ip, port))
        )
        .await,
        Ok(Ok(_))
    )
}

/// Finds out what protocol the device at `ip` speaks.
pub async fn detect(ip: IpAddr) -> Kind {
    let seen = last_seen(ip);
    let upnp = seen.as_ref().map(|d| d.upnp.clone()).unwrap_or_default();
    let services: Vec<String> = seen
        .as_ref()
        .map(|d| d.services.iter().map(|s| s.kind.clone()).collect())
        .unwrap_or_default();
    let (shelly, tasmota, wled, hue, kasa, yeelight, mqtt, ipp) = tokio::join!(
        json_at(format!("http://{ip}/shelly")),
        json_at(format!("http://{ip}/cm?cmnd=Status")),
        json_at(format!("http://{ip}/json/info")),
        json_at(format!("http://{ip}/api/config")),
        open(ip, 9999),
        open(ip, 55443),
        open(ip, 1883),
        open(ip, 631),
    );
    if let Some(s) = shelly.filter(|s| s.get("type").is_some() || s.get("gen").is_some()) {
        return Kind::Shelly {
            gen2: s["gen"].as_u64().unwrap_or(1) >= 2,
        };
    }
    if tasmota.is_some_and(|t| t.get("Status").is_some()) {
        return Kind::Tasmota;
    }
    if wled.is_some_and(|w| w.get("leds").is_some()) {
        return Kind::Wled;
    }
    if hue.is_some_and(|h| h.get("bridgeid").is_some()) {
        return Kind::Hue;
    }
    if yeelight {
        return Kind::Yeelight;
    }
    if kasa {
        return Kind::Kasa;
    }
    // TVs and Cast devices.
    let tv = crate::tv::detect(ip).await;
    match tv {
        crate::tv::Brand::Unknown => {}
        crate::tv::Brand::Cast => {
            // A Cast device with no screen is a speaker; with one, a TV.
            let tvish = seen
                .as_ref()
                .is_some_and(|d| d.identity.kind.contains("TV"))
                || services.iter().any(|s| s.contains("androidtvremote"));
            return if tvish { Kind::Tv(tv) } else { Kind::Cast };
        }
        b => return Kind::Tv(b),
    }
    let controls: Vec<(String, String)> = upnp.iter().flat_map(|u| u.controls.clone()).collect();
    if controls
        .iter()
        .any(|(t, _)| t.contains("AVTransport") || t.contains("RenderingControl"))
    {
        return Kind::Renderer(controls);
    }
    if controls.iter().any(|(t, _)| {
        t.contains("WANIPConnection")
            || t.contains("WANPPPConnection")
            || t.contains("WANCommonInterfaceConfig")
    }) {
        return Kind::Router(controls);
    }
    if ipp
        || services
            .iter()
            .any(|s| s.starts_with("_ipp") || s.starts_with("_printer"))
    {
        return Kind::Printer;
    }
    if mqtt {
        return Kind::Mqtt;
    }
    Kind::Unknown
}

/// Does `act` on the device at `ip` (`mac` wakes a TV that is off).
/// `target` picks one light, relay or channel inside the device.
pub async fn control(
    ip: IpAddr,
    mac: Option<String>,
    act: Act,
    target: Option<&str>,
) -> Result<String, String> {
    if !crate::is_private(ip) {
        return Err(format!("{ip} is not on the local network"));
    }
    // A TV that is off answers nothing: switching it on goes by Wake-on-LAN.
    if act == Act::On
        && let Some(m) = mac.clone()
        && !open(ip, 80).await
        && !open(ip, 8001).await
        && !open(ip, 8009).await
        && !open(ip, 5555).await
    {
        return crate::tv::control(ip, crate::tv::Action::PowerOn { mac: Some(m) }).await;
    }
    let kind = detect(ip).await;
    let done = match &kind {
        Kind::Tv(_) => crate::tv::control(ip, tv_action(&act, mac)?).await,
        Kind::Cast => crate::tv::control(ip, tv_action(&act, mac)?).await,
        Kind::Hue => lights::hue(ip, &act, target).await,
        Kind::Shelly { gen2 } => lights::shelly(ip, *gen2, &act, target).await,
        Kind::Tasmota => lights::tasmota(ip, &act).await,
        Kind::Wled => lights::wled(ip, &act).await,
        Kind::Kasa => lights::kasa(ip, &act).await,
        Kind::Yeelight => lights::yeelight(ip, &act).await,
        Kind::Renderer(c) => media::renderer(c, &act).await,
        Kind::Router(c) => media::router(c, &act).await,
        Kind::Printer => ipp::status(ip, &act).await,
        Kind::Mqtt => mqtt::control(ip, &act).await,
        // Nothing to control, but what is known about it still answers "status".
        Kind::Unknown if act == Act::Status => match last_seen(ip) {
            Some(d) => {
                let id = &d.identity;
                let what = [id.kind.as_str(), id.brand.as_str(), id.model.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" · ");
                let web = if d.open_ports.contains(&80) {
                    format!("; its own page is http://{ip}/")
                } else if d.open_ports.contains(&443) {
                    format!("; its own page is https://{ip}/")
                } else {
                    String::new()
                };
                Ok(format!(
                    "{} at {ip}: answering on the network, with no control protocol this engine speaks{web}",
                    if what.is_empty() {
                        "device".to_string()
                    } else {
                        what
                    }
                ))
            }
            None => Err(format!("{ip} did not answer")),
        },
        Kind::Unknown => Err(format!(
            "{ip} does not answer in any protocol this engine knows. Try device_control with its own web API (network_scan shows its open ports and services)."
        )),
    };
    done.map(|t| {
        if matches!(kind, Kind::Tv(_) | Kind::Unknown) {
            t
        } else {
            format!("{} · {t}", kind.label())
        }
    })
}

fn tv_action(act: &Act, mac: Option<String>) -> Result<crate::tv::Action, String> {
    use crate::tv::Action as T;
    Ok(match act {
        Act::Status => T::Info,
        Act::On => T::PowerOn { mac },
        Act::Off => T::PowerOff,
        Act::SetVolume(v) => T::SetVolume(*v),
        Act::VolumeUp(n) => T::VolumeUp(*n),
        Act::VolumeDown(n) => T::VolumeDown(*n),
        Act::Mute => T::Mute,
        Act::Play => T::Key("play".into()),
        Act::Pause => T::Key("pause".into()),
        Act::Stop => T::Key("stop".into()),
        Act::Next => T::Key("next".into()),
        Act::Previous => T::Key("previous".into()),
        Act::Launch { app, content } => T::Launch {
            app: app.clone(),
            content: content.clone(),
        },
        Act::Key(k) => T::Key(k.clone()),
        Act::Apps => T::Apps,
        Act::Toggle => return Err("a TV is switched with on or off".into()),
        Act::Brightness(_) | Act::Color(..) => {
            return Err("a TV has no brightness or colour control here".into());
        }
        Act::Publish { .. } | Act::Listen { .. } => {
            return Err("publish and listen are for an MQTT broker".into());
        }
    })
}

/// The Kasa discovery request (a broadcast, no length prefix).
pub fn kasa_probe() -> Vec<u8> {
    lights::kasa_cipher(br#"{"system":{"get_sysinfo":{}}}"#)
}

/// `(alias, model)` from a Kasa discovery answer.
pub fn kasa_answer(m: &[u8]) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_slice(&lights::kasa_decipher(m)).ok()?;
    let sys = &v["system"]["get_sysinfo"];
    Some((
        sys["alias"].as_str()?.to_string(),
        sys["model"].as_str().unwrap_or("").to_string(),
    ))
}

/// `#ff8800`, `ff8800`, or a few colour names, as RGB.
pub fn parse_color(s: &str) -> Option<(u8, u8, u8)> {
    let lower = s.trim().to_lowercase();
    let named = match lower.as_str() {
        "red" | "vermelho" => "ff0000",
        "green" | "verde" => "00ff00",
        "blue" | "azul" => "0000ff",
        "white" | "branco" => "ffffff",
        "warm" | "warm white" | "quente" => "ffb46b",
        "yellow" | "amarelo" => "ffd000",
        "orange" | "laranja" => "ff7a00",
        "purple" | "roxo" => "8a2be2",
        "pink" | "rosa" => "ff4fa3",
        "cyan" | "ciano" => "00e5ff",
        other => other.trim_start_matches('#'),
    };
    if named.len() != 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&named[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_by_hex_or_name() {
        assert_eq!(parse_color("#ff8800"), Some((255, 136, 0)));
        assert_eq!(parse_color("azul"), Some((0, 0, 255)));
        assert_eq!(parse_color("nope"), None);
    }

    #[test]
    fn tv_actions_map_from_the_general_ones() {
        assert_eq!(
            tv_action(&Act::Off, None).unwrap(),
            crate::tv::Action::PowerOff
        );
        assert!(tv_action(&Act::Brightness(50), None).is_err());
    }
}
