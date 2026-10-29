//! Devices simulated on this computer, spoken to end to end through the
//! same path a real one takes: detection, then the protocol. They use the
//! devices' real ports; a test is skipped when its port is already taken.

use coturbo_net::home::{Act, Kind, control, detect};
use std::net::{IpAddr, Ipv4Addr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const HERE: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// One simulated device at a time: both live on this computer's address, and
/// detection would find the other one's port.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cipher(plain: &[u8]) -> Vec<u8> {
    let mut key = 171u8;
    plain
        .iter()
        .map(|&b| {
            key ^= b;
            key
        })
        .collect()
}

fn decipher(c: &[u8]) -> Vec<u8> {
    let mut key = 171u8;
    c.iter()
        .map(|&x| {
            let p = key ^ x;
            key = x;
            p
        })
        .collect()
}

/// A Kasa plug: answers get_sysinfo and switches its relay.
#[tokio::test]
async fn a_kasa_plug_is_found_and_switched() {
    let _one = ONE_AT_A_TIME.lock().await;
    let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, 9999)).await else {
        return;
    };
    let plug = tokio::spawn(async move {
        let mut relay = 0;
        let mut seen = Vec::new();
        while let Ok((mut s, _)) = listener.accept().await {
            let mut len = [0u8; 4];
            if s.read_exact(&mut len).await.is_err() {
                continue;
            }
            let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
            s.read_exact(&mut body).await.unwrap();
            let req: serde_json::Value = serde_json::from_slice(&decipher(&body)).unwrap();
            if let Some(state) = req["system"]["set_relay_state"]["state"].as_u64() {
                relay = state;
                seen.push(state);
            }
            let reply = serde_json::json!({"system": {"get_sysinfo": {"alias": "Desk lamp", "model": "HS100(EU)", "relay_state": relay}}});
            let c = cipher(reply.to_string().as_bytes());
            let mut out = (c.len() as u32).to_be_bytes().to_vec();
            out.extend(c);
            s.write_all(&out).await.unwrap();
            if seen.len() == 2 {
                return seen;
            }
        }
        seen
    });
    assert_eq!(detect(HERE).await, Kind::Kasa);
    let status = control(HERE, None, Act::Status, None).await.unwrap();
    assert!(
        status.contains("Desk lamp") && status.contains("off"),
        "{status}"
    );
    control(HERE, None, Act::On, None).await.unwrap();
    control(HERE, None, Act::Toggle, None).await.unwrap();
    assert_eq!(plug.await.unwrap(), vec![1, 0], "on, then toggled back off");
}

async fn read_mqtt(s: &mut tokio::net::TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut h = [0u8; 1];
    s.read_exact(&mut h).await.ok()?;
    let (mut len, mut mult) = (0usize, 1usize);
    loop {
        let mut b = [0u8; 1];
        s.read_exact(&mut b).await.ok()?;
        len += (b[0] & 0x7f) as usize * mult;
        if b[0] & 0x80 == 0 {
            break;
        }
        mult *= 128;
    }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).await.ok()?;
    Some((h[0], body))
}

/// An MQTT broker with Zigbee2MQTT behind it: the device list is read and a
/// command published.
#[tokio::test]
async fn zigbee_through_an_mqtt_broker() {
    let _one = ONE_AT_A_TIME.lock().await;
    let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, 1883)).await else {
        return;
    };
    let broker = tokio::spawn(async move {
        let mut published = None;
        for _ in 0..20 {
            let Ok((mut s, _)) = listener.accept().await else {
                break;
            };
            // A port probe connects and leaves without a word.
            let Some((0x10, _)) = read_mqtt(&mut s).await else {
                continue;
            };
            s.write_all(&[0x20, 0x02, 0x00, 0x00]).await.unwrap();
            let Some((h, body)) = read_mqtt(&mut s).await else {
                continue;
            };
            if h == 0x82 {
                s.write_all(&[0x90, 0x03, body[0], body[1], 0x00])
                    .await
                    .unwrap();
                let devices = r#"[{"type":"Coordinator","friendly_name":"Coordinator"},{"type":"Router","friendly_name":"sala lamp","definition":{"description":"Smart bulb"}}]"#;
                let topic = "zigbee2mqtt/bridge/devices";
                let mut p = (topic.len() as u16).to_be_bytes().to_vec();
                p.extend_from_slice(topic.as_bytes());
                p.extend_from_slice(devices.as_bytes());
                let mut frame = vec![0x31];
                let mut n = p.len();
                loop {
                    let mut b = (n % 128) as u8;
                    n /= 128;
                    if n > 0 {
                        b |= 0x80;
                    }
                    frame.push(b);
                    if n == 0 {
                        break;
                    }
                }
                frame.extend(p);
                s.write_all(&frame).await.unwrap();
            } else if h & 0xF0 == 0x30 {
                let tlen = u16::from_be_bytes([body[0], body[1]]) as usize;
                published = Some((
                    String::from_utf8_lossy(&body[2..2 + tlen]).into_owned(),
                    String::from_utf8_lossy(&body[2 + tlen..]).into_owned(),
                ));
                break;
            }
        }
        published
    });
    // Port 1883 open, nothing else: a broker.
    if detect(HERE).await != Kind::Mqtt {
        broker.abort();
        return; // something else answers on this machine's other ports
    }
    let list = control(HERE, None, Act::Status, None).await.unwrap();
    assert!(
        list.contains("1 Zigbee device(s): sala lamp (Smart bulb)"),
        "{list}"
    );
    control(
        HERE,
        None,
        Act::Publish {
            topic: "zigbee2mqtt/sala lamp/set".into(),
            payload: r#"{"state":"ON"}"#.into(),
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        broker.await.unwrap(),
        Some((
            "zigbee2mqtt/sala lamp/set".into(),
            r#"{"state":"ON"}"#.into()
        ))
    );
}
