//! The devices seen before, kept in the CodeTurbo config folder by MAC
//! address: their address changes, they may be off right now, and the user
//! gives them names ("living room TV"). A TV that is off answers nothing,
//! yet it can be woken with the MAC remembered from when it was on.

use crate::{Device, Inventory};
use serde_json::{Value, json};
use std::net::IpAddr;
use std::path::PathBuf;

/// One remembered device.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Known {
    pub mac: String,
    /// The name the user gave it, if any.
    pub name: String,
    pub ip: String,
    pub what: String,
    /// Unix seconds when it last answered.
    pub seen: u64,
}

fn path() -> PathBuf {
    crate::tv::state_dir().join("devices.json")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn load() -> Vec<Known> {
    let v: Value = std::fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    let mut out: Vec<Known> = v
        .as_object()
        .into_iter()
        .flatten()
        .map(|(mac, d)| Known {
            mac: mac.clone(),
            name: d["name"].as_str().unwrap_or("").into(),
            ip: d["ip"].as_str().unwrap_or("").into(),
            what: d["what"].as_str().unwrap_or("").into(),
            seen: d["seen"].as_u64().unwrap_or(0),
        })
        .collect();
    out.sort_by_key(|k| std::cmp::Reverse(k.seen));
    out
}

fn save(all: &[Known]) {
    let mut v = serde_json::Map::new();
    for k in all {
        v.insert(
            k.mac.clone(),
            json!({"name": k.name, "ip": k.ip, "what": k.what, "seen": k.seen}),
        );
    }
    let _ = std::fs::write(
        path(),
        serde_json::to_string_pretty(&Value::Object(v)).unwrap_or_default(),
    );
}

fn describe(d: &Device) -> String {
    let id = &d.identity;
    [id.kind.as_str(), id.brand.as_str(), id.model.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Records what a scan found; keeps the user's names.
pub fn remember(inv: &Inventory) {
    let mut all = load();
    for d in &inv.devices {
        let (Some(mac), Some(ip)) = (&d.mac, d.ip) else {
            continue;
        };
        let what = describe(d);
        match all.iter_mut().find(|k| &k.mac == mac) {
            Some(k) => {
                k.ip = ip.to_string();
                if !what.is_empty() {
                    k.what = what;
                }
                k.seen = now();
            }
            None => all.push(Known {
                mac: mac.clone(),
                name: String::new(),
                ip: ip.to_string(),
                what,
                seen: now(),
            }),
        }
    }
    save(&all);
}

/// Gives a device a name, found by its address, MAC or current name.
pub fn name(target: &str, name: &str) -> Result<Known, String> {
    let mut all = load();
    let k = all
        .iter_mut()
        .find(|k| matches(k, target))
        .ok_or_else(|| format!("no known device matches `{target}`: run network_scan first"))?;
    k.name = name.trim().to_string();
    let named = k.clone();
    save(&all);
    Ok(named)
}

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn matches(k: &Known, target: &str) -> bool {
    let t = norm(target);
    !t.is_empty()
        && (k.ip == target.trim()
            || norm(&k.mac) == t
            || (!k.name.is_empty() && norm(&k.name) == t))
}

/// The device a user means: an address, a MAC or a name given before
/// (exact, then a name containing the words).
pub fn find(target: &str) -> Option<Known> {
    let all = load();
    if let Some(k) = all.iter().find(|k| matches(k, target)) {
        return Some(k.clone());
    }
    let t = norm(target);
    all.into_iter()
        .find(|k| !k.name.is_empty() && (norm(&k.name).contains(&t) || t.contains(&norm(&k.name))))
}

/// An address for `target`: an IP as given, or the one last seen for a
/// known device.
pub fn resolve(target: &str) -> Option<(IpAddr, Option<String>)> {
    if let Ok(ip) = target.trim().parse::<IpAddr>() {
        let mac = load()
            .into_iter()
            .find(|k| k.ip == target.trim())
            .map(|k| k.mac);
        return Some((ip, mac));
    }
    let k = find(target)?;
    Some((k.ip.parse().ok()?, Some(k.mac)))
}

/// The remembered devices that did not answer this scan (off or asleep).
pub fn missing(inv: &Inventory) -> Vec<Known> {
    load()
        .into_iter()
        .filter(|k| {
            !inv.devices
                .iter()
                .any(|d| d.mac.as_deref() == Some(k.mac.as_str()))
        })
        .collect()
}

/// The name the user gave the device with this MAC.
pub fn name_of(mac: Option<&str>) -> Option<String> {
    let mac = mac?;
    load()
        .into_iter()
        .find(|k| k.mac == mac)
        .map(|k| k.name)
        .filter(|n| !n.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_loosely() {
        let k = Known {
            mac: "aa:bb:cc:dd:ee:ff".into(),
            name: "TV da sala".into(),
            ip: "192.168.1.40".into(),
            ..Known::default()
        };
        assert!(matches(&k, "tv da sala"));
        assert!(matches(&k, "192.168.1.40"));
        assert!(matches(&k, "AA-BB-CC-DD-EE-FF"));
        assert!(!matches(&k, "TV do quarto"));
    }
}
