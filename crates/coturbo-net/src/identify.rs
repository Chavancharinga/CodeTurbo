//! What each device is and who made it, from every clue the network gives
//! (the way Home Assistant recognises devices): the services it announces,
//! its UPnP description, its name on the router, the maker of its network
//! card (IEEE OUI registry) and its web page title.

use crate::{Device, dns};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

/// What a device is, as a person would say it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Identity {
    /// "TV", "speaker", "printer", "phone", "router", ...
    pub kind: String,
    pub brand: String,
    pub model: String,
    /// The name it gives itself or the router knows it by.
    pub name: String,
    /// How it can be controlled here, when known.
    pub control: String,
}

/// mDNS service types: (service, kind, brand, how to control it).
const SERVICES: &[(&str, &str, &str, &str)] = &[
    ("kasa", "smart plug / light", "TP-Link Kasa", "device"),
    ("yeelight", "light", "Yeelight", "device"),
    ("_wled._tcp", "light", "WLED", "device"),
    ("_androidtvremote2._tcp", "TV", "Android TV", "device"),
    ("_googlecast._tcp", "TV / speaker (Google Cast)", "Google Cast", "device"),
    ("_amzn-wplay._tcp", "TV", "Amazon Fire TV", "device"),
    ("_airplay._tcp", "TV / AirPlay", "Apple", ""),
    ("_raop._tcp", "speaker / AirPlay", "", ""),
    ("_spotify-connect._tcp", "speaker", "", ""),
    ("_sonos._tcp", "speaker", "Sonos", "device"),
    ("_hap._tcp", "HomeKit accessory", "", ""),
    ("_homekit._tcp", "HomeKit accessory", "", ""),
    ("_matter._tcp", "Matter device", "", ""),
    ("_hue._tcp", "light bridge", "Philips Hue", "device"),
    ("_shelly._tcp", "smart switch / plug", "Shelly", "device"),
    ("_esphomelib._tcp", "smart device", "ESPHome", ""),
    ("_mqtt._tcp", "MQTT broker", "", ""),
    ("_ipp._tcp", "printer", "", "device"),
    ("_printer._tcp", "printer", "", ""),
    ("_pdl-datastream._tcp", "printer", "", ""),
    ("_companion-link._tcp", "Apple device", "Apple", ""),
    ("_smb._tcp", "computer / file share", "", ""),
    ("_workstation._tcp", "computer", "", ""),
    ("_ssh._tcp", "computer", "", ""),
];

/// Clues in a name or description: (text, kind, brand).
const NAME_HINTS: &[(&str, &str, &str)] = &[
    ("galaxy", "phone / tablet", "Samsung"),
    ("iphone", "phone", "Apple"),
    ("ipad", "tablet", "Apple"),
    ("macbook", "computer", "Apple"),
    ("imac", "computer", "Apple"),
    ("appletv", "TV", "Apple"),
    ("android", "Android device", ""),
    ("redmi", "phone", "Xiaomi"),
    ("poco", "phone", "Xiaomi"),
    ("xiaomi-1", "phone", "Xiaomi"),
    ("xiaomi-mi", "phone", "Xiaomi"),
    ("mitv", "TV", "Xiaomi"),
    ("xiaomi", "", "Xiaomi"),
    ("huawei", "", "Huawei"),
    ("oneplus", "phone", "OnePlus"),
    ("pixel", "phone", "Google"),
    ("chromecast", "TV / Chromecast", "Google"),
    ("bravia", "TV", "Sony"),
    ("[tv] samsung", "TV", "Samsung"),
    ("samsung", "", "Samsung"),
    ("lgwebos", "TV", "LG"),
    ("webos", "TV", "LG"),
    ("roku", "TV", "Roku"),
    ("firetv", "TV", "Amazon"),
    ("echo", "speaker", "Amazon"),
    ("alexa", "speaker", "Amazon"),
    ("sonos", "speaker", "Sonos"),
    ("playstation", "game console", "Sony"),
    ("ps5", "game console", "Sony"),
    ("xbox", "game console", "Microsoft"),
    ("nintendo", "game console", "Nintendo"),
    ("switch", "game console", "Nintendo"),
    ("hp", "printer", "HP"),
    ("epson", "printer", "Epson"),
    ("canon", "printer", "Canon"),
    ("brother", "printer", "Brother"),
    ("desktop-", "computer", "Windows PC"),
    ("laptop-", "computer", "Windows PC"),
    ("raspberrypi", "computer", "Raspberry Pi"),
    ("tasmota", "smart switch / plug", "Tasmota"),
    ("shelly", "smart switch / plug", "Shelly"),
    ("esp-", "smart device", "Espressif"),
    ("tuya", "smart device", "Tuya"),
    ("hikvision", "camera", "Hikvision"),
    ("reolink", "camera", "Reolink"),
    ("tapo", "camera / plug", "TP-Link Tapo"),
    ("ring", "camera / doorbell", "Ring"),
];

