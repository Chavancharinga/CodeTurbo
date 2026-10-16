//! Devices on the user's own network, natively: what is there (mDNS/Bonjour,
//! UPnP/SSDP, the ARP table, an optional quick check of common ports on the
//! local subnet) and how to talk to it (HTTP to a LAN address, UPnP actions,
//! Wake-on-LAN). No hub or home-automation server is needed. Control only
//! ever reaches private addresses, never the internet.

pub mod dns;
pub mod identify;
pub mod tv;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;

/// One device, by address.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Device {
    pub ip: Option<IpAddr>,
    pub mac: Option<String>,
    /// Names it announces (mDNS host, instance and UPnP friendly names).
    pub names: BTreeSet<String>,
    /// mDNS services: type, port and a few TXT entries.
    pub services: Vec<Service>,
    pub upnp: Vec<Upnp>,
    pub open_ports: BTreeSet<u16>,
    /// What it is and who made it, from all of the above.
    pub identity: identify::Identity,
    /// Its name in the router.
    pub host: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    pub kind: String,
    pub port: u16,
    pub txt: Vec<String>,
}

/// A UPnP device description: what it is and the actions it offers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Upnp {
    pub location: String,
    pub device_type: String,
    pub manufacturer: String,
    pub model: String,
    pub server: String,
    /// `(service type, absolute control URL)`.
    pub controls: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct Inventory {
    pub local: Option<Ipv4Addr>,
    pub devices: Vec<Device>,
}

/// What a scan looks at.
#[derive(Debug, Clone, Copy)]
pub struct ScanOptions {
    /// How long to listen for announcements.
    pub listen: Duration,
    /// Also try common ports on every address of the local /24.
    pub sweep: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { listen: Duration::from_millis(2500), sweep: false }
    }
}

/// Service types asked for even when a device does not list its services.
const COMMON_SERVICES: &[&str] = &[
    "_googlecast._tcp.local",
    "_airplay._tcp.local",
    "_raop._tcp.local",
    "_hap._tcp.local",
    "_homekit._tcp.local",
    "_spotify-connect._tcp.local",
    "_ipp._tcp.local",
    "_printer._tcp.local",
    "_pdl-datastream._tcp.local",
    "_http._tcp.local",
    "_hue._tcp.local",
    "_shelly._tcp.local",
    "_esphomelib._tcp.local",
    "_mqtt._tcp.local",
    "_sonos._tcp.local",
    "_androidtvremote2._tcp.local",
    "_amzn-wplay._tcp.local",
    "_smb._tcp.local",
    "_ssh._tcp.local",
    "_workstation._tcp.local",
    "_companion-link._tcp.local",
    "_matter._tcp.local",
];

/// Ports tried by the sweep: web, SSH, cameras, printers, casting, MQTT, hubs.
pub const SWEEP_PORTS: &[u16] = &[22, 80, 443, 554, 1400, 1883, 5000, 6668, 8008, 8009, 8080, 8123, 8443, 9100];

/// The address this computer uses on the local network.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    // Connecting a UDP socket sends nothing; it only picks the route.
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// Finds the devices on the local network.
pub async fn scan(opts: ScanOptions) -> Inventory {
    let local = local_ipv4();
    let (mdns, ssdp) = tokio::join!(mdns(local, opts.listen), ssdp(local, opts.listen));
    let upnp = describe_all(ssdp).await;
    let mut by_ip: BTreeMap<IpAddr, Device> = BTreeMap::new();
    for (ip, d) in mdns {
        merge(by_ip.entry(ip).or_default(), d);
    }
    for (ip, u) in upnp {
        let dev = by_ip.entry(ip).or_default();
        if let Some(name) = u.1 {
            dev.names.insert(name);
        }
        dev.upnp.push(u.0);
    }
    if opts.sweep
        && let Some(l) = local
    {
        let o = l.octets();
        let hosts: Vec<IpAddr> = (1..=254u8).map(|h| IpAddr::V4(Ipv4Addr::new(o[0], o[1], o[2], h))).filter(|ip| *ip != IpAddr::V4(l)).collect();
        for (ip, port) in probe(&hosts, SWEEP_PORTS).await {
            by_ip.entry(ip).or_default().open_ports.insert(port);
        }
    }
    // Read last: the sweep fills the system's ARP cache with every host that
    // answered at all, even one with no open port.
    for (ip, mac) in arp_table() {
        if local.is_some_and(|l| !same_subnet(l, ip)) {
            continue;
        }
        by_ip.entry(IpAddr::V4(ip)).or_default().mac = Some(mac);
    }
    let devices = by_ip
        .into_iter()
        .map(|(ip, mut d)| {
            d.ip = Some(ip);
            d
        })
        .filter(|d| Some(d.ip.unwrap()) != local.map(IpAddr::V4) || !d.services.is_empty())
        .collect();
    let mut inv = Inventory { local, devices };
    name_all(&mut inv).await;
    inv
}

