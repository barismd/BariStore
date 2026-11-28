//! The pairable host's Bonjour advertisement, sent as raw mDNS (RFC 6762
//! §8.3): what Open Minis's `minis-bonjour advertise` had iOS publish.
//!
//! rootshell's Info.plist declares no `_remotepairing-pairable-host._tcp`,
//! so iOS would not publish it for rootshell; but rootshell has Apple's
//! multicast entitlement, so a program it runs may send to 224.0.0.251:5353
//! itself. The device's own mDNSResponder takes the announcement into its
//! cache and Settings › Developer Mode lists "Pair with BariStore" from
//! there (device-proven by siboot's rootshell pairing, MIT, and the programs
//! made from it: PTR, SRV, TXT and an A record with the device's Wi-Fi
//! address, a long TTL, sent more than once; a goodbye with TTL 0 at the end).
//!
//! The A record needs this device's address, which rootshell does not tell
//! its programs (`getsockname` says 0.0.0.0). `baristore` saves the output
//! of rootshell's `ifconfig` in ifconfig.txt before it starts the program;
//! failing that, BariStore listens for its own announcement coming back
//! from the group, or asks Bonjour where this device's RemotePairing
//! service is.

use std::time::Duration;

use crate::net::UdpSocket;
use crate::{logf, state};

pub const GROUP: &str = "224.0.0.251";
pub const PORT: u16 = 5353;
/// Long enough to find the entry in Settings; the goodbye withdraws it.
pub const TTL: u32 = 75 * 60;

const A: u16 = 1;
const PTR: u16 = 12;
const TXT: u16 = 16;
const SRV: u16 = 33;

pub struct Announcement {
    /// `_remotepairing-pairable-host._tcp.local`.
    pub service: String,
    /// The instance: the host identity's identifier, as the TXT says too.
    pub instance: String,
    /// What the SRV record points at, `<label>.local`.
    pub host: String,
    pub addresses: Vec<[u8; 4]>,
    pub port: u16,
    pub txt: Vec<(String, String)>,
}

fn name(n: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in n.trim_end_matches('.').split('.') {
        let label = &label.as_bytes()[..label.len().min(63)];
        out.push(label.len() as u8);
        out.extend_from_slice(label);
    }
    out.push(0);
    out
}

fn record(owner: &str, kind: u16, rdata: &[u8], ttl: u32, unique: bool) -> Vec<u8> {
    let mut out = name(owner);
    out.extend_from_slice(&kind.to_be_bytes());
    // Class IN; the cache-flush bit on the records only this host answers.
    out.extend_from_slice(&(1u16 | if unique { 0x8000 } else { 0 }).to_be_bytes());
    out.extend_from_slice(&ttl.to_be_bytes());
    out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    out.extend_from_slice(rdata);
    out
}

