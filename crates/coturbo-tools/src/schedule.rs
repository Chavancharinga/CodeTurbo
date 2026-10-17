//! Scheduled tasks: "every day at 23:00 turn off the TV", "in 20 minutes run
//! the tests". Kept in the project (`.codeturbo/schedule.json`) and run by
//! CodeTurbo when they are due, while it is open.

use crate::{Res, ToolOutput, arg};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: u64,
    pub task: String,
    /// Unix seconds of the next run.
    pub next: u64,
    /// Seconds between runs; 0 runs once.
    pub every: u64,
    pub when: String,
}

fn file(root: &Path) -> PathBuf {
    root.join(".codeturbo").join("schedule.json")
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Seconds east of UTC of this machine's clock, for "at 23:00".
fn utc_offset() -> i64 {
    // The difference between local midnight math and UTC: read from the
    // environment when set (TZ_OFFSET_MINUTES), else assume the system clock
    // reports local time through chrono-less means: use 0 when unknown.
    std::env::var("CODETURBO_TZ_OFFSET_MINUTES").ok().and_then(|m| m.parse::<i64>().ok()).map_or_else(local_offset, |m| m * 60)
}

#[cfg(windows)]
fn local_offset() -> i64 {
    // GetTimeZoneInformation's bias is minutes west of UTC.
    #[repr(C)]
    struct Tz {
        bias: i32,
        rest: [u8; 168],
    }
    unsafe extern "system" {
        fn GetTimeZoneInformation(tz: *mut Tz) -> u32;
    }
    let mut tz = Tz { bias: 0, rest: [0; 168] };
    // SAFETY: a plain out-parameter struct of the documented size.
    let kind = unsafe { GetTimeZoneInformation(&mut tz) };
    let daylight = if kind == 2 { i32::from_le_bytes([tz.rest[164], tz.rest[165], tz.rest[166], tz.rest[167]]) } else { 0 };
    -i64::from(tz.bias + daylight) * 60
}

#[cfg(not(windows))]
fn local_offset() -> i64 {
    // `date +%z` gives +hhmm.
    std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let sign = if s.starts_with('-') { -1 } else { 1 };
            let d = s.trim_start_matches(['+', '-']);
            let (h, m) = (d.get(0..2)?.parse::<i64>().ok()?, d.get(2..4)?.parse::<i64>().ok()?);
            Some(sign * (h * 3600 + m * 60))
        })
        .unwrap_or(0)
}

/// `(next run, interval)` for `when`, from `now` (Unix seconds) with the
/// local clock `offset` seconds east of UTC.
/// Takes: "in 20m", "in 2h", "every 30m", "every 2h", "at 23:00",
/// "every day at 23:00", "daily 7:30".
pub fn parse_when(when: &str, now: u64, offset: i64) -> Option<(u64, u64)> {
    let w = when.trim().to_lowercase();
    let span = |s: &str| -> Option<u64> {
        let s = s.trim();
        let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
        let n: u64 = n.parse().ok()?;
        Some(n * match unit.trim() {
            "s" | "sec" | "seconds" => 1,
            "m" | "min" | "mins" | "minutes" | "minute" => 60,
            "h" | "hour" | "hours" => 3600,
            "d" | "day" | "days" => 86_400,
            _ => return None,
        })
    };
    let clock = |s: &str| -> Option<u64> {
        let (h, m) = s.trim().split_once(':')?;
        let (h, m): (u64, u64) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
        (h < 24 && m < 60).then_some(h * 3600 + m * 60)
    };
    let next_at = |secs: u64| -> u64 {
        let local = now as i64 + offset;
        let day_start = local - local.rem_euclid(86_400);
        let mut t = day_start + secs as i64;
        if t <= local {
            t += 86_400;
        }
        (t - offset) as u64
    };
    if let Some(rest) = w.strip_prefix("in ") {
        return Some((now + span(rest)?, 0));
    }
    if let Some(rest) = w.strip_prefix("every day at ").or_else(|| w.strip_prefix("daily at ")).or_else(|| w.strip_prefix("daily ")) {
        return Some((next_at(clock(rest)?), 86_400));
    }
    if let Some(rest) = w.strip_prefix("every ") {
        let s = span(rest)?;
        return Some((now + s, s));
    }
    if let Some(rest) = w.strip_prefix("at ") {
        return Some((next_at(clock(rest)?), 0));
    }
    None
}

pub fn load(root: &Path) -> Vec<Entry> {
    let v: Value = std::fs::read_to_string(file(root)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!([]));
    v.as_array()
        .into_iter()
        .flatten()
        .map(|e| Entry {
            id: e["id"].as_u64().unwrap_or(0),
            task: e["task"].as_str().unwrap_or("").to_string(),
            next: e["next"].as_u64().unwrap_or(0),
            every: e["every"].as_u64().unwrap_or(0),
            when: e["when"].as_str().unwrap_or("").to_string(),
        })
        .collect()
}