/// Fills in what each device is: router names, makers, web page titles.
async fn name_all(inv: &mut Inventory) {
    use futures_util::StreamExt;
    let vendors = identify::vendors().await;
    let router = inv
        .devices
        .iter()
        .find(|d| d.upnp.iter().any(|u| u.device_type.contains("InternetGateway") || u.device_type.contains("WFADevice")))
        .and_then(|d| d.ip)
        .or_else(|| inv.local.map(|l| IpAddr::V4(Ipv4Addr::new(l.octets()[0], l.octets()[1], l.octets()[2], 1))));
    let ips: Vec<Option<IpAddr>> = inv.devices.iter().map(|d| d.ip).collect();
    let clues: Vec<(Option<String>, Option<String>)> = futures_util::stream::iter(ips)
        .map(|ip| async move {
            let Some(ip) = ip else { return (None, None) };
            let host = match (router, ip) {
                (Some(IpAddr::V4(r)), IpAddr::V4(v4)) => identify::host_name(r, v4).await,
                _ => None,
            };
            (host, identify::web_title(ip).await)
        })
        .buffered(32)
        .collect()
        .await;
    for (d, (host, title)) in inv.devices.iter_mut().zip(clues) {
        let vendor = d.mac.as_deref().and_then(|m| identify::vendor_of(&vendors, m));
        d.identity = identify::identify(d, vendor, host.as_deref(), title.as_deref());
        d.host = host;
    }
}

fn merge(into: &mut Device, from: Device) {
    into.names.extend(from.names);
    into.services.extend(from.services);
}

fn same_subnet(a: Ipv4Addr, b: Ipv4Addr) -> bool {
    a.octets()[..3] == b.octets()[..3]
}

/// A UDP socket on the LAN address, so multicast leaves through the network
/// the devices are on (not a VPN or virtual-machine adapter).
async fn lan_socket(local: Option<Ipv4Addr>) -> Option<UdpSocket> {
    let ip = local.unwrap_or(Ipv4Addr::UNSPECIFIED);
    let socket = match UdpSocket::bind((ip, 0)).await {
        Ok(s) => s,
        Err(_) => UdpSocket::bind("0.0.0.0:0").await.ok()?,
    };
    let _ = socket.set_multicast_ttl_v4(4);
    Some(socket)
}

async fn listen(socket: &UdpSocket, window: Duration, mut on: impl FnMut(&[u8], SocketAddr)) {
    let deadline = tokio::time::Instant::now() + window;
    let mut buf = vec![0u8; 9000];
    while let Ok(Ok((n, from))) = tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await {
        on(&buf[..n], from);
    }
}

/// Bonjour / mDNS: lists the service types announced, then asks for each.
async fn mdns(local: Option<Ipv4Addr>, window: Duration) -> HashMap<IpAddr, Device> {
    let Some(socket) = lan_socket(local).await else { return HashMap::new() };
    let group: SocketAddr = "224.0.0.251:5353".parse().expect("valid address");
    let mut records = Vec::new();
    let _ = socket.send_to(&dns::query(&["_services._dns-sd._udp.local"], dns::PTR), group).await;
    listen(&socket, window / 2, |m, _| records.extend(dns::records(m))).await;
    let mut types: BTreeSet<String> = COMMON_SERVICES.iter().map(|s| s.to_string()).collect();
    for r in &records {
        if r.name == "_services._dns-sd._udp.local"
            && let dns::Data::Ptr(t) = &r.data
        {
            types.insert(t.clone());
        }
    }
    let types: Vec<&str> = types.iter().map(String::as_str).collect();
    for chunk in types.chunks(12) {
        let _ = socket.send_to(&dns::query(chunk, dns::PTR), group).await;
    }
    listen(&socket, window, |m, _| records.extend(dns::records(m))).await;
    assemble(&records)
}

