//! WiFi sensing, natively: people disturb the radio waves that fill a room,
//! and the disturbance shows in the signal. Two sources:
//!
//! - **CSI from ESP32 nodes** (the RuView / ESP32 CSI firmware frame, UDP
//!   port 5005): the amplitude of every subcarrier, tens of times a second.
//!   Enough for movement, presence and, when the person is still, breathing.
//! - **This machine's own WiFi** (RSSI from the WLAN API or `netsh` on
//!   Windows, `/proc/net/wireless`, or Termux): no extra hardware. Enough
//!   for movement near the path between the computer and the router. On
//!   Windows it also scans every access point in reach: each radio (a
//!   router's bands, the neighbours') crosses the room by its own path, so
//!   each is one more sensor.
//!
//! Every number is an estimate: WiFi sensing is coarse, not a camera and not
//! a medical device.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const CSI_PORT: u16 = 5005;
const CSI_MAGIC: u32 = 0xC511_0001;
/// Seconds of samples kept.
const KEEP: f64 = 60.0;

/// Set to stop a running sensing loop (the screen's `x`).
static STOP: AtomicBool = AtomicBool::new(false);

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
}

/// One CSI frame of an ESP32 node.
#[derive(Debug, Clone, PartialEq)]
pub struct CsiFrame {
    pub node: u8,
    pub seq: u32,
    pub freq_mhz: u32,
    pub rssi: i8,
    /// Amplitude of each subcarrier.
    pub amps: Vec<f32>,
}

/// The ESP32 CSI frame: magic, node, antennas, subcarriers, frequency,
/// sequence, RSSI, noise floor, two reserved bytes, then I/Q pairs as i8.
pub fn parse_csi(b: &[u8]) -> Option<CsiFrame> {
    if b.len() < 20 || u32::from_le_bytes(b[0..4].try_into().ok()?) != CSI_MAGIC {
        return None;
    }
    let iq = &b[20..];
    let amps = iq.as_chunks::<2>().0.iter().map(|p| ((p[0] as i8 as f32).powi(2) + (p[1] as i8 as f32).powi(2)).sqrt()).collect::<Vec<_>>();
    if amps.is_empty() {
        return None;
    }
    Some(CsiFrame {
        node: b[4],
        seq: u32::from_le_bytes(b[12..16].try_into().ok()?),
        freq_mhz: u32::from_le_bytes(b[8..12].try_into().ok()?),
        rssi: b[16] as i8,
        amps,
    })
}

/// RSSI in dBm from `netsh wlan show interfaces` (the `Rssi` line, or the
/// `Signal` percentage on older Windows, in any language).
pub fn parse_netsh(out: &str) -> Option<f64> {
    let value = |l: &str| l.split_once(':').map(|(_, v)| v.trim().to_string());
    if let Some(v) = out.lines().find(|l| l.trim_start().to_lowercase().starts_with("rssi")).and_then(value) {
        return v.parse().ok();
    }
    let pct = out.lines().filter_map(value).find_map(|v| v.strip_suffix('%').and_then(|n| n.trim().parse::<f64>().ok()))?;
    Some(pct / 2.0 - 100.0)
}

/// RSSI in dBm of the first interface in `/proc/net/wireless`.
pub fn parse_proc_wireless(text: &str) -> Option<f64> {
    let line = text.lines().find(|l| l.contains(':') && !l.contains('|'))?;
    let level = line.split_once(':')?.1.split_whitespace().nth(2)?;
    let v: f64 = level.trim_end_matches('.').parse().ok()?;
    Some(if v > 0.0 { v - 256.0 } else { v })
}

