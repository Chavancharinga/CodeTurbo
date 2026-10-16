//! Android TV over ADB on port 5555 (Developer options > network / USB
//! debugging). The first connection makes the TV ask whether to allow this
//! computer; its RSA key is kept so it is not asked again. Commands run in
//! the TV's shell: key presses, app launches, the app list, system info.

use super::Action;
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, Pkcs1v15Sign, RsaPrivateKey};
use std::net::IpAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const CNXN: u32 = 0x4e58_4e43;
const AUTH: u32 = 0x4854_5541;
const OPEN: u32 = 0x4e45_504f;
const OKAY: u32 = 0x5941_4b4f;
const CLSE: u32 = 0x4553_4c43;
const WRTE: u32 = 0x4554_5257;
const VERSION: u32 = 0x0100_0000;
const MAX_DATA: u32 = 256 * 1024;

/// One ADB message on the wire: a 24-byte header, then the payload.
pub fn message(command: u32, arg0: u32, arg1: u32, data: &[u8]) -> Vec<u8> {
    let sum = data.iter().map(|&b| b as u32).fold(0u32, u32::wrapping_add);
    let mut m = Vec::with_capacity(24 + data.len());
    for word in [command, arg0, arg1, data.len() as u32, sum, command ^ 0xffff_ffff] {
        m.extend_from_slice(&word.to_le_bytes());
    }
    m.extend_from_slice(data);
    m
}

struct Packet {
    command: u32,
    arg0: u32,
    data: Vec<u8>,
}

async fn read(stream: &mut TcpStream, wait: Duration) -> Result<Packet, String> {
    let mut head = [0u8; 24];
    tokio::time::timeout(wait, stream.read_exact(&mut head)).await.map_err(|_| "the TV stopped answering".to_string())?.map_err(|e| e.to_string())?;
    let word = |i: usize| u32::from_le_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
    let mut data = vec![0u8; word(12) as usize];
    stream.read_exact(&mut data).await.map_err(|e| e.to_string())?;
    Ok(Packet { command: word(0), arg0: word(4), data })
}

/// This computer's ADB key, made once.
fn key() -> Result<RsaPrivateKey, String> {
    let path = super::state_dir().join("adbkey");
    if let Ok(pem) = std::fs::read_to_string(&path)
        && let Ok(k) = RsaPrivateKey::from_pkcs8_pem(&pem)
    {
        return Ok(k);
    }
    let k = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).map_err(|e| e.to_string())?;
    let pem = k.to_pkcs8_pem(LineEnding::LF).map_err(|e| e.to_string())?;
    std::fs::write(&path, pem.as_bytes()).map_err(|e| e.to_string())?;
    Ok(k)
}

/// The public key in Android's own format, as the TV stores it.
pub fn android_public_key(n: &BigUint, e: u32) -> Vec<u8> {
    const WORDS: usize = 64;
    let n_bytes = n.to_bytes_le();
    let word_at = |bytes: &[u8], i: usize| {
        let mut w = [0u8; 4];
        for (j, b) in w.iter_mut().enumerate() {
            *b = *bytes.get(i * 4 + j).unwrap_or(&0);
        }
        u32::from_le_bytes(w)
    };
    // -1 / n[0] mod 2^32, by Newton's iteration.
    let n0 = word_at(&n_bytes, 0);
    let mut inv: u32 = 1;
    for _ in 0..5 {
        inv = inv.wrapping_mul(2u32.wrapping_sub(n0.wrapping_mul(inv)));
    }
    let rr = (BigUint::from(1u8) << (WORDS * 32 * 2)) % n;
    let rr_bytes = rr.to_bytes_le();
    let mut out = Vec::with_capacity(4 * (3 + 2 * WORDS));
    out.extend_from_slice(&(WORDS as u32).to_le_bytes());
    out.extend_from_slice(&inv.wrapping_neg().to_le_bytes());
    for i in 0..WORDS {
        out.extend_from_slice(&word_at(&n_bytes, i).to_le_bytes());
    }
    for i in 0..WORDS {
        out.extend_from_slice(&word_at(&rr_bytes, i).to_le_bytes());
    }
    out.extend_from_slice(&e.to_le_bytes());
    out
}