/// Devices from mDNS records: PTR (type → instance), SRV (instance → host,
/// port), TXT (instance details), A (host → address).
pub fn assemble(records: &[dns::Record]) -> HashMap<IpAddr, Device> {
    let mut hosts: HashMap<String, Vec<IpAddr>> = HashMap::new();
    let mut srv: HashMap<String, (String, u16)> = HashMap::new();
    let mut txt: HashMap<String, Vec<String>> = HashMap::new();
    let mut instances: BTreeSet<(String, String)> = BTreeSet::new();
    for r in records {
        match &r.data {
            dns::Data::A(ip) => hosts.entry(r.name.to_lowercase()).or_default().push(IpAddr::V4(*ip)),
            dns::Data::Srv { port, target } => {
                srv.insert(r.name.clone(), (target.to_lowercase(), *port));
            }
            dns::Data::Txt(t) => {
                txt.insert(r.name.clone(), t.clone());
            }
            dns::Data::Ptr(inst) if r.name.starts_with('_') && r.name != "_services._dns-sd._udp.local" => {
                instances.insert((r.name.clone(), inst.clone()));
            }
            _ => {}
        }
    }
    let mut out: HashMap<IpAddr, Device> = HashMap::new();
    for (kind, inst) in instances {
        let Some((target, port)) = srv.get(&inst) else { continue };
        let Some(ips) = hosts.get(target) else { continue };
        let Some(ip) = ips.first() else { continue };
        let dev = out.entry(*ip).or_default();
        let label = inst.strip_suffix(&format!(".{kind}")).unwrap_or(&inst).replace("\\032", " ");
        let txt = txt.get(&inst).cloned().unwrap_or_default();
        // Cast devices put their real name in `fn=`.
        if let Some(name) = txt.iter().find_map(|t| t.strip_prefix("fn=")) {
            dev.names.insert(name.to_string());
        } else {
            dev.names.insert(label);
        }
        dev.names.insert(target.trim_end_matches(".local").to_string());
        let txt = txt.into_iter().filter(|t| useful_txt(t)).take(6).collect();
        dev.services.push(Service { kind: kind.trim_end_matches(".local").to_string(), port: *port, txt });
    }
    out
}

/// TXT entries that tell what a device is.
fn useful_txt(t: &str) -> bool {
    let key = t.split('=').next().unwrap_or("").to_ascii_lowercase();
    matches!(key.as_str(), "md" | "model" | "ty" | "manufacturer" | "mf" | "fn" | "am" | "product" | "vendor" | "rp" | "path" | "ver" | "fv" | "id")
}

/// UPnP / SSDP: everyone that answers an M-SEARCH, by address, with the
/// description URL and server line.
async fn ssdp(local: Option<Ipv4Addr>, window: Duration) -> Vec<(IpAddr, String, String)> {
    let Some(socket) = lan_socket(local).await else { return Vec::new() };
    let msg = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: ssdp:all\r\n\r\n";
    for _ in 0..2 {
        let _ = socket.send_to(msg.as_bytes(), "239.255.255.250:1900").await;
    }
    let mut seen = BTreeSet::new();
    listen(&socket, window, |m, from| {
        let text = String::from_utf8_lossy(m);
        let header = |key: &str| {
            text.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim().eq_ignore_ascii_case(key).then(|| v.trim().to_string())
            })
        };
        if let Some(loc) = header("location") {
            seen.insert((from.ip(), loc, header("server").unwrap_or_default()));
        }
    })
    .await;
    seen.into_iter().collect()
}

async fn describe_all(found: Vec<(IpAddr, String, String)>) -> Vec<(IpAddr, (Upnp, Option<String>))> {
    use futures_util::StreamExt;
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(3)).no_proxy().build() else { return Vec::new() };
    futures_util::stream::iter(found)
        .map(|(ip, loc, server)| {
            let client = client.clone();
            async move {
                let body = match client.get(&loc).send().await {
                    Ok(r) => r.text().await.unwrap_or_default(),
                    Err(_) => String::new(),
                };
                let (mut u, name) = parse_description(&loc, &body);
                u.server = server;
                (ip, (u, name))
            }
        })
        .buffer_unordered(16)
        .collect()
        .await
}

fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{name}>"))?;
    Some(xml[start..start + end].trim())
}