fn quiet(cmd: &str, args: &[&str]) -> Option<String> {
    let mut c = std::process::Command::new(cmd);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let out = c.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The Windows WLAN API: the link's RSSI without starting a process, and
/// active scans of every access point in reach.
#[cfg(windows)]
pub mod wlan {
    use std::ffi::c_void;

    #[link(name = "wlanapi")]
    unsafe extern "system" {
        fn WlanOpenHandle(version: u32, reserved: *mut c_void, negotiated: *mut u32, handle: *mut *mut c_void) -> u32;
        fn WlanCloseHandle(handle: *mut c_void, reserved: *mut c_void) -> u32;
        fn WlanEnumInterfaces(handle: *mut c_void, reserved: *mut c_void, list: *mut *mut u8) -> u32;
        fn WlanQueryInterface(handle: *mut c_void, guid: *const [u8; 16], op: u32, reserved: *mut c_void, size: *mut u32, data: *mut *mut u8, kind: *mut u32) -> u32;
        fn WlanScan(handle: *mut c_void, guid: *const [u8; 16], ssid: *const c_void, ie: *const c_void, reserved: *mut c_void) -> u32;
        fn WlanGetNetworkBssList(handle: *mut c_void, guid: *const [u8; 16], ssid: *const c_void, bss_type: u32, secure: i32, reserved: *mut c_void, list: *mut *mut u8) -> u32;
        fn WlanFreeMemory(memory: *mut c_void);
    }

    const OPCODE_RSSI: u32 = 0x1000_0102;
    const BSS_ANY: u32 = 3;
    /// `WLAN_BSS_ENTRY`: the MAC at 40, `lRssi` at 56.
    const BSS_ENTRY: usize = 360;

    /// An open handle on the first WiFi interface.
    pub struct Wlan {
        handle: *mut c_void,
        guid: [u8; 16],
    }

    // SAFETY: the handle is a token for the API, used from one thread at a time.
    unsafe impl Send for Wlan {}

    impl Wlan {
        pub fn open() -> Option<Self> {
            let (mut negotiated, mut handle) = (0u32, std::ptr::null_mut());
            // SAFETY: out-pointers to locals.
            if unsafe { WlanOpenHandle(2, std::ptr::null_mut(), &mut negotiated, &mut handle) } != 0 {
                return None;
            }
            let mut list = std::ptr::null_mut();
            // SAFETY: `WLAN_INTERFACE_INFO_LIST` holds the count at 0 and the
            // first interface's GUID at 8; the list is freed after reading.
            let guid = unsafe {
                if WlanEnumInterfaces(handle, std::ptr::null_mut(), &mut list) != 0 || list.is_null() {
                    None
                } else {
                    let n = list.cast::<u32>().read_unaligned();
                    let guid = (n > 0).then(|| list.add(8).cast::<[u8; 16]>().read_unaligned());
                    WlanFreeMemory(list.cast());
                    guid
                }
            };
            match guid {
                Some(guid) => Some(Self { handle, guid }),
                None => {
                    // SAFETY: the handle opened above.
                    unsafe { WlanCloseHandle(handle, std::ptr::null_mut()) };
                    None
                }
            }
        }

        /// The connected link's RSSI in dBm, if connected.
        pub fn rssi(&self) -> Option<f64> {
            let (mut size, mut data, mut kind) = (0u32, std::ptr::null_mut(), 0u32);
            // SAFETY: the API allocates `data` (a LONG), freed after reading.
            unsafe {
                if WlanQueryInterface(self.handle, &self.guid, OPCODE_RSSI, std::ptr::null_mut(), &mut size, &mut data, &mut kind) != 0 || data.is_null() {
                    return None;
                }
                let v = data.cast::<i32>().read_unaligned();
                WlanFreeMemory(data.cast());
                (v < 0).then_some(f64::from(v))
            }
        }

        /// Asks the card for a fresh scan; the results land a few seconds later.
        pub fn scan(&self) {
            // SAFETY: a null SSID and IE ask for a scan of everything.
            unsafe { WlanScan(self.handle, &self.guid, std::ptr::null(), std::ptr::null(), std::ptr::null_mut()) };
        }

        /// Every access point of the last scan: MAC and dBm.
        pub fn access_points(&self) -> Vec<([u8; 6], f64)> {
            let mut list = std::ptr::null_mut();
            // SAFETY: `WLAN_BSS_LIST` holds the count at 4 and entries of
            // `BSS_ENTRY` bytes from 8; the list is freed after reading.
            unsafe {
                if WlanGetNetworkBssList(self.handle, &self.guid, std::ptr::null(), BSS_ANY, 0, std::ptr::null_mut(), &mut list) != 0 || list.is_null() {
                    return Vec::new();
                }
                let n = list.add(4).cast::<u32>().read_unaligned() as usize;
                let out = (0..n)
                    .map(|i| {
                        let e = list.add(8 + i * BSS_ENTRY);
                        (e.add(40).cast::<[u8; 6]>().read_unaligned(), f64::from(e.add(56).cast::<i32>().read_unaligned()))
                    })
                    .filter(|(_, dbm)| *dbm < 0.0)
                    .collect();
                WlanFreeMemory(list.cast());
                out
            }
        }
    }

    impl Drop for Wlan {
        fn drop(&mut self) {
            // SAFETY: the handle opened in `open`.
            unsafe { WlanCloseHandle(self.handle, std::ptr::null_mut()) };
        }
    }
}

/// This machine's WiFi signal now, in dBm, if it is on WiFi.
pub fn host_rssi() -> Option<f64> {
    host_wifi().ok()
}

/// Runs a command for at most `secs`. The Termux:API commands wait forever
/// when the Termux:API app is missing, so they are killed after a while.
/// `Err(true)` when the command is not installed.
fn bounded(cmd: &str, args: &[&str], secs: u64) -> Result<Option<String>, bool> {
    let mut child = std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.kind() == std::io::ErrorKind::NotFound)?;
    let until = Instant::now() + Duration::from_secs(secs);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                // Both streams: some tools explain themselves on either.
                let mut out = String::new();
                if let Some(mut o) = child.stdout.take() {
                    let _ = std::io::Read::read_to_string(&mut o, &mut out);
                }
                if let Some(mut e) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut e, &mut out);
                }
                return Ok(Some(out));
            }
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
        }
    }
}

