//! WiFi sensing, natively: people disturb the radio waves that fill a room,
//! and the disturbance shows in the signal. Two sources:
//!
//! - **CSI from ESP32 nodes** (the RuView / ESP32 CSI firmware frame, UDP
//!   port 5005): the amplitude of every subcarrier, tens of times a second.
//!   Enough for movement, presence and, when the person is still, breathing.
//! - **This machine's own WiFi link** (RSSI from `netsh`, `/proc/net/wireless`
//!   or Termux): no extra hardware, about once a second. Enough for
//!   movement near the path between the computer and the router.
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

/// This machine's WiFi signal now, in dBm, if it is on WiFi.
pub fn host_rssi() -> Option<f64> {
    if cfg!(windows) {
        return parse_netsh(&quiet("netsh", &["wlan", "show", "interfaces"])?);
    }
    if let Some(v) = std::fs::read_to_string("/proc/net/wireless").ok().as_deref().and_then(parse_proc_wireless) {
        return Some(v);
    }
    // Termux on Android (the Termux:API add-on).
    let j: serde_json::Value = serde_json::from_str(&quiet("termux-wifi-connectioninfo", &[])?).ok()?;
    j["rssi"].as_f64().filter(|r| *r < 0.0)
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
    csi: VecDeque<Sample>,
    nodes: std::collections::BTreeSet<u8>,
    /// The quietest motion seen: the empty room.
    floor: Option<f64>,
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
            let w: Vec<f64> = self.rssi.iter().filter(|s| now - s.t <= 6.0).map(|s| s.value).collect();
            (w.len() >= 3).then(|| std_dev(w.iter().copied()))
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
        let source = if csi { format!("ESP32 CSI ({} node{})", self.nodes.len(), if self.nodes.len() == 1 { "" } else { "s" }) } else { "this computer's WiFi".to_string() };

        let raw = self.raw_motion(now, csi);
        // The floor rises slowly (a room changes) and drops at once to a quieter reading.
        if let Some(r) = raw {
            self.floor = Some(match self.floor {
                Some(f) if r < f => r,
                Some(f) => f + (r - f) * 0.002,
                None => r,
            });
        }
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
        } else if self.last_motion_at.is_some_and(|t| now - t < 15.0) {
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
            let mut last = Reading { source: String::new(), csi: false, rate_hz: 0.0, seconds: 0.0, level: None, motion: 0.0, presence: Presence::Learning, breathing: None, motion_history: Vec::new(), signal_history: Vec::new() };
            let mut tick = 0u32;
            let mut wifi = true;
            loop {
                let now = started.elapsed().as_secs_f64();
                if STOP.load(Ordering::SeqCst) || seconds.is_some_and(|s| now >= s) {
                    STOP.store(true, Ordering::SeqCst);
                    return last;
                }
                // The computer's WiFi about twice a second.
                if wifi && tick.is_multiple_of(2) {
                    match tokio::task::spawn_blocking(host_rssi).await.ok().flatten() {
                        Some(v) => {
                            if let Ok(mut a) = an.lock() {
                                a.push_rssi(started.elapsed().as_secs_f64(), v);
                            }
                        }
                        None if tick == 0 => wifi = false,
                        None => {}
                    }
                }
                if let Ok(mut a) = an.lock() {
                    last = a.reading(started.elapsed().as_secs_f64(), now);
                }
                if !wifi && last.signal_history.is_empty() {
                    last.source = format!("waiting for ESP32 CSI on UDP port {CSI_PORT} (this computer is not on WiFi)");
                }
                on(Event::Reading(last.clone()));
                tick += 1;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    };
    let (_, last) = tokio::join!(listen, sample);
    on(Event::End);
    Ok(last)
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
