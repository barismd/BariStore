//! One pairing run inside Open Minis: advertise through minis-bonjour, keep
//! Minis alive with silence, show the code *before* the user leaves for
//! Settings (and as a notification), accept up to three tries.

use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use super::host::{self, PairError, PairableHost};
use super::record::{HostIdentity, PairingRecord};
use crate::{logf, minis, state};

pub const DISPLAY_NAME: &str = "BariStore";
const TRIES: u32 = 3;
const WAIT_FOR_DEVICE: Duration = Duration::from_secs(10 * 60);
const WAIT_FOR_CODE: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug)]
pub enum Event {
    /// Advertising; the user can go to Settings now and type `code`.
    Ready { code: String, keepalive: bool, notification: bool },
    Connected { attempt: u32 },
    WrongCode { attempts_left: u32 },
    Paired { udid: String, name: String },
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
    let code = new_code();
    minis::withdraw_stale(host::SERVICE_TYPE).await;

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    let player = match minis::Player::start(&state::home()).await {
        Ok(p) => Some(p),
        Err(e) => {
            logf!("pair: keep-alive unavailable: {e}");
            None
        }
    };
    let advert = minis::advertise(host::SERVICE_TYPE, &identity.identifier, port,
                                  &host::txt_records(&identity, DISPLAY_NAME))
        .await
        .map_err(|e| format!("Minis could not advertise the pairing host: {e}"))?;
    logf!("pair: advertising {} on guest port {port} ({})", advert.name, advert.state);
    let spaced = format!("{} {}", &code[..3], &code[3..]);
    let notification = minis::notify(
        "Pair with BariStore",
        &format!("Settings › Privacy & Security › Developer Mode › Pair with {DISPLAY_NAME}. Code {spaced}"),
    ).await.is_ok();
    let _ = events.send(Event::Ready { code: code.clone(), keepalive: player.is_some(), notification });

    let started = Instant::now();
    let mut attempt = 0;
    let mut last_check = Instant::now();
    loop {
        let left = WAIT_FOR_DEVICE.checked_sub(started.elapsed())
            .ok_or("no iPhone connected within 10 minutes")?;
        let accepted = tokio::select! {
            accepted = listener.accept() => Some(accepted),
            _ = tokio::time::sleep(Duration::from_secs(3).min(left)) => None,
        };
        if last_check.elapsed() > Duration::from_secs(3) {
            if let Some(p) = &player {
                let status = p.status().await;
                if status != "playing" {
                    logf!("pair: player is {status}; resuming");
                    p.resume().await;
                }
            }
            last_check = Instant::now();
        }
        let Some(accepted) = accepted else { continue };
        let (stream, peer) = accepted.map_err(|e| e.to_string())?;
        attempt += 1;
        logf!("pair: connection {attempt} from {peer}");
        let _ = events.send(Event::Connected { attempt });
        let log = |line: String| logf!("pair: {line}");
        let conversation = PairableHost::new(stream, &identity, DISPLAY_NAME, &log).accept(&code);
        match tokio::time::timeout(WAIT_FOR_CODE, conversation).await {
            Ok(Ok(peer)) => {
                let record = PairingRecord { host: identity.clone(), peer };
                state::write_private(&record_path(), &record.to_xml())
                    .map_err(|e| format!("paired, but the record could not be saved: {e}"))?;
                logf!("pair: paired with {} ({})", record.peer.name, record.peer.model);
                let _ = events.send(Event::Paired {
                    udid: record.peer.udid.clone(), name: record.peer.name.clone() });
                let _ = minis::notify("Paired", "Go back to Open Minis: BariStore continues.").await;
                drop(advert);
                drop(player);
                return Ok(record);
            }
            Ok(Err(PairError::WrongCode)) => {
                logf!("pair: wrong code on try {attempt}");
                let _ = events.send(Event::WrongCode { attempts_left: TRIES.saturating_sub(attempt) });
            }
            Ok(Err(e)) => logf!("pair: try {attempt} ended: {e}"),
            Err(_) => logf!("pair: try {attempt}: no code typed within 5 minutes"),
        }
        if attempt >= TRIES {
            return Err("pairing did not finish after three tries".into());
        }
    }
}
