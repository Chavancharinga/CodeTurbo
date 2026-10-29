//! The bits of the DNS wire format mDNS needs: building a query and reading
//! every record of an answer, compressed names included.

use std::net::{Ipv4Addr, Ipv6Addr};

pub const PTR: u16 = 12;
pub const TXT: u16 = 16;
pub const SRV: u16 = 33;
pub const A: u16 = 1;
pub const AAAA: u16 = 28;

#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    Ptr(String),
    Srv { port: u16, target: String },
    Txt(Vec<String>),
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub name: String,
    pub data: Data,
}

/// An mDNS query for `names` of type `qtype`, asking for unicast answers
/// (so they come back to our own port, not 5353).
pub fn query(names: &[&str], qtype: u16) -> Vec<u8> {
    let mut q = vec![0, 0, 0, 0];
    q.extend_from_slice(&(names.len() as u16).to_be_bytes());
    q.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    for name in names {
        for label in name.trim_end_matches('.').split('.') {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&qtype.to_be_bytes());
        q.extend_from_slice(&0x8001u16.to_be_bytes());
    }
    q
}

/// Every answer, authority and additional record of a message.
pub fn records(msg: &[u8]) -> Vec<Record> {
    parse(msg).unwrap_or_default()
}

fn parse(msg: &[u8]) -> Option<Vec<Record>> {
    let count = |i: usize| u16::from_be_bytes([msg[i], msg[i + 1]]) as usize;
    if msg.len() < 12 {
        return None;
    }
    let (qd, total) = (count(4), count(6) + count(8) + count(10));
    let mut at = 12;
    for _ in 0..qd {
        at = read_name(msg, at)?.1 + 4;
    }
    let mut out = Vec::new();
    for _ in 0..total {
        let (name, next) = read_name(msg, at)?;
        let head = msg.get(next..next + 10)?;
        let kind = u16::from_be_bytes([head[0], head[1]]);
        let len = u16::from_be_bytes([head[8], head[9]]) as usize;
        let start = next + 10;
        let body = msg.get(start..start + len)?;
        let data = match kind {
            PTR => Data::Ptr(read_name(msg, start)?.0),
            SRV if len >= 6 => Data::Srv { port: u16::from_be_bytes([body[4], body[5]]), target: read_name(msg, start + 6)?.0 },
            TXT => {
                let mut items = Vec::new();
                let mut i = 0;
                while i < body.len() {
                    let n = body[i] as usize;
                    if let Some(s) = body.get(i + 1..i + 1 + n) {
                        items.push(String::from_utf8_lossy(s).into_owned());
                    }
                    i += 1 + n;
                }
                Data::Txt(items)
            }
            A if len == 4 => Data::A(Ipv4Addr::new(body[0], body[1], body[2], body[3])),
            AAAA if len == 16 => Data::Aaaa(Ipv6Addr::from(<[u8; 16]>::try_from(body).ok()?)),
            _ => Data::Other,
        };
        out.push(Record { name, data });
        at = start + len;
    }
    Some(out)
}

/// The name at `at` and the offset right after it (pointers followed).
pub fn read_name(msg: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut end = None;
    for _ in 0..64 {
        let len = *msg.get(at)? as usize;
        if len & 0xC0 == 0xC0 {
            let ptr = ((len & 0x3F) << 8) | *msg.get(at + 1)? as usize;
            end.get_or_insert(at + 2);
            at = ptr;
            continue;
        }
        if len == 0 {
            return Some((labels.join("."), end.unwrap_or(at + 1)));
        }
        labels.push(String::from_utf8_lossy(msg.get(at + 1..at + 1 + len)?).into_owned());
        at += 1 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(out: &mut Vec<u8>, n: &str) {
        for l in n.split('.') {
            out.push(l.len() as u8);
            out.extend_from_slice(l.as_bytes());
        }
        out.push(0);
    }

    #[test]
    fn reads_an_mdns_answer_with_compressed_names() {
        let mut m = vec![0, 0, 0x84, 0, 0, 0, 0, 3, 0, 0, 0, 0];
        // PTR _googlecast._tcp.local -> Living-Room._googlecast._tcp.local
        let svc = m.len();
        name(&mut m, "_googlecast._tcp.local");
        m.extend_from_slice(&[0, 12, 0, 1, 0, 0, 0, 120]);
        let mut rdata = vec![11];
        rdata.extend_from_slice(b"Living-Room");
        rdata.extend_from_slice(&[0xC0, svc as u8]);
        m.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        let inst = m.len();
        m.extend_from_slice(&rdata);
        // SRV instance -> tv.local:8009
        m.extend_from_slice(&[0xC0, inst as u8, 0, 33, 0x80, 1, 0, 0, 0, 120]);
        let mut srv = vec![0, 0, 0, 0, 0x1F, 0x49];
        name(&mut srv, "tv.local");
        m.extend_from_slice(&(srv.len() as u16).to_be_bytes());
        m.extend_from_slice(&srv);
        // A tv.local -> 192.168.1.20
        name(&mut m, "tv.local");
        m.extend_from_slice(&[0, 1, 0x80, 1, 0, 0, 0, 120, 0, 4, 192, 168, 1, 20]);
        let r = records(&m);
        assert_eq!(r[0].data, Data::Ptr("Living-Room._googlecast._tcp.local".into()));
        assert_eq!(r[1].name, "Living-Room._googlecast._tcp.local");
        assert_eq!(r[1].data, Data::Srv { port: 8009, target: "tv.local".into() });
        assert_eq!(r[2].data, Data::A(Ipv4Addr::new(192, 168, 1, 20)));
    }

    #[test]
    fn a_query_asks_for_unicast_answers() {
        let q = query(&["_services._dns-sd._udp.local"], PTR);
        assert_eq!(&q[4..6], &[0, 1]);
        assert_eq!(&q[q.len() - 4..], &[0, 12, 0x80, 1]);
    }
}