impl Announcement {
    /// The response: PTR, SRV, TXT and an A record per address. TTL 0 is the
    /// goodbye.
    pub fn packet(&self, ttl: u32) -> Vec<u8> {
        let full = format!("{}.{}", self.instance, self.service);
        let mut srv = vec![0, 0, 0, 0];
        srv.extend_from_slice(&self.port.to_be_bytes());
        srv.extend(name(&self.host));
        let mut txt = Vec::new();
        for (k, v) in &self.txt {
            let entry = format!("{k}={v}");
            let bytes = &entry.as_bytes()[..entry.len().min(255)];
            txt.push(bytes.len() as u8);
            txt.extend_from_slice(bytes);
        }
        let mut records = vec![
            record(&self.service, PTR, &name(&full), ttl, false),
            record(&full, SRV, &srv, ttl, true),
            record(&full, TXT, &txt, ttl, true),
        ];
        records.extend(self.addresses.iter().map(|ip| record(&self.host, A, ip, ttl, true)));
        // ID 0, a response with authority; no questions.
        let mut out = vec![0, 0, 0x84, 0, 0, 0];
        out.extend_from_slice(&(records.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        for r in records {
            out.extend(r);
        }
        out
    }
}

/// An announcement out on the network; the goodbye goes when it is dropped.
pub struct Advert {
    socket: UdpSocket,
    announcement: Announcement,
}

impl Advert {
    pub fn start(announcement: Announcement) -> std::io::Result<Advert> {
        let advert = Advert { socket: UdpSocket::bind(0)?, announcement };
        advert.announce()?;
        Ok(advert)
    }

    /// Sends the announcement (three times: one may be lost). Repeated
    /// while pairing waits, in case the cache was emptied meanwhile.
    pub fn announce(&self) -> std::io::Result<()> {
        let packet = self.announcement.packet(TTL);
        for _ in 0..3 {
            self.socket.send_to(&packet, GROUP, PORT)?;
        }
        Ok(())
    }
}

impl Drop for Advert {
    fn drop(&mut self) {
        let goodbye = self.announcement.packet(0);
        for _ in 0..2 {
            let _ = self.socket.send_to(&goodbye, GROUP, PORT);
        }
        logf!("pair: announcement withdrawn");
    }
}

/// `ifconfig`'s lines as they were meant: rootshell's leaves out the line
/// break after each `hwassist=…<…>` line, so an interface's `inet` or the
/// next interface's header follows on the same line (atelier-install saw
/// it on an iPhone, 14 Nov 2026).
fn ifconfig_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    for mut line in text.lines() {
        while let Some(end) = line.find("hwassist=").and_then(|at| line[at..].find('>').map(|e| at + e + 1)) {
            if end >= line.len() {
                break;
            }
            lines.push(&line[..end]);
            line = &line[end..];
        }
        lines.push(line);
    }
    lines
}

/// The interfaces and IPv4 addresses `ifconfig` output names; never
/// loopback, link-local or the VPN's tunnels.
fn ifconfig_inets(text: &str) -> Vec<(String, [u8; 4])> {
    let mut found = Vec::new();
    let mut iface = String::new();
    for line in ifconfig_lines(text) {
        if !line.starts_with([' ', '\t']) {
            iface = line.split(':').next().unwrap_or("").trim().to_string();
            continue;
        }
        let mut words = line.split_whitespace();
        if words.next() != Some("inet") {
            continue;
        }
        let Some(ip) = words.next().and_then(|a| a.split('/').next()).and_then(|a| a.parse::<std::net::Ipv4Addr>().ok())
        else { continue };
        let o = ip.octets();
        if ip.is_loopback() || o[0] == 169 && o[1] == 254 || iface.starts_with("utun") || iface.starts_with("ipsec") {
            continue;
        }
        found.push((iface.clone(), o));
    }
    found
}

/// The IPv4 addresses `ifconfig` output names, en0's (Wi-Fi) first.
pub fn ifconfig_addresses(text: &str) -> Vec<[u8; 4]> {
    let found = ifconfig_inets(text);
    let wifi = found.iter().filter(|(i, _)| i == "en0");
    let other = found.iter().filter(|(i, _)| i != "en0");
    wifi.chain(other).map(|(_, a)| *a).collect()
}

/// Whether `ifconfig` output shows Wi-Fi joined: en0, the Wi-Fi interface
/// on iPhone and iPad, has an IPv4 address only then.
pub fn wifi_joined(text: &str) -> bool {
    ifconfig_inets(text).iter().any(|(i, _)| i == "en0")
}

/// This device's own addresses for the A record, and where they came from.
pub fn own_addresses() -> (Vec<[u8; 4]>, &'static str) {
    let path = state::path("ifconfig.txt");
    if let Ok(text) = std::fs::read_to_string(&path) {
        let found = ifconfig_addresses(&text);
        if !found.is_empty() {
            return (found, "ifconfig.txt");
        }
    }
    if let Some(ip) = echoed_address() {
        return (vec![ip], "its own announcement");
    }
    let found = bonjour_addresses();
    if !found.is_empty() {
        return (found, "Bonjour (RemotePairing)");
    }
    (Vec::new(), "nowhere")
}

/// A question only this run asks, sent to the mDNS group from a socket on
/// 5353 (shared with mDNSResponder): the group sends it back to this
/// device too, from the device's own address.
fn echoed_address() -> Option<[u8; 4]> {
    let socket = UdpSocket::bind(PORT).ok()?;
    let label = format!("baristore-{}", hex::encode(rand::random::<[u8; 6]>()));
    let id: [u8; 2] = rand::random();
    let mut query = vec![id[0], id[1], 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    query.extend(name(&format!("{label}.local")));
    query.extend_from_slice(&A.to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes());
    socket.send_to(&query, GROUP, PORT).ok()?;
    let started = std::time::Instant::now();
    let mut buf = [0u8; 9000];
    while started.elapsed() < Duration::from_millis(1500) {
        let Ok(Some((n, Some(from)))) = socket.recv_from(&mut buf, Duration::from_millis(300)) else { continue };
        if buf[..n] == query[..] && from != [127, 0, 0, 1] {
            return Some(from);
        }
    }
    None
}

/// The addresses Bonjour gives for `_remotepairing._tcp`: this device's
/// RemotePairing service, when it is the only one on the network.
fn bonjour_addresses() -> Vec<[u8; 4]> {
    let Ok(found) = crate::rootshell::bonjour_lookup("_remotepairing._tcp", 3000) else { return Vec::new() };
    let mut ips: Vec<[u8; 4]> = found.iter().flat_map(|(_, _, _, addrs)| addrs.iter())
        .filter_map(|a| a.parse::<std::net::Ipv4Addr>().ok()).map(|ip| ip.octets()).collect();
    ips.sort();
    ips.dedup();
    if ips.len() == 1 { ips } else { Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (owner, type, class, ttl, rdata) of each answer.
    fn parse(packet: &[u8]) -> Vec<(String, u16, u16, u32, Vec<u8>)> {
        fn read_name(p: &[u8], mut i: usize) -> (String, usize) {
            let mut labels = Vec::new();
            while p[i] != 0 {
                let len = p[i] as usize;
                labels.push(String::from_utf8(p[i + 1..i + 1 + len].to_vec()).unwrap());
                i += 1 + len;
            }
            (labels.join("."), i + 1)
        }
        let count = u16::from_be_bytes([packet[6], packet[7]]) as usize;
        let mut i = 12;
        let mut out = Vec::new();
        for _ in 0..count {
            let (owner, j) = read_name(packet, i);
            let kind = u16::from_be_bytes([packet[j], packet[j + 1]]);
            let class = u16::from_be_bytes([packet[j + 2], packet[j + 3]]);
            let ttl = u32::from_be_bytes(packet[j + 4..j + 8].try_into().unwrap());
            let len = u16::from_be_bytes([packet[j + 8], packet[j + 9]]) as usize;
            out.push((owner, kind, class, ttl, packet[j + 10..j + 10 + len].to_vec()));
            i = j + 10 + len;
        }
        assert_eq!(i, packet.len(), "trailing bytes");
        out
    }

    #[test]
    fn the_announcement_has_ptr_srv_txt_and_a() {
        let ad = Announcement {
            service: "_remotepairing-pairable-host._tcp.local".into(), instance: "ABC-123".into(),
            host: "baristore-1a2b.local".into(), addresses: vec![[192, 168, 1, 20]], port: 51234,
            txt: vec![("name".into(), "BariStore".into()), ("ver".into(), "26".into())],
        };
        let records = parse(&ad.packet(TTL));
        let full = "ABC-123._remotepairing-pairable-host._tcp.local";
        assert_eq!((records[0].0.as_str(), records[0].1, records[0].2), ("_remotepairing-pairable-host._tcp.local", PTR, 1));
        assert_eq!(records[0].4, name(full));
        assert_eq!((records[1].0.as_str(), records[1].1, records[1].2), (full, SRV, 0x8001));
        assert_eq!(&records[1].4[4..6], &51234u16.to_be_bytes());
        assert_eq!(records[2].4, b"\x0ename=BariStore\x06ver=26");
        assert_eq!((records[3].0.as_str(), records[3].1, records[3].4.as_slice()), ("baristore-1a2b.local", A, &[192, 168, 1, 20][..]));
        assert!(records.iter().all(|r| r.3 == TTL));
        assert!(parse(&ad.packet(0)).iter().all(|r| r.3 == 0));
    }

    #[test]
    fn addresses_from_ifconfig() {
        let text = "lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384\n\tinet 127.0.0.1 netmask 0xff000000\n\
                    en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\tether 1a:2b:3c:4d:5e:6f\n\
                    \tinet6 fe80::1%en0 prefixlen 64 secured scopeid 0xb\n\tinet 192.168.1.20 netmask 0xffffff00 broadcast 192.168.1.255\n\
                    utun4: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1500\n\tinet 10.7.0.0 --> 10.7.0.1 netmask 0xffffff00\n\
                    bridge100: flags=8863<UP> mtu 1500\n\tinet 172.20.10.1 netmask 0xfffffff0\n";
        assert_eq!(ifconfig_addresses(text), vec![[192, 168, 1, 20], [172, 20, 10, 1]]);
    }

    /// rootshell's own `ifconfig` on an iPhone runs the line after each
    /// `hwassist=` line into it: Wi-Fi's inet, and a tunnel's header.
    #[test]
    fn addresses_from_rootshells_run_together_ifconfig() {
        let text = "pdp_ip0: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1450\n\
                    \thwassist=703000<CSUM_PARTIAL,MULTIPAGES,TSO_V4,TSO_V6>\tinet 10.142.7.9 --> 10.142.7.9 netmask 0xffffffff\n\
                    ipsec2: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1500\n\
                    \thwassist=703000<CSUM_PARTIAL,TSO_V4>utun0: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1380\n\
                    \tinet 10.7.0.0 --> 10.7.0.1 netmask 0xffffff00\n\
                    en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\
                    \thwassist=20000<VLAN_MTU>\tinet 192.168.1.20 netmask 0xffffff00 broadcast 192.168.1.255\n";
        assert_eq!(ifconfig_addresses(text), vec![[192, 168, 1, 20], [10, 142, 7, 9]]);
        assert!(wifi_joined(text));
    }

    /// On mobile data en0 has no IPv4 address: only the carrier's
    /// interface and LocalDevVPN's tunnel have one.
    #[test]
    fn no_wifi_on_mobile_data() {
        let text = "pdp_ip0: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1450\n\
                    \tinet 10.142.7.9 --> 10.142.7.9 netmask 0xffffffff\n\
                    utun4: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1500\n\tinet 10.7.0.0 --> 10.7.0.1 netmask 0xffffff00\n\
                    en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\tether 1a:2b:3c:4d:5e:6f\n\
                    \tinet6 fe80::1%en0 prefixlen 64 secured scopeid 0xb\n\tstatus: inactive\n";
        assert!(!wifi_joined(text));
        assert_eq!(ifconfig_addresses(text), vec![[10, 142, 7, 9]]);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn announcements_go_to_the_group() {
        let before = crate::rootshell::multicast_sent();
        let advert = Advert::start(Announcement {
            service: "_remotepairing-pairable-host._tcp.local".into(), instance: "X".into(), host: "h.local".into(),
            addresses: vec![[10, 0, 0, 2]], port: 1, txt: vec![],
        }).unwrap();
        drop(advert);
        assert!(crate::rootshell::multicast_sent() >= before + 5);
    }
}