fn save(root: &Path, all: &[Entry]) -> Result<(), String> {
    let v: Vec<Value> = all.iter().map(|e| json!({"id": e.id, "task": e.task, "next": e.next, "every": e.every, "when": e.when})).collect();
    std::fs::create_dir_all(root.join(".codeturbo")).map_err(|e| e.to_string())?;
    std::fs::write(file(root), serde_json::to_string_pretty(&v).unwrap_or_default()).map_err(|e| e.to_string())
}

/// The tasks due now; repeating ones move to their next time, the others go.
pub fn take_due(root: &Path) -> Vec<Entry> {
    let mut all = load(root);
    let t = now();
    let due: Vec<Entry> = all.iter().filter(|e| e.next <= t).cloned().collect();
    if due.is_empty() {
        return due;
    }
    all.retain(|e| e.next > t || e.every > 0);
    for e in all.iter_mut().filter(|e| e.next <= t) {
        while e.next <= t {
            e.next += e.every;
        }
    }
    let _ = save(root, &all);
    due
}

fn describe(e: &Entry) -> String {
    let left = e.next.saturating_sub(now());
    let soon = if left < 3600 { format!("in {} min", left / 60) } else { format!("in {}h{:02}", left / 3600, left % 3600 / 60) };
    format!("#{} · {} · next {soon} · {}", e.id, e.when, e.task)
}

/// `schedule`: add, list or cancel.
pub(crate) fn tool(root: &Path, input: &Value) -> Res {
    match input["action"].as_str().unwrap_or("add") {
        "list" => {
            let all = load(root);
            Ok(ToolOutput::ok(if all.is_empty() { "Nothing scheduled.".into() } else { all.iter().map(describe).collect::<Vec<_>>().join("\n") }))
        }
        "cancel" => {
            let id = input["id"].as_u64().ok_or("cancel needs the `id` from list")?;
            let mut all = load(root);
            let before = all.len();
            all.retain(|e| e.id != id);
            if all.len() == before {
                return Err(format!("nothing scheduled with id {id}"));
            }
            save(root, &all)?;
            Ok(ToolOutput::ok(format!("Cancelled #{id}")))
        }
        _ => {
            let when = arg(input, "when")?;
            let task = arg(input, "task")?;
            let (next, every) = parse_when(when, now(), utc_offset())
                .ok_or_else(|| format!("`{when}` is not a time I understand: use in 20m, in 2h, every 30m, at 23:00 or every day at 23:00"))?;
            let mut all = load(root);
            let id = all.iter().map(|e| e.id).max().unwrap_or(0) + 1;
            let e = Entry { id, task: task.to_string(), next, every, when: when.to_string() };
            let text = describe(&e);
            all.push(e);
            save(root, &all)?;
            Ok(ToolOutput::ok(format!("Scheduled {text}. It runs while CodeTurbo is open in this project.")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn understands_times() {
        let now = 1_000_000; // a Monday, 13:46:40 UTC
        assert_eq!(parse_when("in 20m", now, 0), Some((now + 1200, 0)));
        assert_eq!(parse_when("every 2h", now, 0), Some((now + 7200, 7200)));
        let (next, every) = parse_when("every day at 23:00", now, 0).unwrap();
        assert_eq!(every, 86_400);
        assert_eq!(next % 86_400, 23 * 3600, "23:00 UTC");
        assert!(next > now && next - now < 86_400);
        // 23:00 in UTC+1 is 22:00 UTC.
        let (next, _) = parse_when("at 23:00", now, 3600).unwrap();
        assert_eq!(next % 86_400, 22 * 3600);
        // A time already past today is tomorrow.
        let (next, _) = parse_when("at 10:00", now, 0).unwrap();
        assert!(next - now > 12 * 3600);
        assert_eq!(parse_when("whenever", now, 0), None);
    }

    #[test]
    fn due_tasks_are_taken_and_repeating_ones_move_on() {
        let dir = tempfile::tempdir().unwrap();
        let t = now();
        save(dir.path(), &[
            Entry { id: 1, task: "once".into(), next: t - 5, every: 0, when: "in 1m".into() },
            Entry { id: 2, task: "repeat".into(), next: t - 5, every: 60, when: "every 1m".into() },
            Entry { id: 3, task: "later".into(), next: t + 600, every: 0, when: "in 10m".into() },
        ])
        .unwrap();
        let due: Vec<String> = take_due(dir.path()).into_iter().map(|e| e.task).collect();
        assert_eq!(due, vec!["once", "repeat"]);
        let left: Vec<(String, bool)> = load(dir.path()).into_iter().map(|e| (e.task, e.next > t)).collect();
        assert_eq!(left, vec![("repeat".to_string(), true), ("later".to_string(), true)]);
    }
}
