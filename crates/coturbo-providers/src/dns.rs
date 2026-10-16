//! Name lookup where the C library cannot do it. The Linux build is a static
//! musl binary; on Android (Termux) there is no `/etc/resolv.conf`, so musl
//! asks 127.0.0.1 and every lookup fails. There, names are asked directly to
//! the servers in Termux's own `resolv.conf`, or to public ones.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// A `reqwest::ClientBuilder` that can resolve names on this system.
pub fn client_builder() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    if needs_own_resolver() {
        return builder.dns_resolver(std::sync::Arc::new(Direct));
    }
    builder
}

fn needs_own_resolver() -> bool {
    // The Android build uses the system's own lookup (bionic), which works.
    cfg!(target_os = "linux") && !std::path::Path::new("/etc/resolv.conf").exists()
}

/// The name servers to ask: Termux's list, else Google's and Cloudflare's.
fn servers() -> Vec<IpAddr> {
    let prefix = std::env::var("PREFIX").unwrap_or_else(|_| "/data/data/com.termux/files/usr".into());
    let listed: Vec<IpAddr> = std::fs::read_to_string(format!("{prefix}/etc/resolv.conf"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().strip_prefix("nameserver"))
        .filter_map(|ip| ip.trim().parse().ok())
        .collect();
    if listed.is_empty() {
        vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))]
    } else {
        listed
    }
}

struct Direct;

impl reqwest::dns::Resolve for Direct {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let ips = lookup(&host).await?;
            let addrs: reqwest::dns::Addrs = Box::new(ips.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

/// The addresses of `host`: IPv4 first, IPv6 if there is none.
pub async fn lookup(host: &str) -> Result<Vec<IpAddr>, Box<dyn std::error::Error + Send + Sync>> {
    if host == "localhost" {
        return Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]);
    }
    let mut last = String::from("no name server answered");
    for server in servers() {
        for qtype in [1u16, 28] {
            match ask(server, host, qtype).await {
                Ok(ips) if !ips.is_empty() => return Ok(ips),
                Ok(_) => {}
                Err(e) => last = e,
            }
        }
    }
    Err(format!("could not resolve {host}: {last}").into())
}

async fn ask(server: IpAddr, host: &str, qtype: u16) -> Result<Vec<IpAddr>, String> {
    let bind: SocketAddr = if server.is_ipv4() { "0.0.0.0:0".parse().unwrap() } else { "[::]:0".parse().unwrap() };
    let socket = tokio::net::UdpSocket::bind(bind).await.map_err(|e| e.to_string())?;
    let id = (std::process::id() as u16) ^ (host.len() as u16).rotate_left(7) ^ qtype;
    socket.send_to(&query(id, host, qtype), SocketAddr::new(server, 53)).await.map_err(|e| e.to_string())?;
    let mut buf = [0u8; 1500];
    let n = tokio::time::timeout(Duration::from_secs(3), socket.recv(&mut buf))
        .await
        .map_err(|_| format!("{server} did not answer"))?
        .map_err(|e| e.to_string())?;
    parse_answer(&buf[..n], id, qtype).ok_or_else(|| format!("unreadable answer from {server}"))
}

/// A standard recursive query for `host`.
pub fn query(id: u16, host: &str, qtype: u16) -> Vec<u8> {
    let mut q = Vec::with_capacity(32 + host.len());
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in host.trim_end_matches('.').split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&qtype.to_be_bytes());
    q.extend_from_slice(&[0, 1]);
    q
}

/// The addresses of type `qtype` in a response to query `id`.
pub fn parse_answer(msg: &[u8], id: u16, qtype: u16) -> Option<Vec<IpAddr>> {
    if msg.len() < 12 || u16::from_be_bytes([msg[0], msg[1]]) != id {
        return None;
    }
    let questions = u16::from_be_bytes([msg[4], msg[5]]);
    let answers = u16::from_be_bytes([msg[6], msg[7]]);
    let mut at = 12;
    for _ in 0..questions {
        at = skip_name(msg, at)? + 4;
    }
    let mut out = Vec::new();
    for _ in 0..answers {
        at = skip_name(msg, at)?;
        let head = msg.get(at..at + 10)?;
        let kind = u16::from_be_bytes([head[0], head[1]]);
        let len = u16::from_be_bytes([head[8], head[9]]) as usize;
        let data = msg.get(at + 10..at + 10 + len)?;
        match (kind, len) {
            (1, 4) if qtype == 1 => out.push(IpAddr::V4(Ipv4Addr::new(data[0], data[1], data[2], data[3]))),
            (28, 16) if qtype == 28 => {
                let b: [u8; 16] = data.try_into().ok()?;
                out.push(IpAddr::V6(Ipv6Addr::from(b)));
            }
            _ => {}
        }
        at += 10 + len;
    }
    Some(out)
}

/// The offset just past a (possibly compressed) name.
fn skip_name(msg: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let len = *msg.get(at)?;
        if len & 0xC0 == 0xC0 {
            return Some(at + 2);
        }
        at += 1;
        if len == 0 {
            return Some(at);
        }
        at += len as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_records_after_a_cname() {
        let q = query(7, "api.example.com", 1);
        let mut msg = q.clone();
        msg[2] = 0x81;
        msg[3] = 0x80;
        msg[7] = 2; // two answers
        // CNAME pointing elsewhere, then the A record.
        msg.extend_from_slice(&[0xC0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 2, 0xC0, 12]);
        msg.extend_from_slice(&[0xC0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 93, 184, 216, 34]);
        assert_eq!(parse_answer(&msg, 7, 1), Some(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]));
        assert_eq!(parse_answer(&msg, 8, 1), None, "another query's answer");
    }

    /// Needs the network: `cargo test -p coturbo-providers dns -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn resolves_a_real_name() {
        let ips = lookup("openrouter.ai").await.expect("resolved");
        assert!(!ips.is_empty());
    }
}
