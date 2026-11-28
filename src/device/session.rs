//! One live connection to this device: control channel + TLS-PSK CDTunnel +
//! our TCP stack + RSD service map. LocalDevVPN loops 10.7.0.1 back to the
//! device's own RemotePairing service (port 49152, or another dynamic port,
//! which Bonjour names: rootshell may browse `_remotepairing._tcp`).

use std::collections::BTreeMap;
use std::net::Ipv6Addr;
use std::time::Duration;
use serde_json::{json, Value as Json};

use super::rppairing::{Control, VerifyError};
use super::tcp::{Reassembler, Stack, TcpConn};
use super::{tls_psk, xpc};
use crate::net::TcpStream;
use crate::pairing::record::PairingRecord;
use crate::{logf, rt, state};

pub const LOOPBACK: &str = "10.7.0.1";
pub const PAIRING_PORT: u16 = 49152;

pub struct Session {
    _control: Control,
    stack: Stack,
    pub services: BTreeMap<String, u16>,
    pub properties: plist::Dictionary,
    pub udid: String,
}

#[derive(Debug)]
pub enum OpenError {
    NoVpn(String),
    NotPaired,
    Failed(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NoVpn(why) => write!(f, "{why}"),
            OpenError::NotPaired => write!(f, "this device no longer accepts BariStore's pairing; pair again"),
            OpenError::Failed(why) => write!(f, "{why}"),
        }
    }
}

/// The host UUID every RemoteXPC handshake presents (fixed per install).
pub fn host_uuid() -> [u8; 16] {
    let path = state::path("host-uuid");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(u) = uuid::Uuid::parse_str(text.trim()) {
            return *u.as_bytes();
        }
    }
    let u = uuid::Uuid::new_v4();
    let _ = state::write_private(&path, u.to_string().as_bytes());
    *u.as_bytes()
}