/// A UPnP description document: the device and its control URLs.
pub fn parse_description(location: &str, xml: &str) -> (Upnp, Option<String>) {
    let base = tag(xml, "URLBase").map(str::to_string).unwrap_or_else(|| origin(location));
    let mut controls = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<service>") {
        let block_end = rest[i..].find("</service>").map_or(rest.len(), |e| i + e);
        let block = &rest[i..block_end];
        if let (Some(kind), Some(ctl)) = (tag(block, "serviceType"), tag(block, "controlURL")) {
            controls.push((kind.to_string(), absolute(&base, ctl)));
        }
        rest = &rest[block_end..];
        if rest.len() <= "</service>".len() {
            break;
        }
        rest = &rest["</service>".len()..];
    }
    let u = Upnp {
        location: location.to_string(),
        device_type: tag(xml, "deviceType").unwrap_or("").to_string(),
        manufacturer: tag(xml, "manufacturer").unwrap_or("").to_string(),
        model: [tag(xml, "modelName"), tag(xml, "modelNumber")].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" "),
        server: String::new(),
        controls,
    };
    (u, tag(xml, "friendlyName").map(str::to_string))
}

fn origin(url: &str) -> String {
    let after = url.find("://").map_or(0, |i| i + 3);
    let end = url[after..].find('/').map_or(url.len(), |i| after + i);
    url[..end].to_string()
}

fn absolute(base: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else if url.starts_with('/') {
        format!("{}{url}", origin(base))
    } else {
        format!("{}/{url}", base.trim_end_matches('/'))
    }
}

/// `(address, MAC)` pairs the system already knows (its ARP cache).
pub fn arp_table() -> Vec<(Ipv4Addr, String)> {
    let text = if cfg!(target_os = "linux") {
        std::fs::read_to_string("/proc/net/arp").unwrap_or_default()
    } else {
        let args: &[&str] = if cfg!(windows) { &["-a"] } else { &["-an"] };
        std::process::Command::new("arp").args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    };
    parse_arp(&text)
}

/// Reads `arp -a` (Windows, macOS) or `/proc/net/arp` (Linux).
pub fn parse_arp(text: &str) -> Vec<(Ipv4Addr, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let ip = words.iter().find_map(|w| w.trim_matches(|c| c == '(' || c == ')').parse::<Ipv4Addr>().ok());
        let mac = words.iter().find_map(|w| normalize_mac(w));
        if let (Some(ip), Some(mac)) = (ip, mac) {
            let o = ip.octets();
            let group = o[0] >= 224 || o[3] == 255;
            if !group && mac != "ff:ff:ff:ff:ff:ff" && mac != "00:00:00:00:00:00" {
                out.push((ip, mac));
            }
        }
    }
    out
}

/// `aa-bb-cc-dd-ee-ff`, `aa:bb:..` or `a:b:..` as `aa:bb:cc:dd:ee:ff`.
pub fn normalize_mac(w: &str) -> Option<String> {
    let parts: Vec<&str> = w.split([':', '-']).collect();
    if parts.len() != 6 || !parts.iter().all(|p| (1..=2).contains(&p.len()) && p.chars().all(|c| c.is_ascii_hexdigit())) {
        return None;
    }
    Some(parts.iter().map(|p| format!("{:0>2}", p.to_ascii_lowercase())).collect::<Vec<_>>().join(":"))
}

/// The `(address, port)` pairs that accept a connection.
async fn probe(hosts: &[IpAddr], ports: &[u16]) -> Vec<(IpAddr, u16)> {
    use futures_util::StreamExt;
    let pairs: Vec<(IpAddr, u16)> = hosts.iter().flat_map(|h| ports.iter().map(move |p| (*h, *p))).collect();
    futures_util::stream::iter(pairs)
        .map(|(ip, port)| async move {
            let ok = matches!(tokio::time::timeout(Duration::from_millis(400), tokio::net::TcpStream::connect((ip, port))).await, Ok(Ok(_)));
            ok.then_some((ip, port))
        })
        .buffer_unordered(256)
        .filter_map(|x| async move { x })
        .collect()
        .await
}

