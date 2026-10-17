//! TVs on the local network, whatever the brand: what it is, its apps, and
//! the remote control (power, volume, keys, launching apps). Each brand
//! speaks its own protocol; DIAL opens apps such as YouTube on almost any.
//!
//! - Samsung (Tizen): WebSocket remote on 8002, accepted once on the TV.
//! - Android TV / Google TV: ADB over the network (developer options), the
//!   TV asks once to allow this computer.
//! - LG (webOS): SSAP WebSocket on 3000/3001, accepted once on the TV.
//! - Roku: the HTTP remote (ECP) on 8060.
//! - Any TV with DIAL (Samsung, LG, Android, Fire TV, Roku): launch apps.

mod adb;
mod cast;
mod dial;
mod lg;
mod roku;
mod samsung;

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// What a TV is, as far as the network tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brand {
    Samsung,
    Lg,
    Roku,
    /// Android TV with network debugging on (full control).
    AndroidAdb,
    /// A Google Cast / Android TV without ADB: DIAL only.
    Cast,
    Unknown,
}

impl Brand {
    pub fn label(self) -> &'static str {
        match self {
            Brand::Samsung => "Samsung (Tizen)",
            Brand::Lg => "LG (webOS)",
            Brand::Roku => "Roku",
            Brand::AndroidAdb => "Android TV (ADB)",
            Brand::Cast => "Android TV / Google TV (Google Cast)",
            Brand::Unknown => "unknown",
        }
    }
}

/// One thing to do on a TV.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Info,
    Apps,
    Launch { app: String, content: Option<String> },
    Key(String),
    PowerOff,
    PowerOn { mac: Option<String> },
    VolumeUp(u8),
    VolumeDown(u8),
    SetVolume(u8),
    Mute,
}

/// Where pairing tokens and keys live: the CodeTurbo config folder.
pub(crate) fn state_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    let dir = base.unwrap_or_else(std::env::temp_dir).join("codeturbo").join("tv");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// A saved value for one TV (a pairing token), by address.
pub(crate) fn load_secret(kind: &str, ip: IpAddr) -> Option<String> {
    let path = state_dir().join(format!("{kind}.json"));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    v[ip.to_string()].as_str().map(str::to_string)
}

pub(crate) fn save_secret(kind: &str, ip: IpAddr, value: &str) {
    let path = state_dir().join(format!("{kind}.json"));
    let mut v: serde_json::Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    v[ip.to_string()] = serde_json::Value::String(value.to_string());
    let _ = std::fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
}

/// TVs use self-signed certificates: on the local network only, any is accepted.
#[derive(Debug)]
struct AnyCert(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AnyCert {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub(crate) fn lan_tls() -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCert(provider)))
        .with_no_client_auth();
    Arc::new(config)
}

pub(crate) type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Opens a WebSocket to a TV (`ws://` or `wss://` with its own certificate).
pub(crate) async fn websocket(url: &str) -> Result<Socket, String> {
    let connect = tokio_tungstenite::connect_async_tls_with_config(url, None, false, Some(tokio_tungstenite::Connector::Rustls(lan_tls())));
    let (ws, _) = tokio::time::timeout(Duration::from_secs(6), connect)
        .await
        .map_err(|_| format!("{url} did not answer"))?
        .map_err(|e| format!("{url}: {e}"))?;
    Ok(ws)
}

pub(crate) fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(5)).no_proxy().danger_accept_invalid_certs(true).build().unwrap_or_default()
}

async fn open(ip: IpAddr, port: u16) -> bool {
    matches!(tokio::time::timeout(Duration::from_millis(700), tokio::net::TcpStream::connect((ip, port))).await, Ok(Ok(_)))
}

/// Finds out what kind of TV answers at `ip`.
pub async fn detect(ip: IpAddr) -> Brand {
    let (s8001, s8002, lg3000, lg3001, roku, adb, cast) =
        tokio::join!(open(ip, 8001), open(ip, 8002), open(ip, 3000), open(ip, 3001), open(ip, 8060), open(ip, 5555), open(ip, 8009));
    if (s8001 || s8002) && samsung::info(ip).await.is_ok() {
        return Brand::Samsung;
    }
    if roku {
        return Brand::Roku;
    }
    if adb {
        return Brand::AndroidAdb;
    }
    if lg3000 || lg3001 {
        return Brand::Lg;
    }
    if cast {
        return Brand::Cast;
    }
    Brand::Unknown
}

/// Does `action` on the TV at `ip`, whatever its brand.
pub async fn control(ip: IpAddr, action: Action) -> Result<String, String> {
    if !crate::is_private(ip) {
        return Err(format!("{ip} is not on the local network"));
    }
    // Power on: the TV is asleep and answers nothing; Wake-on-LAN wakes it.
    if let Action::PowerOn { mac } = &action {
        let mac = match mac {
            Some(m) => m.clone(),
            None => known_mac(ip).ok_or("give the TV's MAC address (network_scan shows it) to wake it")?,
        };
        let woke = crate::wake(&mac, None).await?;
        return Ok(format!("{woke}. Most TVs need \"Turn on via network\" / \"Wake on LAN\" on in their settings."));
    }
    let brand = detect(ip).await;
    let done = match brand {
        Brand::Samsung => samsung::control(ip, &action).await,
        Brand::AndroidAdb => adb::control(ip, &action).await,
        Brand::Lg => lg::control(ip, &action).await,
        Brand::Roku => roku::control(ip, &action).await,
        Brand::Cast => cast::control(ip, &action).await,
        Brand::Unknown => match &action {
            Action::Launch { app, content } => dial::launch(ip, app, content.as_deref()).await,
            Action::Info => dial::info(ip).await,
            _ => Err(format!("{ip} does not look like a TV this engine knows (Samsung, LG, Android TV / Google TV, Roku)")),
        },
    };
    done.map(|text| format!("{} · {text}", brand.label()))
}

/// The MAC address of `ip`: from the ARP cache, else remembered from when
/// it was last on (a TV that is off is not in the cache).
fn known_mac(ip: IpAddr) -> Option<String> {
    let IpAddr::V4(v4) = ip else { return None };
    crate::arp_table().into_iter().find(|(a, _)| *a == v4).map(|(_, m)| m).or_else(|| crate::registry::resolve(&ip.to_string()).and_then(|(_, m)| m))
}

/// Common app names to their usual package or id, per platform.
pub(crate) fn app_alias(app: &str) -> String {
    app.trim().to_lowercase().replace([' ', '+'], "")
}