/// This machine's WiFi signal in dBm, or why it cannot be read, in words.
pub fn host_wifi() -> Result<f64, String> {
    if cfg!(windows) {
        return quiet("netsh", &["wlan", "show", "interfaces"]).as_deref().and_then(parse_netsh).ok_or_else(|| "this computer is not on WiFi".to_string());
    }
    if let Some(v) = std::fs::read_to_string("/proc/net/wireless").ok().as_deref().and_then(parse_proc_wireless) {
        return Ok(v);
    }
    let termux = std::env::var_os("TERMUX_VERSION").is_some() || cfg!(target_os = "android");
    // Termux on Android: the Termux:API app and its `termux-api` package.
    match bounded("termux-wifi-connectioninfo", &[], 6) {
        // Termux from Google Play has no Termux:API yet, and the API app only
        // pairs with a Termux from the same source (F-Droid or GitHub).
        Ok(Some(out)) if out.contains("Google Play") => Err("Termux from Google Play cannot read the WiFi yet (no Termux:API there): install Termux and Termux:API both from F-Droid or both from GitHub".to_string()),
        Ok(Some(out)) => {
            let j: serde_json::Value = serde_json::from_str(&out).map_err(|_| "Termux:API gave no WiFi reading".to_string())?;
            j["rssi"].as_f64().filter(|r| *r < 0.0 && *r > -127.0).ok_or_else(|| "this phone is not on WiFi".to_string())
        }
        Ok(None) => Err("the Termux:API app did not answer: install it from the store you got Termux from, open it once, then run /sense again".to_string()),
        Err(true) if termux => Err("to read this phone's WiFi, install the Termux:API app and run: pkg install termux-api".to_string()),
        Err(_) => Err("this computer is not on WiFi".to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// Not enough samples yet.
    Learning,
    Moving,
    /// No movement, but breathing shows: someone is there and still.
    Still,
    Empty,
}

impl Presence {
    pub fn label(self) -> &'static str {
        match self {
            Presence::Learning => "learning the room",
            Presence::Moving => "someone is moving",
            Presence::Still => "someone is there, still",
            Presence::Empty => "no movement",
        }
    }
}

/// What the signal says right now.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// `ESP32 CSI (2 nodes)` or `this computer's WiFi`.
    pub source: String,
    pub csi: bool,
    pub rate_hz: f64,
    pub seconds: f64,
    /// Signal level in dBm, when known.
    pub level: Option<f64>,
    /// 0 (still) to 1 (a lot of movement).
    pub motion: f64,
    pub presence: Presence,
    /// Breaths per minute and confidence 0-1.
    pub breathing: Option<(f64, f64)>,
    /// Motion over the last minute, oldest first, 0-1.
    pub motion_history: Vec<f64>,
    /// The signal over the last minute (dBm or mean amplitude).
    pub signal_history: Vec<f64>,
    /// The level of each path heard lately, by name (`link` or an access
    /// point's MAC): where a person stands changes it, so a room map can match it.
    pub fingerprint: Vec<(String, f64)>,
}

impl Reading {
    /// The reading in a few lines, for the model.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "source: {} ({:.1} samples/s, {:.0} s measured)\npresence: {} (motion {:.0}%)",
            self.source,
            self.rate_hz,
            self.seconds,
            self.presence.label(),
            self.motion * 100.0
        );
        if let Some(l) = self.level {
            s.push_str(&format!("\nsignal: {l:.0} dBm"));
        }
        match self.breathing {
            Some((bpm, c)) => s.push_str(&format!("\nbreathing: {bpm:.0}/min ({:.0}% confidence)", c * 100.0)),
            None if self.csi => s.push_str("\nbreathing: not found (shows only while the person is still)"),
            None => s.push_str("\nbreathing: needs CSI from an ESP32 node; the computer's own WiFi only shows movement"),
        }
        s.push_str("\nEstimates from WiFi: coarse, not a camera and not a medical measurement.");
        s
    }
}