/// Is LocalDevVPN answering? It hands every IPv4 packet for 10.7.0.1 back to
/// the device with source and destination address swapped, ports unchanged
/// (its PacketTunnelProvider, read 2026-11-28), so a datagram from port P to
/// 10.7.0.1:P comes back to the socket that sent it; without it nothing
/// comes back. (A datagram to a closed port's ICMP refusal names the
/// swapped ports, so it never reached the socket that asked: 1.2.0-alpha.2
/// and older saw no LocalDevVPN at all.) A datagram, not a TCP connection:
/// rootshell's connect has no time limit, and a SYN nobody answers would
/// hold up the whole program.
pub async fn vpn_up() -> bool {
    let host = override_endpoint().map_or_else(|| LOOPBACK.to_string(), |(h, _)| h);
    // Up to three tries: back in the foreground, the first can go nowhere
    // while LocalDevVPN comes back (the owner's second try a few seconds
    // later always connected, 1.1.0).
    for _ in 0..3 {
        let word = format!("BariStore {}", hex::encode(rand::random::<[u8; 8]>()));
        match crate::net::udp_echo(&host, word.as_bytes(), Duration::from_millis(1500)).await {
            Ok(true) => return true,
            Ok(false) => logf!("device: nothing came back from {host}"),
            Err(e) => logf!("device: {host}: {e}"),
        }
        rt::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

/// BARISTORE_DEVICE=host:port points at a mock device in tests.
fn override_endpoint() -> Option<(String, u16)> {
    let v = std::env::var("BARISTORE_DEVICE").ok()?;
    let (h, p) = v.rsplit_once(':')?;
    Some((h.to_string(), p.parse().ok()?))
}

/// What a knock on one port found.
#[derive(PartialEq, Eq)]
enum Knock {
    /// The RemotePairing service answered its handshake.
    Answered,
    /// Refused, or something else took the connection: look elsewhere.
    Closed,
    /// Nothing came back in time: LocalDevVPN (or Wi-Fi) is not carrying it.
    Silent,
}

/// Does `port` answer an RPPairing handshake (the RemotePairing service)?
async fn knock(host: &str, port: u16, answer: Duration) -> Knock {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = match TcpStream::connect(host, port).await {
        Ok(s) => s,
        Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset) => {
            return Knock::Closed;
        }
        Err(_) => return Knock::Silent,
    };
    let body = serde_json::to_vec(&json!({"message": {"plain": {"_0": {"request": {"_0": {"handshake": {"_0": {
        "hostOptions": {"attemptPairVerify": true},
        "wireProtocolVersion": super::rppairing::WIRE_VERSION}}}}}}},
        "originatedBy": "host", "sequenceNumber": 0})).unwrap();
    let mut frame = b"RPPairing".to_vec();
    frame.extend_from_slice(&(body.len() as u16).to_be_bytes());
    frame.extend_from_slice(&body);
    let mut magic = [0u8; 9];
    let answered = s.write_all(&frame).await.is_ok()
        && matches!(rt::time::timeout(answer, s.read_exact(&mut magic)).await, Ok(Ok(_)))
        && &magic == b"RPPairing";
    if answered { Knock::Answered } else { Knock::Closed }
}

pub const NO_ROUTE: &str = "No connection to the device: turn on Wi-Fi and connect LocalDevVPN.";

/// The ports Bonjour lists for `_remotepairing._tcp` (rootshell 1.0.16-177
/// browses it): this device's RemotePairing service among them, the ports
/// at one of this device's own addresses (ifconfig.txt) first.
fn bonjour_ports() -> Vec<u16> {
    let Ok(found) = crate::rootshell::bonjour_lookup("_remotepairing._tcp", 3000) else { return Vec::new() };
    let own: Vec<String> = crate::mdns::ifconfig_addresses(&std::fs::read_to_string(state::path("ifconfig.txt"))
        .unwrap_or_default()).iter().map(|a| std::net::Ipv4Addr::from(*a).to_string()).collect();
    let mut ports: Vec<(bool, u16)> = found.iter()
        .map(|(_, _, port, addrs)| (!addrs.iter().any(|a| own.contains(a)), *port)).collect();
    ports.sort();
    let mut list: Vec<u16> = ports.into_iter().map(|(_, p)| p).collect();
    list.dedup();
    logf!("device: Bonjour lists RemotePairing on port(s) {list:?}");
    list
}

/// Where the RemotePairing service answers. Usually 49152, but iOS takes
/// the first free dynamic port at boot: then the ports Bonjour lists, and
/// last the other dynamic ports one by one (as siboot 0.19.1 does, but
/// rootshell connects one at a time); the port found is remembered.
pub async fn endpoint() -> Result<(String, u16), OpenError> {
    if let Some(e) = override_endpoint() {
        return Ok(e);
    }
    let saved = std::fs::read_to_string(state::path("device-port")).ok()
        .and_then(|s| s.trim().parse::<u16>().ok()).filter(|p| *p != PAIRING_PORT);
    for port in saved.into_iter().chain([PAIRING_PORT]) {
        match knock(LOOPBACK, port, Duration::from_secs(5)).await {
            Knock::Answered => return Ok((LOOPBACK.to_string(), port)),
            // Silence is no route at all (siboot): knocking on 16 000 more
            // ports would only wait longer.
            Knock::Silent => return Err(OpenError::NoVpn(NO_ROUTE.into())),
            Knock::Closed => {}
        }
    }
    for port in bonjour_ports().into_iter().filter(|p| *p != PAIRING_PORT) {
        if knock(LOOPBACK, port, Duration::from_secs(5)).await == Knock::Answered {
            logf!("device: the pairing service answers on {port} (Bonjour)");
            let _ = std::fs::write(state::path("device-port"), port.to_string());
            return Ok((LOOPBACK.to_string(), port));
        }
    }
    logf!("device: {PAIRING_PORT} is not the pairing service; knocking on the other dynamic ports");
    let started = std::time::Instant::now();
    let ports: Vec<u16> = (PAIRING_PORT + 1..=u16::MAX).filter(|p| *p != 62078).collect();
    for chunk in ports.chunks(32) {
        if started.elapsed() > Duration::from_secs(90) {
            break;
        }
        let probes = chunk.iter().map(|&p| async move { (p, knock(LOOPBACK, p, Duration::from_millis(300)).await) });
        let results = futures_util::future::join_all(probes).await;
        if let Some((port, _)) = results.iter().find(|(_, k)| *k == Knock::Answered) {
            logf!("device: the pairing service answers on {port}");
            let _ = std::fs::write(state::path("device-port"), port.to_string());
            return Ok((LOOPBACK.to_string(), *port));
        }
        if results.iter().all(|(_, k)| *k == Knock::Silent) {
            return Err(OpenError::NoVpn(NO_ROUTE.into()));
        }
    }
    Err(OpenError::Failed("LocalDevVPN is connected, but the device does not answer. Restart the device and try \
        again.".into()))
}

pub struct Tunnel {
    pub control: Control,
    pub stack: Stack,
    pub rsd_port: u16,
}

impl Tunnel {
    /// Pair-verify, createListener, TLS-PSK, CDTunnel, and our TCP stack on it.
    pub async fn open(record: &PairingRecord) -> Result<Tunnel, OpenError> {
        let (host, pairing_port) = endpoint().await?;
        let host = host.as_str();
        let mut control = match Control::open(host, pairing_port, record).await {
            Ok(c) => c,
            Err(VerifyError::NotPaired) => return Err(OpenError::NotPaired),
            Err(VerifyError::Other(why)) if why.contains("LocalDevVPN") => {
                logf!("device: {why}");
                return Err(OpenError::NoVpn(NO_ROUTE.into()));
            }
            Err(VerifyError::Other(why)) => return Err(OpenError::Failed(format!("pair-verify: {why}"))),
        };
        logf!("device: pair-verify ok");
        let port = control.create_tcp_listener().await.map_err(|e| OpenError::Failed(format!("tunnel listener: {e}")))?;
        let tunnel = TcpStream::connect(host, port).await
            .map_err(|e| OpenError::Failed(format!("tunnel port {port}: {e}")))?;
        let (r, w) = tunnel.into_split();
        let (mut reader, mut writer) = tls_psk::connect(r, w, &control.psk).await
            .map_err(|e| OpenError::Failed(format!("TLS-PSK: {e}")))?;
        logf!("device: TLS-PSK tunnel up on port {port}");

        let request = serde_json::to_vec(&json!({"type": "clientHandshakeRequest", "mtu": 16000})).unwrap();
        let mut hello = b"CDTunnel".to_vec();
        hello.extend_from_slice(&(request.len() as u16).to_be_bytes());
        hello.extend_from_slice(&request);
        writer.write(&hello).await.map_err(|e| OpenError::Failed(format!("CDTunnel: {e}")))?;
        let mut got = Vec::new();
        let (info, rest) = loop {
            got.extend(reader.read().await.map_err(|e| OpenError::Failed(format!("CDTunnel: {e}")))?);
            if got.len() >= 10 {
                if &got[..8] != b"CDTunnel" {
                    return Err(OpenError::Failed("CDTunnel answer without its magic".into()));
                }
                let len = u16::from_be_bytes([got[8], got[9]]) as usize;
                if got.len() >= 10 + len {
                    let info: Json = serde_json::from_slice(&got[10..10 + len])
                        .map_err(|e| OpenError::Failed(format!("CDTunnel JSON: {e}")))?;
                    break (info, got[10 + len..].to_vec());
                }
            }
        };
        let addr = |v: Option<&Json>| v.and_then(Json::as_str).and_then(|s| s.split('/').next())
            .and_then(|s| s.parse::<Ipv6Addr>().ok());
        let client = addr(info.pointer("/clientParameters/address"));
        let server = addr(info.get("serverAddress"));
        let rsd_port = info.get("serverRSDPort")
            .and_then(|p| p.as_u64().or_else(|| p.as_str().and_then(|s| s.parse().ok())))
            .and_then(|p| u16::try_from(p).ok());
        let (Some(client), Some(server), Some(rsd_port)) = (client, server, rsd_port) else {
            return Err(OpenError::Failed(format!("incomplete CDTunnel answer: {info}")));
        };
        let mtu = info.pointer("/clientParameters/mtu").and_then(Json::as_u64).unwrap_or(1280) as usize;
        logf!("device: tunnel {client} -> {server}, RSD port {rsd_port}, MTU {mtu}");

        let (stack, mut outgoing) = Stack::start(client, server, mtu);
        let inbound = stack.clone();
        rt::spawn(async move {
            let mut assembler = Reassembler::default();
            for p in assembler.push(&rest) {
                inbound.deliver(p);
            }
            loop {
                match reader.read().await {
                    Ok(data) => {
                        for p in assembler.push(&data) {
                            inbound.deliver(p);
                        }
                    }
                    Err(e) => {
                        logf!("device: tunnel closed: {e}");
                        inbound.link_down(format!("the tunnel closed: {e}"));
                        return;
                    }
                }
            }
        });
        rt::spawn(async move {
            while let Some(first) = outgoing.recv().await {
                let mut batch = first;
                while batch.len() < 256 * 1024 {
                    match outgoing.try_recv() {
                        Ok(p) => batch.extend_from_slice(&p),
                        Err(_) => break,
                    }
                }
                if writer.write(&batch).await.is_err() {
                    return;
                }
            }
        });

        Ok(Tunnel { control, stack, rsd_port })
    }
}

impl Session {
    pub async fn open(record: &PairingRecord) -> Result<Session, OpenError> {
        let Tunnel { control, stack, rsd_port, .. } = Tunnel::open(record).await?;
        let rsd = stack.connect(rsd_port).await.map_err(|e| OpenError::Failed(format!("RSD: {e}")))?;
        let (services, properties) = xpc::rsd_handshake(rsd, host_uuid()).await.map_err(OpenError::Failed)?;
        let udid = properties.get("UniqueDeviceID").and_then(plist::Value::as_string)
            .unwrap_or(&record.peer.udid).to_string();
        logf!("device: RSD lists {} services", services.len());
        Ok(Session { _control: control, stack, services, properties, udid })
    }

    pub fn port(&self, names: &[&str]) -> Result<u16, String> {
        for name in names {
            if let Some(p) = self.services.get(*name) {
                return Ok(*p);
            }
        }
        Err(format!("the device does not offer {} (Developer Mode off?)", names[0]))
    }

    pub async fn connect(&self, names: &[&str]) -> Result<TcpConn, String> {
        let port = self.port(names)?;
        self.stack.connect(port).await
    }
}
