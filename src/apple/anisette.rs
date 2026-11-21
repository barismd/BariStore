//! Anisette v3: device-identity headers minted by a remote server that runs
//! Apple's ADI. The server never sees the Apple ID or password: it gets a
//! random keychain identifier and relays Apple's provisioning blobs.
//!
//! Since October 2026 Apple refuses a sign-in whose claimed platform differs
//! from the anisette data (-80045) and refuses the Android claim (-80046), so
//! we use Mac-emulating servers and send exactly the client_info/user_agent
//! the server reports (/v3/client_info).

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use futures_util::{SinkExt, StreamExt};
use plist::{Dictionary, Value};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::Message;

use super::gsa::GrandSlam;
use crate::{logf, state};

/// Mac-emulating v3 servers, tried in order (fail over on identity refusals).
pub const SERVERS: &[&str] = &[
    "https://ani.iloader.app",
    "https://mac-ani.jkcoxson.com",
    "https://ani.jkcoxson.com",
    "https://ani.sidestore.io",
];

#[derive(Clone, Debug, Deserialize)]
pub struct ClientInfo {
    pub client_info: String,
    pub user_agent: String,
}

#[derive(Clone)]
pub struct Anisette {
    pub server: String,
    pub info: ClientInfo,
    keychain_id: [u8; 16],
    adi_pb: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct Headers {
    pub machine_id: String,
    pub otp: String,
    pub routing_info: String,
    pub device_id: String,
}

fn state_path(server: &str) -> std::path::PathBuf {
    let host = server.trim_start_matches("https://").replace(['/', ':'], "_");
    state::path(&format!("anisette-{host}.plist"))
}

impl Anisette {
    pub async fn connect(http: &reqwest::Client, server: &str) -> Result<Anisette, String> {
        let info: ClientInfo = http.get(format!("{server}/v3/client_info"))
            .send().await.map_err(|e| format!("{server}: {e}"))?
            .error_for_status().map_err(|e| format!("{server}: {e}"))?
            .json().await.map_err(|e| format!("{server}: bad client_info: {e}"))?;
        let mut a = Anisette { server: server.into(), info, keychain_id: rand::random(), adi_pb: None };
        if let Ok(Value::Dictionary(d)) = plist::from_file::<_, Value>(state_path(server)) {
            if let (Some(id), Some(pb)) = (d.get("keychain_identifier").and_then(Value::as_data),
                                           d.get("adi_pb").and_then(Value::as_data)) {
                if let Ok(id) = id.try_into() {
                    a.keychain_id = id;
                    a.adi_pb = Some(pb.to_vec());
                }
            }
        }
        Ok(a)
    }

    pub fn device_id(&self) -> String {
        uuid::Uuid::from_bytes(self.keychain_id).to_string()
    }

    fn local_user(&self) -> String {
        hex::encode(Sha256::digest(self.keychain_id))
    }

    pub fn forget(&mut self) {
        self.keychain_id = rand::random();
        self.adi_pb = None;
        let _ = std::fs::remove_file(state_path(&self.server));
    }

    fn save(&self) {
        let mut d = Dictionary::new();
        d.insert("keychain_identifier".into(), Value::Data(self.keychain_id.to_vec()));
        if let Some(pb) = &self.adi_pb {
            d.insert("adi_pb".into(), Value::Data(pb.clone()));
        }
        let _ = state::write_private(&state_path(&self.server), &crate::pairing::record::plist_xml(&d));
    }

    /// Fresh headers, provisioning first when needed.
    pub async fn headers(&mut self, http: &reqwest::Client, gs: &GrandSlam) -> Result<Headers, String> {
        for _ in 0..3 {
            if self.adi_pb.is_none() {
                self.provision(gs).await?;
            }
            let body = serde_json::json!({
                "identifier": B64.encode(self.keychain_id),
                "adi_pb": B64.encode(self.adi_pb.as_ref().unwrap()),
            });
            let reply: serde_json::Value = http.post(format!("{}/v3/get_headers", self.server))
                .json(&body).send().await.map_err(|e| e.to_string())?
                .error_for_status().map_err(|e| e.to_string())?
                .json().await.map_err(|e| e.to_string())?;
            let text = |k: &str| reply.get(k).and_then(|v| v.as_str()).map(str::to_string);
            match text("result").as_deref() {
                Some("Headers") => return Ok(Headers {
                    machine_id: text("X-Apple-I-MD-M").unwrap_or_default(),
                    otp: text("X-Apple-I-MD").unwrap_or_default(),
                    routing_info: text("X-Apple-I-MD-RINFO").unwrap_or_else(|| "17106176".into()),
                    device_id: self.device_id(),
                }),
                _ => {
                    let message = text("message").unwrap_or_else(|| reply.to_string());
                    logf!("anisette: {} get_headers: {message}", self.server);
                    if message.contains("-45061") {
                        self.forget();
                        continue;
                    }
                    return Err(format!("anisette server refused: {message}"));
                }
            }
        }
        Err("anisette provisioning kept failing".into())
    }

