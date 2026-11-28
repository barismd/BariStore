//! One pairing run in rootshell: announce the pairable host over mDNS
//! (`crate::mdns`) and wait for Settings to connect. The code is chosen as
//! the run starts and shown on the page at once, since the user leaves the
//! page for Settings (Safari stays behind; the Shortcut's web view closes
//! as rootshell goes to the background; atelier-install shows its code
//! before Settings the same way); once the device asks for it (after "Pair
//! with BariStore" and the passcode) it also goes out as a notification,
//! which the user sees over Settings (rootshell posts it in the background
//! too, with its Terminal Notifications on). Up to three tries, each with a
//! new code.
//! rootshell keeps running in the background while its Location Diary Mode
//! is on.

use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use super::host::{self, PairError, PairableHost};
use super::record::{HostIdentity, PairingRecord};
use crate::net::TcpListener;
use crate::rootshell::osc;
use crate::{logf, mdns, rt, state};

pub const DISPLAY_NAME: &str = "BariStore";
const TRIES: u32 = 3;
const WAIT_FOR_DEVICE: Duration = Duration::from_secs(10 * 60);
const WAIT_FOR_CODE: Duration = Duration::from_secs(5 * 60);
/// How often the announcement goes out again while pairing waits.
const ANNOUNCE_EVERY: Duration = Duration::from_secs(20);

#[derive(Clone, Debug)]
pub enum Event {
    /// Announced; the user can go to Settings now, with this code.
    Waiting { code: String },
    /// Settings asks for the code now.
    Code { code: String, attempt: u32 },
    /// The next try takes this new code.
    WrongCode { attempts_left: u32, code: String },
    Paired { name: String },
}

pub fn record_path() -> std::path::PathBuf {
    state::path("pairing.plist")
}

pub fn load_record() -> Option<PairingRecord> {
    PairingRecord::from_bytes(&std::fs::read(record_path()).ok()?)
}

pub fn new_code() -> String {
    format!("{:06}", rand::random::<u32>() % 1_000_000)
}

pub async fn run(events: mpsc::UnboundedSender<Event>) -> Result<PairingRecord, String> {
    let identity = HostIdentity::load_or_create(&state::path("host.plist"))
        .map_err(|e| format!("cannot save the host identity: {e}"))?;
    let (addresses, from) = mdns::own_addresses();
    if addresses.is_empty() {
        return Err("BariStore cannot tell this device's Wi-Fi address. Quit BariStore (Ctrl-C) and start it with \
                    baristore, which notes it (or run: ifconfig > .baristore/ifconfig.txt), then try again.".into());
    }
    let shown: Vec<String> = addresses.iter().map(|a| std::net::Ipv4Addr::from(*a).to_string()).collect();
    logf!("pair: this device's address {} (from {from})", shown.join(", "));
    let port = loop {
        let p = 49200 + rand::random::<u16>() % 12000;
        if p != 62078 && !crate::net::in_use(p) {
            break p;
        }
    };
    let listener = TcpListener::bind(port).map_err(|e| format!("cannot listen for the device: {e}"))?;
    let label = format!("baristore-{}", &identity.identifier.to_lowercase().replace('-', "")[..8]);
    let advert = mdns::Advert::start(mdns::Announcement {
        service: format!("{}.local", host::SERVICE_TYPE), instance: identity.identifier.clone(),
        host: format!("{label}.local"), addresses, port, txt: host::txt_records(&identity, DISPLAY_NAME),
    }).map_err(|e| format!("cannot announce the pairing host on the network: {e}"))?;
    logf!("pair: announced {} on port {port}", identity.identifier);
    let mut code = new_code();
    let _ = events.send(Event::Waiting { code: code.clone() });

    let started = Instant::now();
    let mut attempt = 0;
    let mut announced = Instant::now();
    loop {
        let left = WAIT_FOR_DEVICE.checked_sub(started.elapsed())
            .ok_or("Settings did not connect within 10 minutes")?;
        let accepted = rt::time::timeout(Duration::from_secs(3).min(left), listener.accept()).await;
        if announced.elapsed() >= ANNOUNCE_EVERY {
            if let Err(e) = advert.announce() {
                logf!("pair: announcing again failed: {e}");
            }
            announced = Instant::now();
        }
        let Ok(accepted) = accepted else { continue };
        let stream = accepted.map_err(|e| e.to_string())?;
        attempt += 1;
        logf!("pair: connection {attempt}");
        let log = |line: String| logf!("pair: {line}");
        let events_for_code = events.clone();
        let this_code = code.clone();
        let issue = move || async move {
            let code = this_code;
            let spaced = format!("{} {}", &code[..3], &code[3..]);
            osc::notify(&format!("Pairing code {spaced}"), &format!("Type {spaced} in Settings to pair with {DISPLAY_NAME}."));
            let _ = events_for_code.send(Event::Code { code: code.clone(), attempt });
            code
        };
        let conversation = PairableHost::new(stream, &identity, DISPLAY_NAME, &log).accept(issue);
        match rt::time::timeout(WAIT_FOR_CODE, conversation).await {
            Ok(Ok(peer)) => {
                let record = PairingRecord { host: identity.clone(), peer };
                state::write_private(&record_path(), &record.to_xml())
                    .map_err(|e| format!("paired, but the record could not be saved: {e}"))?;
                logf!("pair: paired with {} ({})", record.peer.name, record.peer.model);
                let _ = events.send(Event::Paired { name: record.peer.name.clone() });
                osc::notify("Paired", "BariStore continues: go back to Safari (or run the BariStore Shortcut).");
                drop(advert);
                return Ok(record);
            }
            Ok(Err(PairError::WrongCode)) => {
                logf!("pair: wrong code on try {attempt}");
                code = new_code();
                let _ = events.send(Event::WrongCode { attempts_left: TRIES.saturating_sub(attempt), code: code.clone() });
            }
            Ok(Err(e)) => logf!("pair: try {attempt} ended: {e}"),
            Err(_) => logf!("pair: try {attempt}: no code typed within 5 minutes"),
        }
        if attempt >= TRIES {
            return Err("Pairing failed after three tries".into());
        }
    }
}
