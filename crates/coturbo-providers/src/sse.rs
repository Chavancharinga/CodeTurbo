//! Minimal Server-Sent Events framing over raw bytes.

/// Incremental SSE parser. Feed it network chunks of any size; it yields
/// complete `(event, data)` pairs. Buffers bytes, so a multi-byte UTF-8 char
/// split across chunks is handled.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<(Option<String>, String)> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some((end, sep)) = find_boundary(&self.buf) {
            let frame: Vec<u8> = self.buf.drain(..end + sep).take(end).collect();
            if let Some(ev) = parse_frame(&String::from_utf8_lossy(&frame)) {
                out.push(ev);
            }
        }
        out
    }
}

/// Position and length of the first blank-line separator (`\n\n` or `\r\n\r\n`).
fn find_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    (0..buf.len()).find_map(|i| {
        if buf[i..].starts_with(b"\r\n\r\n") {
            Some((i, 4))
        } else if buf[i..].starts_with(b"\n\n") {
            Some((i, 2))
        } else {
            None
        }
    })
}

fn parse_frame(frame: &str) -> Option<(Option<String>, String)> {
    let mut event = None;
    let mut data: Vec<&str> = Vec::new();
    for line in frame.lines() {
        if let Some(v) = line.strip_prefix("event:") {
            event = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("data:") {
            data.push(v.strip_prefix(' ').unwrap_or(v));
        }
    }
    (!data.is_empty()).then(|| (event, data.join("\n")))
}