#[derive(Debug, Clone)]
struct Sample {
    t: f64,
    /// dBm for RSSI, mean amplitude for CSI.
    value: f64,
    amps: Vec<f32>,
}

/// Turns samples into readings.
#[derive(Debug, Default)]
pub struct Analyzer {
    rssi: VecDeque<Sample>,
    /// Every access point heard in the scans: its MAC and `(time, dBm)`.
    aps: std::collections::BTreeMap<[u8; 6], VecDeque<(f64, f64)>>,
    csi: VecDeque<Sample>,
    nodes: std::collections::BTreeSet<u8>,
    /// The empty room's raw motion, from `raws`.
    floor: Option<f64>,
    /// Raw motion over the last two minutes: `(time, value)`.
    raws: VecDeque<(f64, f64)>,
    motions: VecDeque<(f64, f64)>,
    last_motion_at: Option<f64>,
}

fn std_dev(v: impl Iterator<Item = f64> + Clone) -> f64 {
    let n = v.clone().count() as f64;
    if n < 2.0 {
        return 0.0;
    }
    let mean = v.clone().sum::<f64>() / n;
    (v.map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
}

/// Strongest rhythm between 0.1 and 0.5 Hz (6 to 30 breaths a minute) in
/// irregularly timed samples: `(breaths per minute, share of the band's power)`.
pub fn breathing(samples: &[(f64, f64)]) -> Option<(f64, f64)> {
    if samples.len() < 40 || samples.last()?.0 - samples.first()?.0 < 20.0 {
        return None;
    }
    let mean = samples.iter().map(|s| s.1).sum::<f64>() / samples.len() as f64;
    let mut best = (0.0, 0.0);
    let mut total = 0.0;
    let mut f = 0.1;
    while f <= 0.5 {
        let (mut re, mut im) = (0.0, 0.0);
        for (t, x) in samples {
            let a = std::f64::consts::TAU * f * t;
            re += (x - mean) * a.cos();
            im -= (x - mean) * a.sin();
        }
        let p = re * re + im * im;
        total += p;
        if p > best.1 {
            best = (f, p);
        }
        f += 0.005;
    }
    if total <= 0.0 {
        return None;
    }
    // Power within ±0.02 Hz of the peak against the whole band.
    let peak: f64 = {
        let mut sum = 0.0;
        let mut g = best.0 - 0.02;
        while g <= best.0 + 0.02 {
            let (mut re, mut im) = (0.0, 0.0);
            for (t, x) in samples {
                let a = std::f64::consts::TAU * g * t;
                re += (x - mean) * a.cos();
                im -= (x - mean) * a.sin();
            }
            sum += re * re + im * im;
            g += 0.005;
        }
        sum
    };
    Some((best.0 * 60.0, (peak / total).min(1.0)))
}

impl Analyzer {
    pub fn push_rssi(&mut self, t: f64, dbm: f64) {
        self.rssi.push_back(Sample { t, value: dbm, amps: Vec::new() });
        while self.rssi.front().is_some_and(|s| t - s.t > KEEP) {
            self.rssi.pop_front();
        }
    }

    /// One scan: each access point heard and its level.
    pub fn push_scan(&mut self, t: f64, heard: &[([u8; 6], f64)]) {
        for (mac, dbm) in heard {
            self.aps.entry(*mac).or_default().push_back((t, *dbm));
        }
        for v in self.aps.values_mut() {
            while v.front().is_some_and(|s| t - s.0 > KEEP) {
                v.pop_front();
            }
        }
        self.aps.retain(|_, v| !v.is_empty());
    }

    /// Access points heard in the last half minute.
    fn heard(&self, now: f64) -> usize {
        self.aps.values().filter(|v| v.back().is_some_and(|s| now - s.0 <= 30.0)).count()
    }

    /// Each path's mean level over the last few seconds: the link over 2 s,
    /// each access point over its last scans (5 s).
    fn fingerprint(&self, now: f64) -> Vec<(String, f64)> {
        let mean = |v: Vec<f64>| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
        let link = mean(self.rssi.iter().filter(|s| now - s.t <= 2.0).map(|s| s.value).collect());
        let aps = self.aps.iter().filter_map(|(mac, v)| {
            let m = mean(v.iter().filter(|s| now - s.0 <= 5.0).map(|s| s.1).collect())?;
            Some((mac.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":"), m))
        });
        link.map(|l| ("link".to_string(), l)).into_iter().chain(aps).collect()
    }

    pub fn push_csi(&mut self, t: f64, f: &CsiFrame) {
        self.nodes.insert(f.node);
        let mean = f.amps.iter().map(|a| f64::from(*a)).sum::<f64>() / f.amps.len() as f64;
        self.csi.push_back(Sample { t, value: mean, amps: f.amps.clone() });
        while self.csi.front().is_some_and(|s| t - s.t > KEEP) {
            self.csi.pop_front();
        }
    }

    /// CSI wins while frames keep coming.
    fn use_csi(&self, now: f64) -> bool {
        self.csi.back().is_some_and(|s| now - s.t < 3.0) && self.csi.len() >= 10
    }

    /// Raw motion of the last few seconds: CSI's mean relative change per
    /// subcarrier, or RSSI's spread in dB.
    fn raw_motion(&self, now: f64, csi: bool) -> Option<f64> {
        if csi {
            let w: Vec<&Sample> = self.csi.iter().filter(|s| now - s.t <= 2.0).collect();
            let n = w.iter().map(|s| s.amps.len()).min()?;
            if w.len() < 5 || n == 0 {
                return None;
            }
            let per: f64 = (0..n)
                .map(|k| {
                    let v = w.iter().map(|s| f64::from(s.amps[k]));
                    let mean = v.clone().sum::<f64>() / w.len() as f64;
                    if mean > 0.5 { std_dev(v) / mean } else { 0.0 }
                })
                .sum::<f64>();
            Some(per / n as f64)
        } else {
            let w: Vec<f64> = self.rssi.iter().filter(|s| now - s.t <= 3.0).map(|s| s.value).collect();
            let link = (w.len() >= 3).then(|| std_dev(w.iter().copied()));
            // The scans: each access point's spread over its last few scans,
            // averaged over the half that moved most (the paths a person crosses).
            let mut per: Vec<f64> = self
                .aps
                .values()
                .map(|v| v.iter().filter(|s| now - s.0 <= 8.0).map(|s| s.1).collect::<Vec<_>>())
                .filter(|v| v.len() >= 3)
                .map(|v| std_dev(v.into_iter()))
                .collect();
            per.sort_by(|a, b| b.total_cmp(a));
            let half = per.len().div_ceil(2);
            let scans = (half > 0).then(|| per[..half].iter().sum::<f64>() / half as f64);
            match (link, scans) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            }
        }
    }

    pub fn reading(&mut self, now: f64, seconds: f64) -> Reading {
        let csi = self.use_csi(now);
        let series = if csi { &self.csi } else { &self.rssi };
        let span = match (series.front(), series.back()) {
            (Some(a), Some(b)) if b.t > a.t => b.t - a.t,
            _ => 0.0,
        };
        let rate = if span > 0.0 { (series.len() - 1) as f64 / span } else { 0.0 };
        let signal_history: Vec<f64> = series.iter().map(|s| s.value).collect();
        let level = if csi { None } else { self.rssi.back().map(|s| s.value) };
        let source = match (csi, self.heard(now)) {
            (true, _) => format!("ESP32 CSI ({} node{})", self.nodes.len(), if self.nodes.len() == 1 { "" } else { "s" }),
            (false, 0) => "this computer's WiFi".to_string(),
            (false, 1) => "this computer's WiFi + 1 access point".to_string(),
            (false, n) => format!("this computer's WiFi + {n} access points"),
        };

        let raw = self.raw_motion(now, csi);
        // The empty room: the lower quarter of the last two minutes, so a few
        // minutes of movement do not become the floor. Never under the noise
        // of 1 dB steps: drivers round RSSI to whole dB, and a level flipping
        // between two values (-42, -43) spreads up to 0.58 dB with no one there.
        if let Some(r) = raw {
            self.raws.push_back((now, r));
        }
        while self.raws.front().is_some_and(|x| now - x.0 > 120.0) {
            self.raws.pop_front();
        }
        let mut sorted: Vec<f64> = self.raws.iter().map(|x| x.1).collect();
        sorted.sort_by(f64::total_cmp);
        let noise = if csi { 0.0 } else { 0.6 };
        self.floor = (!sorted.is_empty()).then(|| sorted[sorted.len() / 4].max(noise));
        // How far above the empty room: CSI in relative change, RSSI in dB.
        let scale = if csi { 0.05 } else { 1.5 };
        let motion = match (raw, self.floor) {
            (Some(r), Some(f)) => ((r - f) / scale).clamp(0.0, 1.0),
            _ => 0.0,
        };
        self.motions.push_back((now, motion));
        while self.motions.front().is_some_and(|m| now - m.0 > KEEP) {
            self.motions.pop_front();
        }
        if motion > 0.25 {
            self.last_motion_at = Some(now);
        }
        let breath = if csi && motion < 0.25 {
            let pts: Vec<(f64, f64)> = self.csi.iter().filter(|s| now - s.t <= 30.0).map(|s| (s.t, s.value)).collect();
            breathing(&pts).filter(|(_, c)| *c >= 0.3)
        } else {
            None
        };
        let presence = if seconds < 5.0 || raw.is_none() {
            Presence::Learning
        } else if self.last_motion_at.is_some_and(|t| now - t < 8.0) {
            Presence::Moving
        } else if breath.is_some() {
            Presence::Still
        } else {
            Presence::Empty
        };
        Reading {
            source,
            csi,
            rate_hz: rate,
            seconds,
            level,
            motion,
            presence,
            breathing: breath,
            motion_history: self.motions.iter().map(|m| m.1).collect(),
            signal_history,
            fingerprint: if csi { Vec::new() } else { self.fingerprint(now) },
        }
    }
}