/// Connects and signs in; the first time the TV asks to allow this computer.
async fn connect(ip: IpAddr) -> Result<TcpStream, String> {
    let mut s = tokio::time::timeout(Duration::from_secs(4), TcpStream::connect((ip, 5555)))
        .await
        .map_err(|_| "the TV did not answer on port 5555".to_string())?
        .map_err(|e| format!("ADB is off on the TV ({e}): turn on Developer options > network debugging"))?;
    s.write_all(&message(CNXN, VERSION, MAX_DATA, b"host::\0")).await.map_err(|e| e.to_string())?;
    let key = key()?;
    let mut sent_key = false;
    loop {
        let wait = if sent_key { Duration::from_secs(60) } else { Duration::from_secs(5) };
        let p = read(&mut s, wait).await.map_err(|e| if sent_key { "nobody allowed this computer on the TV in time; try again and choose Allow".to_string() } else { e })?;
        match p.command {
            CNXN => return Ok(s),
            AUTH if p.arg0 == 1 && !sent_key => {
                // Sign the token; if the TV does not know the key yet, offer it.
                let signature = key.sign(Pkcs1v15Sign::new::<sha1::Sha1>(), &p.data).map_err(|e| e.to_string())?;
                s.write_all(&message(AUTH, 2, 0, &signature)).await.map_err(|e| e.to_string())?;
                let next = read(&mut s, Duration::from_secs(5)).await?;
                if next.command == CNXN {
                    return Ok(s);
                }
                use base64::Engine;
                let public = key.to_public_key();
                let e = public.e().to_bytes_le().iter().rev().fold(0u32, |acc, &b| (acc << 8) | b as u32);
                let mut blob = base64::engine::general_purpose::STANDARD.encode(android_public_key(public.n(), e)).into_bytes();
                blob.extend_from_slice(b" codeturbo@codeturbo\0");
                s.write_all(&message(AUTH, 3, 0, &blob)).await.map_err(|e| e.to_string())?;
                sent_key = true;
            }
            _ => {}
        }
    }
}

/// Runs `command` in the TV's shell and returns what it printed.
pub(crate) async fn shell(ip: IpAddr, command: &str) -> Result<String, String> {
    let mut s = connect(ip).await?;
    let local = 1u32;
    let mut service = format!("shell:{command}").into_bytes();
    service.push(0);
    s.write_all(&message(OPEN, local, 0, &service)).await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    loop {
        let p = read(&mut s, Duration::from_secs(15)).await?;
        match p.command {
            WRTE => {
                out.extend_from_slice(&p.data);
                s.write_all(&message(OKAY, local, p.arg0, &[])).await.map_err(|e| e.to_string())?;
            }
            CLSE => break,
            _ => {}
        }
    }
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// Android key codes for the generic names.
pub fn key_code(key: &str) -> Option<u32> {
    let k = key.trim().to_lowercase().replace([' ', '-'], "_");
    let k = k.strip_prefix("keycode_").unwrap_or(&k);
    Some(match k {
        "home" => 3,
        "back" => 4,
        "up" | "dpad_up" => 19,
        "down" | "dpad_down" => 20,
        "left" | "dpad_left" => 21,
        "right" | "dpad_right" => 22,
        "ok" | "enter" | "select" | "center" | "dpad_center" => 23,
        "volume_up" | "volup" => 24,
        "volume_down" | "voldown" => 25,
        "power" => 26,
        "menu" | "settings" => 82,
        "play_pause" | "play" | "pause" => 85,
        "stop" => 86,
        "next" => 87,
        "previous" | "prev" => 88,
        "rewind" => 89,
        "fast_forward" | "forward" => 90,
        "mute" => 164,
        "channel_up" => 166,
        "channel_down" => 167,
        "sleep" => 223,
        "wakeup" => 224,
        n => return n.parse().ok(),
    })
}

/// Package names of common apps on Android TV.
fn package(app: &str) -> Option<&'static str> {
    Some(match super::app_alias(app).as_str() {
        "youtube" => "com.google.android.youtube.tv",
        "netflix" => "com.netflix.ninja",
        "primevideo" | "prime" | "amazonprimevideo" => "com.amazon.amazonvideo.livingroom",
        "disney" | "disneyplus" => "com.disney.disneyplus",
        "spotify" => "com.spotify.tv.android",
        "hbomax" | "max" => "com.wbd.stream",
        "twitch" => "tv.twitch.android.app",
        "plex" => "com.plexapp.android",
        "kodi" => "org.xbmc.kodi",
        "playstore" | "googleplay" => "com.android.vending",
        "settings" => "com.android.tv.settings",
        _ => return None,
    })
}