    async fn provision(&mut self, gs: &GrandSlam) -> Result<(), String> {
        let start = gs.url("midStartProvisioning")?;
        let finish = gs.url("midFinishProvisioning")?;
        let ws_url = format!("{}/v3/provisioning_session", self.server).replacen("https://", "wss://", 1);
        let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await
            .map_err(|e| format!("anisette provisioning socket: {e}"))?;
        let mut extra = reqwest::header::HeaderMap::new();
        extra.insert("X-Apple-I-MD-LU", self.local_user().parse().unwrap());
        extra.insert("X-Mme-Device-Id", self.device_id().parse().unwrap());
        loop {
            let msg = tokio::time::timeout(std::time::Duration::from_secs(30), ws.next()).await
                .map_err(|_| "anisette provisioning timed out".to_string())?
                .ok_or("anisette provisioning socket closed")?
                .map_err(|e| e.to_string())?;
            let Message::Text(text) = msg else { continue };
            let m: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let field = |k: &str| m.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let reply = match field("result").as_str() {
                "GiveIdentifier" => serde_json::json!({"identifier": B64.encode(self.keychain_id)}),
                "GiveStartProvisioningData" => {
                    let mut body = Dictionary::new();
                    body.insert("Header".into(), Value::Dictionary(Dictionary::new()));
                    body.insert("Request".into(), Value::Dictionary(Dictionary::new()));
                    let r = gs.plist_post(&start, &body, Some(extra.clone())).await?;
                    serde_json::json!({"spim": r.get("spim").and_then(Value::as_string).unwrap_or("")})
                }
                "GiveEndProvisioningData" => {
                    let mut request = Dictionary::new();
                    request.insert("cpim".into(), Value::String(field("cpim")));
                    let mut body = Dictionary::new();
                    body.insert("Header".into(), Value::Dictionary(Dictionary::new()));
                    body.insert("Request".into(), Value::Dictionary(request));
                    let r = gs.plist_post(&finish, &body, Some(extra.clone())).await?;
                    let s = |k: &str| r.get(k).and_then(Value::as_string).unwrap_or("").to_string();
                    serde_json::json!({"ptm": s("ptm"), "tk": s("tk")})
                }
                "ProvisioningSuccess" => {
                    self.adi_pb = Some(B64.decode(field("adi_pb")).map_err(|e| e.to_string())?);
                    let _ = ws.close(None).await;
                    self.save();
                    logf!("anisette: provisioned with {}", self.server);
                    return Ok(());
                }
                other => return Err(format!("anisette provisioning failed: {other} {}", field("message"))),
            };
            ws.send(Message::Text(reply.to_string().into())).await.map_err(|e| e.to_string())?;
        }
    }
}

impl Headers {
    /// The cpd ("client provided data") dictionary GrandSlam requests carry.
    pub fn cpd(&self) -> Dictionary {
        let mut d = Dictionary::new();
        for (k, v) in [("bootstrap", "true"), ("icscrec", "true"), ("loc", "en_US"),
                       ("pbe", "false"), ("prkgen", "true"), ("svct", "iCloud")] {
            d.insert(k.into(), Value::String(v.into()));
        }
        for (k, v) in self.pairs() {
            d.insert(k.into(), Value::String(v));
        }
        d
    }

    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("X-Mme-Device-Id", self.device_id.clone()),
            ("X-Apple-I-MD", self.otp.clone()),
            ("X-Apple-I-MD-M", self.machine_id.clone()),
        ]
    }
}