/// What a sensing run tells the screen.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Start,
    Reading(Reading),
    /// The run ended (time up, or the user closed the screen).
    End,
}

/// Senses for `seconds` (or until [`stop`]), calling `on` four times a second.
/// Listens for ESP32 CSI on UDP 5005 and samples this machine's WiFi.
pub async fn run(seconds: Option<f64>, on: &(dyn Fn(Event) + Send + Sync)) -> Result<Reading, String> {
    STOP.store(false, Ordering::SeqCst);
    on(Event::Start);
    let started = Instant::now();
    let an = std::sync::Arc::new(std::sync::Mutex::new(Analyzer::default()));
    let native = native_wifi(an.clone(), started);
    // Elsewhere the system's own reading, on a thread of its own: on a phone
    // it can take seconds. It keeps trying, and says why while it cannot.
    let problem = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let host = native.is_none().then(|| {
        let (an, problem) = (an.clone(), problem.clone());
        std::thread::spawn(move || {
            while !STOP.load(Ordering::SeqCst) {
                let got = host_wifi();
                let wait = match &got {
                    Ok(v) => {
                        if let Ok(mut a) = an.lock() {
                            a.push_rssi(started.elapsed().as_secs_f64(), *v);
                        }
                        400
                    }
                    Err(_) => 2000,
                };
                if let Ok(mut p) = problem.lock() {
                    *p = got.err();
                }
                let until = Instant::now() + Duration::from_millis(wait);
                while Instant::now() < until && !STOP.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        })
    });
    let udp = tokio::net::UdpSocket::bind(("0.0.0.0", CSI_PORT)).await.ok();
    let listen = {
        let an = an.clone();
        async move {
            let Some(sock) = udp else { return };
            let mut buf = vec![0u8; 4096];
            while !STOP.load(Ordering::SeqCst) {
                if let Ok(Ok((n, _))) = tokio::time::timeout(Duration::from_millis(200), sock.recv_from(&mut buf)).await
                    && let Some(f) = parse_csi(&buf[..n])
                    && let Ok(mut a) = an.lock()
                {
                    a.push_csi(started.elapsed().as_secs_f64(), &f);
                }
            }
        }
    };
    let sample = {
        let an = an.clone();
        async move {
            let mut last = Reading { source: String::new(), csi: false, rate_hz: 0.0, seconds: 0.0, level: None, motion: 0.0, presence: Presence::Learning, breathing: None, motion_history: Vec::new(), signal_history: Vec::new(), fingerprint: Vec::new() };
            loop {
                let now = started.elapsed().as_secs_f64();
                if STOP.load(Ordering::SeqCst) || seconds.is_some_and(|s| now >= s) {
                    STOP.store(true, Ordering::SeqCst);
                    return last;
                }
                if let Ok(mut a) = an.lock() {
                    last = a.reading(started.elapsed().as_secs_f64(), now);
                }
                if let Some(why) = problem.lock().ok().and_then(|p| p.clone())
                    && last.signal_history.is_empty()
                {
                    last.source = format!("waiting for ESP32 CSI on UDP port {CSI_PORT} ({why})");
                }
                on(Event::Reading(last.clone()));
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    };
    let (_, last) = tokio::join!(listen, sample);
    for t in native.into_iter().chain(host) {
        let _ = tokio::task::spawn_blocking(move || t.join()).await;
    }
    on(Event::End);
    Ok(last)
}

/// Windows: the WLAN API reads the link ten times a second and scans every
/// access point every two seconds, on a thread of its own until the run stops.
#[cfg(windows)]
fn native_wifi(an: std::sync::Arc<std::sync::Mutex<Analyzer>>, started: Instant) -> Option<std::thread::JoinHandle<()>> {
    let w = wlan::Wlan::open()?;
    w.rssi()?;
    Some(std::thread::spawn(move || {
        w.scan();
        let mut last_scan = Instant::now();
        while !STOP.load(Ordering::SeqCst) {
            let t = started.elapsed().as_secs_f64();
            if let Some(v) = w.rssi()
                && let Ok(mut a) = an.lock()
            {
                a.push_rssi(t, v);
            }
            if last_scan.elapsed() >= Duration::from_secs(2) {
                let heard = w.access_points();
                if let Ok(mut a) = an.lock() {
                    a.push_scan(t, &heard);
                }
                w.scan();
                last_scan = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }))
}

#[cfg(not(windows))]
fn native_wifi(_: std::sync::Arc<std::sync::Mutex<Analyzer>>, _: Instant) -> Option<std::thread::JoinHandle<()>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(node: u8, amp: i8) -> Vec<u8> {
        let mut b = CSI_MAGIC.to_le_bytes().to_vec();
        b.extend([node, 1]);
        b.extend(4u16.to_le_bytes());
        b.extend(2462u32.to_le_bytes());
        b.extend(7u32.to_le_bytes());
        b.extend([(-50i8) as u8, (-90i8) as u8, 0, 0]);
        for _ in 0..4 {
            b.extend([amp as u8, 0]);
        }
        b
    }

    #[test]
    fn frames_and_readouts_parse() {
        let f = parse_csi(&frame(3, 12)).unwrap();
        assert_eq!((f.node, f.seq, f.freq_mhz, f.rssi), (3, 7, 2462, -50));
        assert_eq!(f.amps, vec![12.0; 4]);
        assert!(parse_csi(b"not a frame at all, really").is_none());
        assert_eq!(parse_netsh("    Signal                 : 89% \n    Rssi                   : -45\n"), Some(-45.0));
        assert_eq!(parse_netsh("    Sinal                  : 80%\n"), Some(-60.0));
        let proc = "Inter-| sta-|   Quality        |   Discarded packets\n face | tus | link level noise |  nwid  crypt\n wlan0: 0000   54.  -56.  -256        0      0\n";
        assert_eq!(parse_proc_wireless(proc), Some(-56.0));
    }

    #[test]
    fn scans_of_every_access_point_show_movement_a_steady_link_hides() {
        let mut a = Analyzer::default();
        let (router, band, neighbour) = ([1; 6], [2; 6], [3; 6]);
        // A quiet room: the link and the scans hold still.
        for i in 0..40 {
            let t = f64::from(i) * 0.5;
            a.push_rssi(t, -43.0);
            if i % 4 == 0 {
                a.push_scan(t, &[(router, -45.0), (band, -57.0), (neighbour, -95.0)]);
            }
            a.reading(t, t);
        }
        let quiet = a.reading(20.0, 20.0);
        assert_eq!(quiet.presence, Presence::Empty);
        assert_eq!(quiet.source, "this computer's WiFi + 3 access points");
        assert_eq!(quiet.fingerprint[0], ("link".to_string(), -43.0));
        assert!(quiet.fingerprint.contains(&("02:02:02:02:02:02".to_string(), -57.0)));
        // Someone walks: the link the driver smooths stays at -43, the scans swing.
        let mut moving = quiet;
        for i in 40..60 {
            let t = f64::from(i) * 0.5;
            a.push_rssi(t, -43.0);
            if i % 4 == 0 {
                let swing = if i % 8 == 0 { 3.0 } else { -2.0 };
                a.push_scan(t, &[(router, -45.0 + swing), (band, -57.0 - swing), (neighbour, -95.0)]);
            }
            moving = a.reading(t, t);
        }
        assert_eq!(moving.presence, Presence::Moving, "{moving:?}");
    }

    #[test]
    fn a_missing_command_says_so_and_a_hung_one_is_cut_short() {
        assert_eq!(bounded("codeturbo-no-such-command", &[], 1), Err(true));
        let (quick, slow): (&[&str], &[&str]) = if cfg!(windows) { (&["/c", "echo hi"], &["/c", "ping -n 6 127.0.0.1 >nul"]) } else { (&["-c", "echo hi"], &["-c", "sleep 5"]) };
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(bounded(shell, quick, 5).unwrap().unwrap().contains("hi"));
        let t = Instant::now();
        assert_eq!(bounded(shell, slow, 1), Ok(None));
        assert!(t.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn whole_db_flips_of_an_empty_room_are_not_movement() {
        // Recorded in an empty room: the driver's level flips between -42 and
        // -43 and the scans between -54 and -55, nobody there.
        let mut a = Analyzer::default();
        let ap = [1; 6];
        for i in 0..360 {
            let t = f64::from(i) * 0.25;
            a.push_rssi(t, if (i / 7) % 2 == 0 { -42.0 } else { -43.0 });
            if i % 8 == 0 {
                a.push_scan(t, &[(ap, if (i / 16) % 2 == 0 { -54.0 } else { -55.0 })]);
            }
            let r = a.reading(t, t);
            assert_ne!(r.presence, Presence::Moving, "at {t}: {r:?}");
        }
    }

    #[test]
    fn breathing_is_found_in_a_still_signal() {
        // 15 breaths a minute (0.25 Hz), sampled at 10 Hz for 30 s, with noise.
        let pts: Vec<(f64, f64)> = (0..300).map(|i| {
            let t = i as f64 / 10.0;
            (t, 20.0 + (std::f64::consts::TAU * 0.25 * t).sin() + ((i * 7919) % 13) as f64 * 0.02)
        }).collect();
        let (bpm, conf) = breathing(&pts).unwrap();
        assert!((bpm - 15.0).abs() < 1.0 && conf > 0.5, "{bpm} {conf}");
    }

    #[test]
    fn movement_stands_out_from_the_empty_room() {
        let mut a = Analyzer::default();
        let mut t = 0.0;
        // A quiet room for 20 s...
        while t < 20.0 {
            a.push_rssi(t, -45.0 + if (t * 2.0) as i64 % 2 == 0 { 0.0 } else { 0.5 });
            a.reading(t, t);
            t += 0.5;
        }
        assert_eq!(a.reading(t, t).presence, Presence::Empty);
        // ...then someone walks through the path.
        for k in 0..12 {
            a.push_rssi(t, -45.0 + [0.0, -6.0, 3.0, -8.0][k % 4]);
            t += 0.5;
        }
        let r = a.reading(t, t);
        assert_eq!(r.presence, Presence::Moving, "{r:?}");
        assert!(r.motion > 0.5 && r.summary().contains("someone is moving"));
        assert!(r.summary().contains("needs CSI"));
    }

    /// An empty room through the WLAN API for 90 s, a line a second:
    /// `cargo test -p coturbo-net live_empty_room -- --ignored --nocapture`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_empty_room() {
        let an = std::sync::Arc::new(std::sync::Mutex::new(Analyzer::default()));
        let t0 = Instant::now();
        let native = native_wifi(an.clone(), t0).expect("WLAN API");
        while t0.elapsed().as_secs_f64() < 90.0 {
            std::thread::sleep(Duration::from_secs(1));
            let now = t0.elapsed().as_secs_f64();
            let mut a = an.lock().unwrap();
            let raw = a.raw_motion(now, false);
            let r = a.reading(now, now);
            let fp: Vec<String> = r.fingerprint.iter().map(|(k, v)| format!("{}={v:.1}", &k[k.len().saturating_sub(2)..])).collect();
            println!("{now:5.1} raw {raw:?} motion {:.2} {:?} {}", r.motion, r.presence, fp.join(" "));
        }
        STOP.store(true, Ordering::SeqCst);
        native.join().unwrap();
    }

    /// This machine's WiFi for 12 s: `cargo test -p coturbo-net live_wifi -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_wifi() {
        let mut a = Analyzer::default();
        let t0 = Instant::now();
        let mut n = 0;
        while t0.elapsed().as_secs_f64() < 12.0 {
            if let Some(v) = host_rssi() {
                a.push_rssi(t0.elapsed().as_secs_f64(), v);
                n += 1;
            }
            std::thread::sleep(Duration::from_millis(400));
        }
        let r = a.reading(t0.elapsed().as_secs_f64(), 12.0);
        println!("{n} samples
{}", r.summary());
        assert!(n > 5);
    }
}
