//! Printers over IPP (port 631): state and ink/toner levels, from the
//! `Get-Printer-Attributes` request every network printer answers.

use super::Act;
use std::collections::HashMap;
use std::net::IpAddr;

/// One attribute value as IPP encodes it.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Int(i32),
    Text(String),
}

fn attr(out: &mut Vec<u8>, tag: u8, name: &str, value: &str) {
    out.push(tag);
    out.extend_from_slice(&(name.len() as u16).to_be_bytes());
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// A `Get-Printer-Attributes` request for the printer at `uri`.
pub fn request(uri: &str) -> Vec<u8> {
    let mut m = vec![0x02, 0x00, 0x00, 0x0B, 0, 0, 0, 1, 0x01];
    attr(&mut m, 0x47, "attributes-charset", "utf-8");
    attr(&mut m, 0x48, "attributes-natural-language", "en");
    attr(&mut m, 0x45, "printer-uri", uri);
    let wanted = ["printer-state", "printer-state-reasons", "printer-make-and-model", "printer-name", "marker-names", "marker-levels", "marker-types"];
    for (i, w) in wanted.iter().enumerate() {
        attr(&mut m, 0x44, if i == 0 { "requested-attributes" } else { "" }, w);
    }
    m.push(0x03);
    m
}

/// Every attribute of an IPP response, by name.
pub fn parse(body: &[u8]) -> HashMap<String, Vec<Val>> {
    let mut out: HashMap<String, Vec<Val>> = HashMap::new();
    let mut i = 8;
    let mut last = String::new();
    while i < body.len() {
        let tag = body[i];
        i += 1;
        if tag == 0x03 {
            break;
        }
        if tag < 0x10 {
            continue;
        }
        let read_len = |i: &mut usize| -> Option<usize> {
            let b = body.get(*i..*i + 2)?;
            *i += 2;
            Some(u16::from_be_bytes([b[0], b[1]]) as usize)
        };
        let Some(nlen) = read_len(&mut i) else { break };
        let Some(name) = body.get(i..i + nlen) else { break };
        i += nlen;
        let Some(vlen) = read_len(&mut i) else { break };
        let Some(value) = body.get(i..i + vlen) else { break };
        i += vlen;
        if nlen > 0 {
            last = String::from_utf8_lossy(name).into_owned();
        }
        let v = match tag {
            0x21 | 0x23 if vlen == 4 => Val::Int(i32::from_be_bytes([value[0], value[1], value[2], value[3]])),
            _ => Val::Text(String::from_utf8_lossy(value).into_owned()),
        };
        out.entry(last.clone()).or_default().push(v);
    }
    out
}

/// What the attributes say, for a person.
pub fn describe(a: &HashMap<String, Vec<Val>>) -> String {
    let text = |k: &str| -> Vec<String> {
        a.get(k).into_iter().flatten().filter_map(|v| if let Val::Text(t) = v { Some(t.clone()) } else { None }).collect()
    };
    let ints = |k: &str| -> Vec<i32> { a.get(k).into_iter().flatten().filter_map(|v| if let Val::Int(n) = v { Some(*n) } else { None }).collect() };
    let state = match ints("printer-state").first() {
        Some(3) => "ready",
        Some(4) => "printing",
        Some(5) => "stopped",
        _ => "state unknown",
    };
    let reasons: Vec<String> = text("printer-state-reasons").into_iter().filter(|r| r != "none").collect();
    let model = text("printer-make-and-model").into_iter().next().unwrap_or_else(|| "printer".into());
    let levels: Vec<String> = text("marker-names")
        .into_iter()
        .zip(ints("marker-levels"))
        .map(|(n, l)| if l < 0 { format!("{n} unknown") } else { format!("{n} {l}%") })
        .collect();
    let mut s = format!("{model} · {state}");
    if !reasons.is_empty() {
        s.push_str(&format!(" ({})", reasons.join(", ")));
    }
    if !levels.is_empty() {
        s.push_str(&format!(" · {}", levels.join(", ")));
    }
    s
}

pub(crate) async fn status(ip: IpAddr, act: &Act) -> Result<String, String> {
    if *act != Act::Status {
        return Err("printers are read here (state, ink and toner); printing goes through the system's own print dialog".into());
    }
    for path in ["/ipp/print", "/ipp/printer", "/ipp", "/printers/", "/"] {
        let uri = format!("ipp://{ip}:631{path}");
        let Ok(r) = super::http().post(format!("http://{ip}:631{path}")).header("Content-Type", "application/ipp").body(request(&uri)).send().await else { continue };
        if !r.status().is_success() {
            continue;
        }
        let Ok(body) = r.bytes().await else { continue };
        // Status code 0x0000-0x00FF is success.
        if body.len() > 8 && body[2] == 0 {
            let a = parse(&body);
            if !a.is_empty() {
                return Ok(describe(&a));
            }
        }
    }
    Err("the printer did not answer over IPP".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_state_and_ink_levels() {
        // A response: version, status ok, request id, then the attributes.
        let mut r = vec![0x02, 0x00, 0x00, 0x00, 0, 0, 0, 1, 0x04];
        let int = |r: &mut Vec<u8>, tag: u8, name: &str, n: i32| {
            r.push(tag);
            r.extend_from_slice(&(name.len() as u16).to_be_bytes());
            r.extend_from_slice(name.as_bytes());
            r.extend_from_slice(&4u16.to_be_bytes());
            r.extend_from_slice(&n.to_be_bytes());
        };
        int(&mut r, 0x23, "printer-state", 3);
        attr(&mut r, 0x41, "printer-make-and-model", "HP OfficeJet 8010");
        attr(&mut r, 0x41, "marker-names", "Black");
        attr(&mut r, 0x41, "", "Cyan");
        int(&mut r, 0x21, "marker-levels", 72);
        int(&mut r, 0x21, "", 8);
        r.push(0x03);
        assert_eq!(describe(&parse(&r)), "HP OfficeJet 8010 · ready · Black 72%, Cyan 8%");
        let q = request("ipp://192.168.1.9:631/ipp/print");
        assert_eq!(&q[2..4], &[0x00, 0x0B]);
        assert_eq!(*q.last().unwrap(), 0x03);
    }
}