/// Combines every clue into what the device is.
pub fn identify(d: &Device, vendor: Option<&str>, host: Option<&str>, title: Option<&str>) -> Identity {
    let mut id = Identity::default();
    let set = |field: &mut String, value: &str| {
        if field.is_empty() && !value.trim().is_empty() {
            *field = value.trim().to_string();
        }
    };
    // 1. What it announces: the strongest clue.
    for s in &d.services {
        if let Some((_, kind, brand, control)) = SERVICES.iter().find(|(t, ..)| s.kind.starts_with(t)) {
            set(&mut id.kind, kind);
            set(&mut id.brand, brand);
            set(&mut id.control, control);
        }
        for t in &s.txt {
            let (k, v) = t.split_once('=').unwrap_or((t, ""));
            match k.to_ascii_lowercase().as_str() {
                "md" | "model" | "ty" | "product" => set(&mut id.model, v),
                "fn" => set(&mut id.name, v),
                "manufacturer" | "mf" | "vendor" => set(&mut id.brand, v),
                "am" => set(&mut id.model, v),
                _ => {}
            }
        }
    }
    // 2. Its UPnP description.
    for u in &d.upnp {
        // The Wi-Fi setup service (WPS) names the radio chip's maker, not the device's.
        if !u.device_type.contains("WFADevice") {
            set(&mut id.brand, &u.manufacturer);
            set(&mut id.model, &u.model);
        }
        let t = u.device_type.to_lowercase();
        let kind = if t.contains("mediarenderer") {
            "TV / speaker"
        } else if t.contains("internetgateway") || t.contains("wfadevice") || t.contains("wlanaccesspoint") {
            "router"
        } else if t.contains("mediaserver") {
            "media server"
        } else if t.contains("printer") {
            "printer"
        } else {
            ""
        };
        set(&mut id.kind, kind);
        if t.contains("mediarenderer") {
            set(&mut id.control, "device");
        }
    }
    let names: Vec<String> = d.names.iter().cloned().chain(host.map(str::to_string)).chain(title.map(str::to_string)).collect();
    // The router's name for it beats generic service names ("WFADevice").
    if let Some(h) = host {
        set(&mut id.name, h);
    }
    if let Some(n) = d.names.iter().find(|n| !n.eq_ignore_ascii_case("WFADevice")) {
        set(&mut id.name, n);
    }
    // 3. Names: the device's own, the router's, the web page title.
    let joined = names.join(" ").to_lowercase().replace(' ', "");
    for (hint, kind, brand) in NAME_HINTS {
        if joined.contains(&hint.replace(' ', "")) {
            set(&mut id.kind, kind);
            set(&mut id.brand, brand);
        }
    }
    // 4. Who made its network card.
    if let Some(v) = vendor {
        set(&mut id.brand, &short_vendor(v));
    }
    if d.mac.as_deref().is_some_and(is_private_mac) {
        set(&mut id.kind, "phone / tablet / laptop (private Wi-Fi address)");
    }
    // 5. Ports that only one kind of device opens.
    let p = &d.open_ports;
    if p.contains(&8001) || p.contains(&8002) {
        set(&mut id.kind, "TV");
        set(&mut id.brand, "Samsung");
        set(&mut id.control, "device");
    }
    if p.contains(&3000) || p.contains(&3001) {
        set(&mut id.control, "device");
    }
    if p.contains(&8060) {
        set(&mut id.kind, "TV");
        set(&mut id.brand, "Roku");
        set(&mut id.control, "device");
    }
    if p.contains(&5555) {
        set(&mut id.kind, "Android device (ADB on)");
        set(&mut id.control, "device");
    }
    if p.contains(&9100) || p.contains(&631) {
        set(&mut id.kind, "printer");
    }
    if p.contains(&554) {
        set(&mut id.kind, "camera");
    }
    if p.contains(&8123) {
        set(&mut id.kind, "Home Assistant server");
    }
    if p.contains(&1883) {
        set(&mut id.kind, "MQTT broker");
    }
    if id.kind.is_empty() && !d.upnp.is_empty() {
        id.kind = "network device".into();
    }
    id
}

/// A MAC address the device made up for privacy (phones, recent laptops).
pub fn is_private_mac(mac: &str) -> bool {
    u8::from_str_radix(mac.get(0..2).unwrap_or("00"), 16).is_ok_and(|b| b & 0x02 != 0)
}

/// "Samsung Electronics Co.,Ltd" → "Samsung Electronics".
fn short_vendor(v: &str) -> String {
    let mut s = v.to_string();
    for cut in [",", " Co.", " Corporation", " Inc", " Ltd", " Limited", " LLC", " GmbH", " Technologies", " Technology"] {
        if let Some(i) = s.find(cut) {
            s.truncate(i);
        }
    }
    s.trim().to_string()
}

/// The IEEE registry of who owns each MAC prefix, downloaded once (a few
/// MB) and kept in the CodeTurbo config folder.
pub async fn vendors() -> HashMap<String, String> {
    let path = crate::tv::state_dir().join("oui.csv");
    let fresh = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < Duration::from_secs(90 * 24 * 3600));
    if !fresh
        && let Ok(Ok(resp)) = tokio::time::timeout(Duration::from_secs(20), reqwest::get("https://standards-oui.ieee.org/oui/oui.csv")).await
        && resp.status().is_success()
        && let Ok(body) = resp.text().await
        && body.len() > 100_000
    {
        let _ = std::fs::write(&path, &body);
    }
    parse_oui(&std::fs::read_to_string(&path).unwrap_or_default())
}

