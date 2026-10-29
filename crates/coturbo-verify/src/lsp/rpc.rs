//! JSON-RPC framing for LSP over stdio: `Content-Length` headers + JSON body.

use serde_json::Value;
use std::io;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, msg: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    w.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    w.write_all(&body).await?;
    w.flush().await
}

/// Reads one message; `Ok(None)` at end of stream.
pub async fn read_msg<R: AsyncBufRead + Unpin>(r: &mut R) -> io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if len.is_some() {
                break;
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse().ok();
        }
    }
    let mut body = vec![0; len.unwrap_or(0)];
    r.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// `file://` URI for an absolute path, percent-encoding what URIs reserve.
pub fn path_to_uri(path: &std::path::Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").unwrap_or(&s).to_string();
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Comparable key for a `file://` URI or a path: decoded, forward slashes,
/// no leading slash before a drive letter, lowercase on Windows.
pub fn uri_key(uri_or_path: &str) -> String {
    let s = uri_or_path.strip_prefix("file://").unwrap_or(uri_or_path);
    let bytes = s.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            decoded.push(b);
            i += 3;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    let mut s = String::from_utf8_lossy(&decoded).replace('\\', "/");
    if s.len() > 2 && s.starts_with('/') && s.as_bytes()[2] == b':' {
        s.remove(0);
    }
    let s = s.strip_prefix("//?/").map(str::to_string).unwrap_or(s);
    if cfg!(windows) { s.to_lowercase() } else { s }
}