/// The inventory as text for the model and the user.
pub fn render(inv: &Inventory) -> String {
    let mut out = String::new();
    match inv.local {
        Some(ip) => out.push_str(&format!("This computer: {ip}\n")),
        None => out.push_str("This computer has no local IPv4 network.\n"),
    }
    if inv.devices.is_empty() {
        out.push_str("No devices answered. Devices may be asleep, on another network, or blocked by the firewall; try again with sweep=true.\n");
        return out;
    }
    out.push_str(&format!("{} device(s):\n", inv.devices.len()));
    for d in &inv.devices {
        let ip = d.ip.map(|i| i.to_string()).unwrap_or_default();
        let names: Vec<&str> = d.names.iter().map(String::as_str).filter(|n| !n.is_empty()).collect();
        let id = &d.identity;
        let what = [id.kind.as_str(), id.brand.as_str(), id.model.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
        out.push_str(&format!("\n{ip}  {}", if what.is_empty() { "unidentified device" } else { &what }));
        if !id.name.is_empty() {
            out.push_str(&format!("  \"{}\"", id.name));
        }
        out.push('\n');
        let mut also = Vec::new();
        if let Some(mac) = &d.mac {
            also.push(format!("mac {mac}"));
        }
        if let Some(h) = &d.host {
            also.push(format!("router name {h}"));
        }
        if !names.is_empty() {
            also.push(format!("names {}", names.join(" / ")));
        }
        if !id.control.is_empty() {
            also.push(format!("control with: {}", id.control));
        }
        if !also.is_empty() {
            out.push_str(&format!("  {}\n", also.join(" · ")));
        }
        for s in &d.services {
            let txt = if s.txt.is_empty() { String::new() } else { format!("  {}", s.txt.join(" ")) };
            out.push_str(&format!("  mdns {} port {}{txt}\n", s.kind, s.port));
        }
        for u in &d.upnp {
            let what = [u.manufacturer.as_str(), u.model.as_str(), u.device_type.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
            out.push_str(&format!("  upnp {what}  ({})\n", u.location));
            for (kind, url) in &u.controls {
                out.push_str(&format!("    action service {kind} at {url}\n"));
            }
        }
        if !d.open_ports.is_empty() {
            let ports: Vec<String> = d.open_ports.iter().map(u16::to_string).collect();
            out.push_str(&format!("  open ports {}\n", ports.join(", ")));
        }
    }
    out
}

/// Whether `ip` is on a private network (never the internet).
pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_link_local() || v.is_loopback(),
        IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// The host of `url` resolved, refused unless every address is private.
async fn private_host(url: &str) -> Result<(), String> {
    let after = url.find("://").map(|i| i + 3).ok_or("the address must start with http:// or https://")?;
    let authority = url[after..].split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let (host, port) = if let Some(rest) = host_port.strip_prefix('[') {
        let (h, p) = rest.split_once(']').ok_or("bad IPv6 address")?;
        (h.to_string(), p.trim_start_matches(':').parse().unwrap_or(80))
    } else {
        match host_port.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h.to_string(), p.parse().unwrap_or(80)),
            _ => (host_port.to_string(), 80u16),
        }
    };
    let ips: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => tokio::net::lookup_host((host.as_str(), port)).await.map_err(|e| format!("could not find {host}: {e}"))?.map(|a| a.ip()).collect(),
    };
    if ips.is_empty() || !ips.iter().all(|ip| is_private(*ip)) {
        return Err(format!("{host} is not on the local network: device control only reaches private addresses"));
    }
    Ok(())
}

const MAX_BODY: usize = 6000;

fn clip(s: String) -> String {
    if s.len() <= MAX_BODY {
        return s;
    }
    let mut end = MAX_BODY;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[... {} more bytes]", &s[..end], s.len() - end)
}

fn lan_client() -> Result<reqwest::Client, String> {
    // Many devices have self-signed certificates: on the LAN only, accept them.
    reqwest::Client::builder().timeout(Duration::from_secs(10)).no_proxy().danger_accept_invalid_certs(true).build().map_err(|e| e.to_string())
}

/// An HTTP request to a device on the local network.
pub async fn http(method: &str, url: &str, headers: &[(String, String)], body: Option<&str>) -> Result<String, String> {
    private_host(url).await?;
    let method = reqwest::Method::from_bytes(method.to_ascii_uppercase().as_bytes()).map_err(|_| format!("unknown method {method}"))?;
    let mut req = lan_client()?.request(method, url);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some(b) = body {
        req = req.body(b.to_string());
    }
    let resp = req.send().await.map_err(|e| format!("the device did not answer: {e}"))?;
    let status = resp.status();
    let kind = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let text = resp.text().await.unwrap_or_default();
    Ok(clip(format!("HTTP {status} {kind}\n{text}")))
}

/// Calls a UPnP action (SOAP), e.g. `Play`, `Pause`, `SetVolume`.
pub async fn upnp_action(control_url: &str, service_type: &str, action: &str, args: &[(String, String)]) -> Result<String, String> {
    private_host(control_url).await?;
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let inner: String = args.iter().map(|(k, v)| format!("<{k}>{}</{k}>", esc(v))).collect();
    let envelope = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{service_type}\">{inner}</u:{action}></s:Body></s:Envelope>"
    );
    let resp = lan_client()?
        .post(control_url)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPACTION", format!("\"{service_type}#{action}\""))
        .body(envelope)
        .send()
        .await
        .map_err(|e| format!("the device did not answer: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.is_success() {
        Ok(clip(format!("{action}: OK\n{text}")))
    } else {
        Err(clip(format!("{action} failed: HTTP {status}\n{text}")))
    }
}