pub(crate) async fn control(ip: IpAddr, action: &Action) -> Result<String, String> {
    let key = |code: u32, times: u8| {
        let one = format!("input keyevent {code}");
        vec![one; times.max(1) as usize].join("; ")
    };
    match action {
        Action::Info => {
            let out = shell(
                ip,
                "getprop ro.product.manufacturer; getprop ro.product.model; getprop ro.build.version.release; \
                 getprop ro.build.version.security_patch; getprop ro.build.display.id",
            )
            .await?;
            let f: Vec<&str> = out.lines().map(str::trim).collect();
            let at = |i: usize| f.get(i).copied().unwrap_or("?");
            Ok(format!(
                "{} {} · Android {} · security patch {} · build {}. A patch date many months old means a system update is likely pending (Settings > System > About > System update).",
                at(0), at(1), at(2), at(3), at(4)
            ))
        }
        Action::Apps => {
            let out = shell(ip, "pm list packages -3").await?;
            let mut apps: Vec<&str> = out.lines().filter_map(|l| l.trim().strip_prefix("package:")).collect();
            apps.sort();
            Ok(format!("{} installed apps: {}", apps.len(), apps.join(", ")))
        }
        Action::Launch { app, content } => {
            let pkg = match package(app) {
                Some(p) => p.to_string(),
                None if app.contains('.') => app.clone(),
                None => {
                    let want = super::app_alias(app);
                    let list = shell(ip, "pm list packages").await?;
                    list.lines()
                        .filter_map(|l| l.trim().strip_prefix("package:"))
                        .find(|p| p.to_lowercase().contains(&want))
                        .map(str::to_string)
                        .ok_or_else(|| format!("{app} is not installed on this TV"))?
                }
            };
            let cmd = match content {
                Some(c) if pkg.contains("youtube") => {
                    let url = if c.starts_with("http") { c.clone() } else { format!("https://www.youtube.com/results?search_query={}", c.replace(' ', "+")) };
                    format!("am start -a android.intent.action.VIEW -d '{url}' {pkg}")
                }
                _ => format!("monkey -p {pkg} -c android.intent.category.LEANBACK_LAUNCHER 1"),
            };
            shell(ip, &cmd).await?;
            Ok(format!("{app} opened"))
        }
        Action::Key(k) => {
            let code = key_code(k).ok_or_else(|| format!("unknown key {k}"))?;
            shell(ip, &key(code, 1)).await.map(|_| format!("{k} pressed"))
        }
        Action::PowerOff => shell(ip, &key(223, 1)).await.map(|_| "turned off (sleep)".into()),
        Action::VolumeUp(n) => shell(ip, &key(24, *n)).await.map(|_| format!("volume up {n}")),
        Action::VolumeDown(n) => shell(ip, &key(25, *n)).await.map(|_| format!("volume down {n}")),
        Action::Mute => shell(ip, &key(164, 1)).await.map(|_| "mute toggled".into()),
        Action::SetVolume(v) => shell(ip, &format!("cmd media_session volume --stream 3 --set {v}")).await.map(|_| format!("volume set to {v}")),
        Action::PowerOn { .. } => unreachable!("handled before"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_carry_the_header_adb_expects() {
        let m = message(CNXN, VERSION, MAX_DATA, b"host::\0");
        assert_eq!(&m[0..4], b"CNXN");
        assert_eq!(u32::from_le_bytes(m[12..16].try_into().unwrap()), 7);
        let magic = u32::from_le_bytes(m[20..24].try_into().unwrap());
        assert_eq!(magic, CNXN ^ 0xffff_ffff);
        assert_eq!(&m[24..], b"host::\0");
    }

    #[test]
    fn the_public_key_has_androids_layout() {
        // A small odd modulus is enough to check the arithmetic.
        let n = BigUint::parse_bytes(b"c7f1b3a5e9d2f4c6a8b0d2e4f6a8c0e2f4a6b8c0d2e4f6a8b0c2d4e6f8a0b2c5", 16).unwrap();
        let blob = android_public_key(&n, 65537);
        assert_eq!(blob.len(), 4 * (3 + 128));
        let word = |i: usize| u32::from_le_bytes(blob[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!(word(0), 64);
        // n0inv * n[0] == -1 mod 2^32
        assert_eq!(word(1).wrapping_mul(word(2)), u32::MAX);
        assert_eq!(word(2 + 128), 65537);
    }

    #[test]
    fn generic_keys_become_android_key_codes() {
        assert_eq!(key_code("home"), Some(3));
        assert_eq!(key_code("volume up"), Some(24));
        assert_eq!(key_code("KEYCODE_MUTE"), Some(164));
        assert_eq!(key_code("85"), Some(85));
        assert_eq!(package("YouTube"), Some("com.google.android.youtube.tv"));
    }
}