/// `Registry,Assignment,Organization Name,...` → prefix "aabbcc" to name.
pub fn parse_oui(csv: &str) -> HashMap<String, String> {
    csv.lines()
        .skip(1)
        .filter_map(|l| {
            let mut fields = l.splitn(3, ',');
            let _registry = fields.next()?;
            let prefix = fields.next()?.trim().to_lowercase();
            let rest = fields.next()?;
            let org = if let Some(stripped) = rest.strip_prefix('"') { stripped.split('"').next()? } else { rest.split(',').next()? };
            (prefix.len() == 6).then(|| (prefix, org.trim().to_string()))
        })
        .collect()
}

pub fn vendor_of<'a>(table: &'a HashMap<String, String>, mac: &str) -> Option<&'a str> {
    let prefix: String = mac.chars().filter(|c| c.is_ascii_hexdigit()).take(6).collect::<String>().to_lowercase();
    table.get(&prefix).map(String::as_str)
}

/// The name the router's DNS gives `ip` (reverse lookup), e.g. "Galaxy-S23".
pub async fn host_name(router: Ipv4Addr, ip: Ipv4Addr) -> Option<String> {
    let o = ip.octets();
    let name = format!("{}.{}.{}.{}.in-addr.arpa", o[3], o[2], o[1], o[0]);
    let mut q = dns::query(&[&name], dns::PTR);
    q[1] = 7;
    q[2] = 0x01; // recursion desired
    let len = q.len();
    q[len - 2..].copy_from_slice(&[0, 1]); // class IN, no unicast bit
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await.ok()?;
    socket.send_to(&q, (router, 53)).await.ok()?;
    let mut buf = [0u8; 1500];
    let n = tokio::time::timeout(Duration::from_millis(800), socket.recv(&mut buf)).await.ok()?.ok()?;
    dns::records(&buf[..n]).into_iter().find_map(|r| match r.data {
        dns::Data::Ptr(h) => Some(h.trim_end_matches('.').trim_end_matches(".home").trim_end_matches(".lan").trim_end_matches(".local").to_string()),
        _ => None,
    })
}

/// The `<title>` of the device's own web page, if it has one.
pub async fn web_title(ip: IpAddr) -> Option<String> {
    let client = reqwest::Client::builder().timeout(Duration::from_millis(1500)).no_proxy().danger_accept_invalid_certs(true).build().ok()?;
    let body = client.get(format!("http://{ip}/")).send().await.ok()?.text().await.ok()?;
    let lower = body.to_lowercase();
    let start = lower.find("<title>")? + 7;
    let end = lower[start..].find("</title>")? + start;
    let t = body[start..end].trim();
    (!t.is_empty() && t.len() < 120).then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Service;

    #[test]
    fn reads_the_ieee_registry() {
        let csv = "Registry,Assignment,Organization Name,Organization Address\nMA-L,F4F5D8,Google Inc.,1600 Amphitheatre\nMA-L,8C79F5,\"Samsung Electronics Co.,Ltd\",Suwon\n";
        let t = parse_oui(csv);
        assert_eq!(vendor_of(&t, "f4:f5:d8:01:02:03"), Some("Google Inc."));
        assert_eq!(vendor_of(&t, "8C-79-F5-AA-BB-CC"), Some("Samsung Electronics Co.,Ltd"));
        assert_eq!(short_vendor("Samsung Electronics Co.,Ltd"), "Samsung Electronics");
    }

    #[test]
    fn recognises_a_chromecast_by_what_it_announces() {
        let d = Device {
            services: vec![Service { kind: "_googlecast._tcp".into(), port: 8009, txt: vec!["md=Chromecast Ultra".into(), "fn=Sala".into()] }],
            ..Device::default()
        };
        let id = identify(&d, None, None, None);
        assert_eq!((id.kind.as_str(), id.brand.as_str(), id.model.as_str(), id.name.as_str()), ("TV / speaker (Google Cast)", "Google Cast", "Chromecast Ultra", "Sala"));
    }

    #[test]
    fn recognises_a_samsung_tv_and_a_phone() {
        let tv = Device { open_ports: [8001, 8002, 9197].into_iter().collect(), ..Device::default() };
        let id = identify(&tv, Some("Samsung Electronics Co.,Ltd"), None, None);
        assert_eq!((id.kind.as_str(), id.brand.as_str(), id.control.as_str()), ("TV", "Samsung Electronics", "device"));
        let phone = Device { mac: Some("da:a1:19:00:00:01".into()), ..Device::default() };
        assert!(identify(&phone, None, Some("Galaxy-S23"), None).brand == "Samsung");
        assert!(identify(&phone, None, None, None).kind.contains("private"));
    }
}