/// Wakes a computer with a Wake-on-LAN magic packet.
pub async fn wake(mac: &str, broadcast: Option<Ipv4Addr>) -> Result<String, String> {
    let mac = normalize_mac(mac).ok_or_else(|| format!("{mac} is not a MAC address"))?;
    let bytes: Vec<u8> = mac.split(':').map(|p| u8::from_str_radix(p, 16).unwrap_or(0)).collect();
    let mut packet = vec![0xFFu8; 6];
    for _ in 0..16 {
        packet.extend_from_slice(&bytes);
    }
    let target = broadcast.unwrap_or(Ipv4Addr::BROADCAST);
    if !is_private(IpAddr::V4(target)) && target != Ipv4Addr::BROADCAST {
        return Err(format!("{target} is not a local broadcast address"));
    }
    let socket = UdpSocket::bind("0.0.0.0:0").await.map_err(|e| e.to_string())?;
    socket.set_broadcast(true).map_err(|e| e.to_string())?;
    for port in [9u16, 7] {
        socket.send_to(&packet, (target, port)).await.map_err(|e| e.to_string())?;
    }
    Ok(format!("magic packet sent to {mac} via {target}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_arp_table_of_each_system() {
        let windows = "Interface: 192.168.1.10 --- 0x5\n  Internet Address      Physical Address      Type\n  192.168.1.1           a4-2b-b0-11-22-33     dynamic\n  192.168.1.255         ff-ff-ff-ff-ff-ff     static\n  224.0.0.22            01-00-5e-00-00-16     static\n";
        assert_eq!(parse_arp(windows), vec![(Ipv4Addr::new(192, 168, 1, 1), "a4:2b:b0:11:22:33".to_string())]);
        let mac_os = "? (10.0.0.7) at 3c:6:30:aa:bb:cc on en0 ifscope [ethernet]\n";
        assert_eq!(parse_arp(mac_os), vec![(Ipv4Addr::new(10, 0, 0, 7), "3c:06:30:aa:bb:cc".to_string())]);
        let linux = "IP address       HW type     Flags       HW address            Mask     Device\n192.168.0.20     0x1         0x2         b8:27:eb:01:02:03     *        wlan0\n";
        assert_eq!(parse_arp(linux), vec![(Ipv4Addr::new(192, 168, 0, 20), "b8:27:eb:01:02:03".to_string())]);
    }

    #[test]
    fn reads_a_upnp_description_with_its_actions() {
        let xml = "<root><device><deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType><friendlyName>Sala TV</friendlyName><manufacturer>Samsung</manufacturer><modelName>QN55</modelName><serviceList><service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/upnp/control/AVTransport1</controlURL></service><service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>RenderingControl/ctrl</controlURL></service></serviceList></device></root>";
        let (u, name) = parse_description("http://192.168.1.30:9197/dmr", xml);
        assert_eq!(name.as_deref(), Some("Sala TV"));
        assert_eq!(u.manufacturer, "Samsung");
        assert_eq!(u.controls[0], ("urn:schemas-upnp-org:service:AVTransport:1".into(), "http://192.168.1.30:9197/upnp/control/AVTransport1".into()));
        assert_eq!(u.controls[1].1, "http://192.168.1.30:9197/RenderingControl/ctrl");
    }

    #[tokio::test]
    async fn control_never_leaves_the_local_network() {
        assert!(http("GET", "http://8.8.8.8/", &[], None).await.unwrap_err().contains("not on the local network"));
        assert!(upnp_action("http://1.1.1.1:80/ctl", "urn:x", "Play", &[]).await.is_err());
        assert!(private_host("http://192.168.1.5:8080/x").await.is_ok());
        assert!(private_host("http://[fe80::1]:80/").await.is_ok());
        assert!(wake("not-a-mac", None).await.is_err());
    }

    #[test]
    fn private_addresses() {
        for ip in ["10.0.0.1", "172.16.4.2", "192.168.0.9", "169.254.1.1", "127.0.0.1", "fd00::1", "fe80::2"] {
            assert!(is_private(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "172.32.0.1", "2001:4860::8888"] {
            assert!(!is_private(ip.parse().unwrap()), "{ip}");
        }
    }
}
