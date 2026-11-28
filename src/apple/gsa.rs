//! GrandSlam (gsa.apple.com): URL bag, SRP-6a sign-in (s2k / s2k_fo),
//! trusted-device and SMS two-factor codes, and the Xcode app token used by
//! the developer services. The password never leaves this device: it only
//! feeds PBKDF2 and the SRP proof. Request shapes follow nab138/isideload (MIT).

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use aes_gcm::aead::{generic_array::typenum::U16, Aead, KeyInit, Payload};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use hmac::{Hmac, Mac};
use plist::{Dictionary, Value};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;

use super::anisette::{self, Anisette, Claim, ClientInfo, Headers, SERVERS};
use crate::{logf, srp, state};

const URL_BAG: &str = "https://gsa.apple.com/grandslam/GsService2/lookup";
/// The anisette server list SideStore and iloader read.
const SERVER_LIST: &str = "https://servers.sidestore.io/servers.json";
/// GrandSlam's client: HTTP/1.1 with its fields as written (title case),
/// TLS checked by rootshell against Apple's trust store, which has Apple's
/// own roots.
pub fn http_client() -> crate::http::Client {
    crate::http::Client::builder()
        // A fresh connection per request: GSA has answered reused
        // connections with 5xx (SideSign 35993d7, isideload f6a4d5d).
        .fresh_connections()
        .timeout(std::time::Duration::from_secs(60))
        .build()
}

#[derive(Debug)]
pub enum LoginError {
    /// -20101 / -20970 / -22406, with Apple's own words. -22406 can also mean
    /// Apple is throttling, or did not believe the anisette identity.
    WrongPassword(String),
    /// Apple refused this anisette identity (-80045/-80046/-23782): try another server.
    IdentityRefused(String),
    /// Apple's front end answered HTTP 503: it refuses the client claimed.
    ServiceRefused(String),
    /// Apple asked for a text-message code and refused to send one (a bare
    /// 403, or its own words): another client may get one.
    NoCode(String),
    /// Retrying cannot help now (429, locked account).
    Stop(String),
    Cancelled,
    Other(String),
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginError::WrongPassword(why) => write!(f,
                "Apple rejected the email or password ({why}). If you are sure they are right, Apple may be \
                 limiting sign-ins: wait a while before trying again"),
            LoginError::Stop(why) => write!(f, "{why}"),
            LoginError::ServiceRefused(why) => write!(f, "Apple's sign-in server refused every client BariStore can claim to be ({why}). Try again later or update BariStore"),
            LoginError::IdentityRefused(why) => write!(f, "Apple refused the sign-in identity ({why})"),
            LoginError::NoCode(why) => write!(f,
                "Apple asked for a verification code but could not send one ({why}), whichever client BariStore \
                 signed in as. If this Apple ID has no trusted phone number or Apple device (one made without a \
                 phone, say), add a phone number at account.apple.com (Sign-In and Security), or sign in with it \
                 once on an iPhone, iPad or Mac; else wait a while and try again"),
            LoginError::Cancelled => write!(f, "sign-in cancelled"),
            LoginError::Other(why) => write!(f, "{why}"),
        }
    }
}

fn other(e: impl std::fmt::Display) -> LoginError {
    LoginError::Other(e.to_string())
}

/// What the user is asked for a second-factor code: where it went, and
/// the other ways to get one (answer, label): a trusted number texted
/// ("text:<id>") or called ("call:<id>"), the trusted devices ("devices"),
/// the same way again ("again").
pub struct CodePrompt {
    pub message: String,
    pub options: Vec<(String, String)>,
}

/// Asks the user for a 2FA code, or another way; None cancels.
pub type CodeAsker = dyn Fn(CodePrompt) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync;

pub struct GrandSlam {
    pub http: crate::http::Client,
    pub anisette: tokio::sync::Mutex<Anisette>,
    pub info: ClientInfo,
    bag: Dictionary,
    /// The developer services' last headers: one set serves the requests of
    /// a few seconds, as AltStore and SideStore use one per operation.
    recent: std::sync::Mutex<Option<(std::time::Instant, Headers)>>,
}

fn check(response: &Dictionary) -> Result<(), LoginError> {
    let status = response.get("Status").and_then(Value::as_dictionary);
    let code = status.and_then(|s| s.get("ec")).and_then(Value::as_signed_integer).unwrap_or(0);
    if code == 0 {
        return Ok(());
    }
    let message = status.and_then(|s| s.get("em")).and_then(Value::as_string).unwrap_or("").to_string();
    let what = format!("{code} {message}");
    logf!("gsa: Apple answered {what}");
    Err(match code {
        -20101 | -20970 | -22406 => LoginError::WrongPassword(what),
        // -22410: an identity provisioned under another client info (iloader #1304).
        -80045 | -80046 | -23782 | -22410 => LoginError::IdentityRefused(what),
        -20209 => LoginError::Stop(format!("Apple locked this Apple Account for security reasons ({what}); \
                                             unlock it at iforgot.apple.com")),
        _ => LoginError::Other(format!("Apple error {what}")),
    })
}

impl GrandSlam {
    pub fn base_headers(&self, json: bool) -> crate::http::HeaderMap {
        let mut h = crate::http::HeaderMap::new();
        let kind = if json { "application/json" } else { "text/x-xml-plist" };
        h.insert("Content-Type", kind);
        h.insert("Accept", kind);
        h.insert("X-Mme-Client-Info", self.info.client_info.clone());
        h.insert("User-Agent", self.info.user_agent.clone());
        h.insert("X-Xcode-Version", "14.2 (14C18)");
        h.insert("X-Apple-App-Info", "com.apple.gs.xcode.auth");
        h
    }

    pub async fn connect(server: &str, advertised: &ClientInfo, claim: Claim) -> Result<GrandSlam, String> {
        let http = http_client();
        let anisette = Anisette::new(server, advertised, claim)
            .ok_or_else(|| format!("{server} cannot sign in as {}", claim.name()))?;
        let info = anisette.info.clone();
        let mut gs = GrandSlam { http, anisette: tokio::sync::Mutex::new(anisette), info, bag: Dictionary::new(),
                                 recent: std::sync::Mutex::new(None) };
        let text = gs.http.get(URL_BAG).headers(gs.base_headers(false)).send().await
            .map_err(|e| format!("gsa.apple.com: {e}"))?
            .text().await.map_err(|e| e.to_string())?;
        let bag: Dictionary = plist::from_bytes(text.as_bytes()).map_err(|e| format!("URL bag: {e}"))?;
        gs.bag = bag.get("urls").and_then(Value::as_dictionary).cloned().ok_or("URL bag without urls")?;
        Ok(gs)
    }

    pub fn url(&self, key: &str) -> Result<String, String> {
        self.bag.get(key).and_then(Value::as_string).map(str::to_string)
            .ok_or_else(|| format!("Apple's URL bag has no {key}"))
    }

    pub async fn plist_post(&self, url: &str, body: &Dictionary, extra: Option<crate::http::HeaderMap>)
        -> Result<Dictionary, String>
    {
        let mut req = self.http.post(url).headers(self.base_headers(false));
        if let Some(extra) = extra {
            req = req.headers(extra);
        }
        let body = crate::pairing::record::plist_xml(body);
        // A 429 is often momentary (iloader 2.3.4): ask again after 2 s and 5 s.
        let mut delays = [2u64, 5].into_iter();
        let resp = loop {
            let resp = req.try_clone().ok_or("request cannot be repeated")?
                .body(body.clone()).send().await.map_err(|e| e.to_string())?;
            match delays.next() {
                Some(d) if resp.status() == crate::http::StatusCode::TOO_MANY_REQUESTS => {
                    logf!("gsa: 429 from Apple; asking again in {d} s");
                    crate::rt::time::sleep(std::time::Duration::from_secs(d)).await;
                }
                _ => break resp,
            }
        };
        let status = resp.status();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            // GrandSlam explains some refusals (401) in a plist with a Status.
            if let Some(r) = plist::from_bytes::<Dictionary>(text.as_bytes()).ok()
                .and_then(|d| d.get("Response").and_then(Value::as_dictionary).cloned())
                .filter(|r| r.contains_key("Status"))
            {
                logf!("gsa: HTTP {status} with a Status");
                return Ok(r);
            }
            return Err(format!("HTTP {status} from Apple: {}", text.chars().take(200).collect::<String>()));
        }
        let d: Dictionary = plist::from_bytes(text.as_bytes())
            .map_err(|_| format!("Apple answered with something that is not a plist: {}",
                                 text.chars().take(200).collect::<String>()))?;
        Ok(d.get("Response").and_then(Value::as_dictionary).cloned().unwrap_or(d))
    }

    pub async fn headers(&self) -> Result<Headers, String> {
        let mut a = self.anisette.lock().await;
        a.headers(&self.http, self, None).await
    }

    /// Headers for the developer services, reused for 30 s: a refresh asks
    /// for many profiles at once, and each new set costs the anisette server
    /// a round trip.
    pub async fn portal_headers(&self) -> Result<Headers, String> {
        let mut a = self.anisette.lock().await;
        if let Some((at, h)) = self.recent.lock().unwrap().as_ref() {
            if at.elapsed() < std::time::Duration::from_secs(30) {
                return Ok(h.clone());
            }
        }
        let h = a.headers(&self.http, self, None).await?;
        *self.recent.lock().unwrap() = Some((std::time::Instant::now(), h.clone()));
        Ok(h)
    }

    async fn resync(&self, sim: &str) -> Result<Headers, String> {
        let mut a = self.anisette.lock().await;
        a.headers(&self.http, self, Some(sim)).await
    }
}

/// What a signed-in account keeps (no password).
#[derive(Clone, Debug)]
pub struct Account {
    pub email: String,
    pub adsid: String,
    pub idms_token: String,
    pub session_key: Vec<u8>,
    pub c: Vec<u8>,
    pub first_name: String,
    pub last_name: String,
    pub xcode_token: String,
    pub xcode_expiry_ms: u64,
}

impl Account {
    pub fn save(&self) -> std::io::Result<()> {
        let mut d = Dictionary::new();
        let s = |v: &str| Value::String(v.into());
        d.insert("email".into(), s(&self.email));
        d.insert("adsid".into(), s(&self.adsid));
        d.insert("GsIdmsToken".into(), s(&self.idms_token));
        d.insert("sk".into(), Value::Data(self.session_key.clone()));
        d.insert("c".into(), Value::Data(self.c.clone()));
        d.insert("firstName".into(), s(&self.first_name));
        d.insert("lastName".into(), s(&self.last_name));
        d.insert("xcodeToken".into(), s(&self.xcode_token));
        d.insert("xcodeExpiry".into(), Value::Integer(self.xcode_expiry_ms.into()));
        state::write_private(&state::path("account.plist"), &crate::pairing::record::plist_xml(&d))
    }

    pub fn load() -> Option<Account> {
        let d = plist::from_file::<_, Value>(state::path("account.plist")).ok()?.into_dictionary()?;
        let s = |k: &str| d.get(k).and_then(Value::as_string).unwrap_or("").to_string();
        let b = |k: &str| d.get(k).and_then(Value::as_data).map(<[u8]>::to_vec).unwrap_or_default();
        Some(Account {
            email: s("email"), adsid: s("adsid"), idms_token: s("GsIdmsToken"), session_key: b("sk"),
            c: b("c"), first_name: s("firstName"), last_name: s("lastName"), xcode_token: s("xcodeToken"),
            xcode_expiry_ms: d.get("xcodeExpiry").and_then(Value::as_unsigned_integer).unwrap_or(0),
        }).filter(|a| !a.email.is_empty())
    }

    pub fn token_fresh(&self) -> bool {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        self.xcode_expiry_ms > now.as_millis() as u64 + 10 * 60 * 1000
    }
}

/// A plist dictionary, also when Apple sends a bare `<dict>` without the
/// `<plist>` root.
fn bare_plist(data: &[u8]) -> Option<Dictionary> {
    let trimmed = std::str::from_utf8(data).ok()?.trim();
    if trimmed.starts_with("<dict") {
        plist::from_bytes(format!("<plist version=\"1.0\">{trimmed}</plist>").as_bytes()).ok()
    } else {
        plist::from_bytes(data).ok()
    }
}

fn session_hmac(key: &[u8], label: &str) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).unwrap();
    mac.update(label.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

enum Stage {
    LoggedIn(Dictionary),
    /// Trusted-device code; `sent` when Apple already pushed one (hsc 409
    /// without `au`), so none is requested again.
    TrustedDevice { spd: Dictionary, sent: bool },
    Sms(Dictionary),
}

async fn srp_login(gs: &GrandSlam, email: &str, password: &str) -> Result<Stage, LoginError> {
    let url = gs.url("gsService").map_err(other)?;
    let headers = gs.headers().await.map_err(other)?;
    let group = srp::Group::rfc5054_2048_sha256();
    let client = srp::Client::new(&group);

    let init = |headers: &Headers| {
        let mut init = Dictionary::new();
        init.insert("A2k".into(), Value::Data(client.public()));
        init.insert("cpd".into(), Value::Dictionary(headers.cpd()));
        init.insert("o".into(), Value::String("init".into()));
        init.insert("ps".into(), Value::Array(vec![Value::String("s2k".into()), Value::String("s2k_fo".into())]));
        init.insert("u".into(), Value::String(email.into()));
        request_body(init)
    };
    let mut r1 = gs.plist_post(&url, &init(&headers), None).await.map_err(gsa_failure)?;
    // Windows-library servers get hsc 434 (-22421) on a machine's first
    // sign-in, with resync data for ADI; once applied, the same request works.
    let mut headers = headers;
    if let Some(sim) = resync_sim(&r1) {
        logf!("gsa: Apple asked for an anisette resync (hsc 434)");
        headers = gs.resync(&sim).await.map_err(other)?;
        r1 = gs.plist_post(&url, &init(&headers), None).await.map_err(gsa_failure)?;
    }
    check(&r1)?;
    let data = |d: &Dictionary, k: &str| d.get(k).and_then(Value::as_data).map(<[u8]>::to_vec)
        .ok_or_else(|| other(format!("Apple's answer lacks {k}")));
    let salt = data(&r1, "s")?;
    let b_pub = data(&r1, "B")?;
    let iterations = r1.get("i").and_then(Value::as_signed_integer).ok_or_else(|| other("no iteration count"))?;
    let c = r1.get("c").and_then(Value::as_string).ok_or_else(|| other("no c"))?.to_string();
    let protocol = r1.get("sp").and_then(Value::as_string).unwrap_or("s2k").to_string();
    if protocol != "s2k" && protocol != "s2k_fo" {
        return Err(other(format!("Apple asked for an unsupported protocol: {protocol}")));
    }
    let digest = Sha256::digest(password.as_bytes());
    let input: Vec<u8> = if protocol == "s2k_fo" { hex::encode(digest).into_bytes() } else { digest.to_vec() };
    let mut derived = [0u8; 32];
    pbkdf2::pbkdf2::<Hmac<Sha256>>(&input, &salt, iterations as u32, &mut derived).map_err(other)?;
    let x = srp::x_without_user(&group, &salt, &derived);
    let session = client.process(email.as_bytes(), &salt, &b_pub, &x).map_err(other)?;

    let mut complete = Dictionary::new();
    complete.insert("M1".into(), Value::Data(session.proof.clone()));
    complete.insert("c".into(), Value::String(c));
    complete.insert("cpd".into(), Value::Dictionary(headers.cpd()));
    complete.insert("o".into(), Value::String("complete".into()));
    complete.insert("u".into(), Value::String(email.into()));
    let r2 = gs.plist_post(&url, &request_body(complete), None).await.map_err(gsa_failure)?;
    check(&r2)?;
    if !session.verify_server(&data(&r2, "M2")?) {
        return Err(other("Apple's server proof did not verify"));
    }
    let spd_sealed = data(&r2, "spd")?;
    // isideload keys the session data on S, pysrp-based clients on H(S);
    // take whichever opens it (as the earlier Python sign-in did).
    let hashed = Sha256::digest(&session.premaster).to_vec();
    let spd = [&session.premaster, &hashed].into_iter().find_map(|base| {
        let key = session_hmac(base, "extra data key:");
        let iv = session_hmac(base, "extra data iv:");
        let raw = cbc::Decryptor::<aes::Aes256>::new_from_slices(&key, &iv[..16]).ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(&spd_sealed).ok()?;
        bare_plist(&raw)
    }).ok_or_else(|| other("cannot decrypt Apple's session data"))?;
    let status = r2.get("Status").and_then(Value::as_dictionary).cloned().unwrap_or_default();
    match status.get("au").and_then(Value::as_string) {
        Some("trustedDeviceSecondaryAuth") => {
            logf!("gsa: Apple asks for a code from the trusted devices");
            return Ok(Stage::TrustedDevice { spd, sent: false });
        }
        Some("secondaryAuth") => {
            logf!("gsa: Apple asks for a code by phone");
            return Ok(Stage::Sms(spd));
        }
        // No second factor set up on the account: nothing more to do.
        Some("repair") => logf!("gsa: no second factor on this Apple ID (repair)"),
        None => {}
        Some(other_step) => logf!("gsa: unknown extra step {other_step}; continuing"),
    }
    if status.get("hsc").and_then(Value::as_signed_integer) == Some(409) {
        logf!("gsa: Apple sent a code to the trusted devices (hsc 409)");
        return Ok(Stage::TrustedDevice { spd, sent: true });
    }
    Ok(Stage::LoggedIn(spd))
}

/// HTTP failures from gsa.apple.com: a 503 is Apple refusing the request
/// before it reads the anisette data, and a 429 left after the retries is
/// throttling, so no other server would do better.
fn gsa_failure(e: String) -> LoginError {
    if e.contains("HTTP 503") {
        LoginError::ServiceRefused("HTTP 503".into())
    } else if e.contains("HTTP 429") {
        LoginError::Stop("Apple is limiting sign-ins for this account or network. Wait a while: every attempt \
                          can extend the wait".into())
    } else {
        LoginError::Other(e)
    }
}

/// The resync data (base64) of a response whose Status asks for an anisette
/// resync (hsc 434).
fn resync_sim(response: &Dictionary) -> Option<String> {
    let status = response.get("Status")?.as_dictionary()?;
    if status.get("hsc").and_then(Value::as_signed_integer) != Some(434) {
        return None;
    }
    match status.get("X-Apple-I-MD-DATA")? {
        Value::String(s) => Some(s.clone()),
        Value::Data(d) => Some(B64.encode(d)),
        _ => None,
    }
}

fn request_body(request: Dictionary) -> Dictionary {
    let mut header = Dictionary::new();
    header.insert("Version".into(), Value::String("1.0.1".into()));
    let mut body = Dictionary::new();
    body.insert("Header".into(), Value::Dictionary(header));
    body.insert("Request".into(), Value::Dictionary(request));
    body
}

async fn second_factor_headers(gs: &GrandSlam, spd: &Dictionary, json: bool) -> Result<crate::http::HeaderMap, LoginError> {
    let a = gs.headers().await.map_err(other)?;
    let mut h = gs.base_headers(json);
    for (k, v) in a.pairs() {
        h.insert(k, v);
    }
    let s = |k: &str| spd.get(k).and_then(Value::as_string).unwrap_or("").to_string();
    let identity = B64.encode(format!("{}:{}", s("adsid"), s("GsIdmsToken")));
    h.insert("X-Apple-Identity-Token", identity);
    h.insert("X-Apple-I-MD-RINFO", a.routing_info);
    Ok(h)
}

/// Where a second-factor code goes: the trusted devices, or a trusted
/// number, texted or called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    Devices,
    Phone { id: u64, call: bool },
}

/// A trusted number as /auth lists it: its id, how it shows ("+90 (•••)
/// •••-••12") and whether it takes calls only (a landline).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Number {
    id: u64,
    shown: String,
    calls_only: bool,
}

/// The account's ways to get a code, from GET /auth: its trusted numbers,
/// and whether Apple says it has no trusted devices.
#[derive(Debug, Default, PartialEq)]
struct Ways {
    numbers: Vec<Number>,
    no_devices: bool,
}

fn ways_of(reply: &serde_json::Value) -> Ways {
    let numbers = reply.get("trustedPhoneNumbers").and_then(|n| n.as_array()).map(|list| {
        list.iter().filter_map(|n| Some(Number {
            id: n.get("id")?.as_u64()?,
            shown: n.get("numberWithDialCode").and_then(|v| v.as_str()).unwrap_or("your trusted number").to_string(),
            calls_only: n.get("pushMode").and_then(|m| m.as_str()) == Some("voice"),
        })).collect()
    }).unwrap_or_default();
    Ways { numbers, no_devices: reply.get("noTrustedDevices").and_then(|v| v.as_bool()).unwrap_or(false) }
}

async fn account_ways(gs: &GrandSlam, spd: &Dictionary) -> Result<Ways, LoginError> {
    let resp = gs.http.get("https://gsa.apple.com/auth").headers(second_factor_headers(gs, spd, true).await?)
        .send().await.map_err(other)?;
    let status = resp.status();
    let reply: serde_json::Value = resp.json().await.unwrap_or_default();
    let ways = ways_of(&reply);
    logf!("gsa: the account's ways ({status}): {} trusted number(s), trusted devices {}, type {}", ways.numbers.len(),
          if ways.no_devices { "none" } else { "maybe" },
          reply.get("authenticationType").and_then(|v| v.as_str()).unwrap_or("?"));
    Ok(ways)
}

/// Apple's first `serviceErrors` entry of a JSON answer: its code and words.
fn service_error(reply: &serde_json::Value) -> Option<(String, String)> {
    let e = reply.get("serviceErrors")?.as_array()?.first()?;
    let text = |k: &str| e.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let words: Vec<String> = [text("title"), text("message")].into_iter().filter(|t| !t.is_empty()).collect();
    Some((text("code"), words.join(": ")))
}

/// Asks Apple to send a code `way`: None when one is on its way (or the
/// last one still holds), else why Apple would not. The devices' request may
/// be turned away (a bare 403) with a code pushed all the same. A number:
/// PUT /auth/verify/phone, "sms" or "voice" (a call reads the code out); a
/// 412 names the number Apple has when ours is not it (its id is not always
/// 1, xtool #424); -22979/-22981 (codes sent recently) leave the last one
/// valid; a bare 403 (Apple refusing the old Mac's client since October
/// 2026) or e.g. -28248 is a refusal.
async fn send_code(gs: &GrandSlam, spd: &Dictionary, way: &mut Way) -> Result<Option<String>, LoginError> {
    let Way::Phone { mut id, call } = *way else {
        let send = gs.url("trustedDeviceSecondaryAuth").map_err(other)?;
        let status = gs.http.get(&send).headers(second_factor_headers(gs, spd, false).await?).send().await
            .map_err(other)?.status();
        logf!("gsa: code to the trusted devices requested ({status})");
        return Ok(None);
    };
    let mode = if call { "voice" } else { "sms" };
    for retried in [false, true] {
        let body = serde_json::json!({"phoneNumber": {"id": id}, "mode": mode});
        let resp = gs.http.put("https://gsa.apple.com/auth/verify/phone")
            .headers(second_factor_headers(gs, spd, true).await?).json(&body).send().await.map_err(other)?;
        let status = resp.status();
        let reply: serde_json::Value = resp.json().await.unwrap_or_default();
        let error = service_error(&reply);
        logf!("gsa: code by {mode} to number {id} requested ({status}){}",
              error.as_ref().map(|(code, words)| format!(": {code} {words}")).unwrap_or_default());
        *way = Way::Phone { id, call };
        if status.is_success() {
            return Ok(None);
        }
        if status == crate::http::StatusCode::PRECONDITION_FAILED {
            match ways_of(&reply).numbers.first().map(|n| n.id) {
                Some(theirs) if theirs != id && !retried => {
                    id = theirs;
                    continue;
                }
                _ => return Ok(None),
            }
        }
        return Ok(match error {
            Some((code, _)) if code == "-22979" || code == "-22981" => None,
            Some((code, words)) => Some(format!("{code} {words}")),
            None => Some(format!("HTTP {status}")),
        });
    }
    Ok(None)
}

/// Whether Apple takes `code` for `way` (Ok(false): a wrong code).
async fn check_code(gs: &GrandSlam, spd: &Dictionary, way: Way, code: &str) -> Result<bool, LoginError> {
    let Way::Phone { id, call } = way else {
        let validate = gs.url("validateCode").map_err(other)?;
        let text = gs.http.get(&validate).headers(second_factor_headers(gs, spd, false).await?)
            .header("security-code", code).send().await.map_err(other)?
            .text().await.map_err(other)?;
        let reply: Dictionary = plist::from_bytes(text.as_bytes()).unwrap_or_default();
        return match check(&reply) {
            Ok(()) => Ok(true),
            Err(LoginError::Other(e)) if e.contains("-21669") => Ok(false),
            Err(e) => Err(e),
        };
    };
    let mode = if call { "voice" } else { "sms" };
    let body = serde_json::json!({"securityCode": {"code": code}, "phoneNumber": {"id": id}, "mode": mode});
    let resp = gs.http.post("https://gsa.apple.com/auth/verify/phone/securitycode")
        .headers(second_factor_headers(gs, spd, true).await?).json(&body).send().await.map_err(other)?;
    if resp.status().is_success() {
        return Ok(true);
    }
    match service_error(&resp.json().await.unwrap_or_default()) {
        Some((code, words)) if code != "-21669" => Err(other(format!("Apple refused the code: {code} {words}"))),
        _ => Ok(false),
    }
}

/// What the user sees for `way`, with every other way on offer.
fn prompt(way: Way, ways: &Ways, hint: Option<String>) -> CodePrompt {
    let shown = |id: u64| ways.numbers.iter().find(|n| n.id == id).map(|n| n.shown.clone())
        .unwrap_or_else(|| "your trusted number".into());
    let sent = match way {
        Way::Devices => "Enter the code Apple sent to your Apple devices.".to_string(),
        Way::Phone { id, call: true } => format!("Apple is calling {}: enter the code it reads out.", shown(id)),
        Way::Phone { id, call: false } => format!("Enter the code Apple texted to {}.", shown(id)),
    };
    let mut options = Vec::new();
    for n in &ways.numbers {
        if !n.calls_only {
            options.push((format!("text:{}", n.id), format!("Text {}", n.shown)));
        }
        options.push((format!("call:{}", n.id), format!("Call {}", n.shown)));
    }
    if !ways.no_devices {
        options.push(("devices".into(), "Send it to my Apple devices".into()));
    }
    options.push(("again".into(), "Send the code again".into()));
    CodePrompt { message: hint.map_or(sent.clone(), |h| format!("{h} {sent}")), options }
}

/// What the user answered: a code, or another way.
#[derive(Debug, PartialEq)]
enum Answer {
    Code(String),
    Way(Way),
    Again,
}

fn answer_of(text: &str) -> Answer {
    let text = text.trim();
    let phone = |id: &str, call| id.parse().ok().map(|id| Answer::Way(Way::Phone { id, call }));
    let way = match text.split_once(':') {
        Some(("text", id)) => phone(id, false),
        Some(("call", id)) => phone(id, true),
        _ => None,
    };
    way.unwrap_or_else(|| match text {
        "devices" => Answer::Way(Way::Devices),
        "again" => Answer::Again,
        _ => Answer::Code(text.chars().filter(char::is_ascii_digit).collect()),
    })
}

/// The second factor: a code from the trusted devices (`first` None) or a
/// trusted number, with the other ways on offer, as SideStore and siboot
/// offer them: each number texted or called, the devices, the same again.
/// Apple refusing to send the code the way it asked for, or an account with
/// nowhere to send one (no trusted number or device: an Apple ID made
/// without a phone), is NoCode at once, so the other client is tried: Apple
/// may not ask that one for a code.
async fn second_factor(gs: &GrandSlam, spd: &Dictionary, by_phone: bool, sent: bool, ask: &CodeAsker)
    -> Result<(), LoginError>
{
    let ways = account_ways(gs, spd).await?;
    if ways.numbers.is_empty() && ways.no_devices {
        return Err(LoginError::NoCode("this Apple ID has no trusted phone number or Apple device".into()));
    }
    // `secondaryAuth` names no number: the first that takes texts (Apple's id 1 when /auth lists none).
    let mut way = if by_phone {
        ways.numbers.iter().find(|n| !n.calls_only).or(ways.numbers.first())
            .map_or(Way::Phone { id: 1, call: false }, |n| Way::Phone { id: n.id, call: n.calls_only })
    } else {
        Way::Devices
    };
    let (mut send, mut chosen, mut hint) = (!sent, false, None);
    for _ in 0..8 {
        if send {
            if let Some(why) = send_code(gs, spd, &mut way).await? {
                if !chosen {
                    return Err(LoginError::NoCode(why));
                }
                hint = Some(format!("Apple did not send it that way ({why}): choose another."));
            }
            send = false;
        }
        match answer_of(&ask(prompt(way, &ways, hint.take())).await.ok_or(LoginError::Cancelled)?) {
            Answer::Way(next) => (way, send, chosen) = (next, true, true),
            Answer::Again => (send, chosen) = (true, true),
            Answer::Code(code) if code.len() == 6 => {
                if check_code(gs, spd, way, &code).await? {
                    return Ok(());
                }
                hint = Some("That code was wrong. Try again.".into());
            }
            Answer::Code(_) => hint = Some("The code has 6 digits.".into()),
        }
    }
    Err(other("too many tries for the verification code"))
}

pub async fn app_token(gs: &GrandSlam, account: &mut Account) -> Result<(), LoginError> {
    let app = "com.apple.gs.xcode.auth";
    let headers = gs.headers().await.map_err(other)?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&account.session_key).map_err(other)?;
    mac.update(b"apptokens");
    mac.update(account.adsid.as_bytes());
    mac.update(app.as_bytes());
    let mut request = Dictionary::new();
    request.insert("app".into(), Value::Array(vec![Value::String(app.into())]));
    request.insert("c".into(), Value::Data(account.c.clone()));
    request.insert("checksum".into(), Value::Data(mac.finalize().into_bytes().to_vec()));
    request.insert("cpd".into(), Value::Dictionary(headers.cpd()));
    request.insert("o".into(), Value::String("apptokens".into()));
    request.insert("u".into(), Value::String(account.adsid.clone()));
    request.insert("t".into(), Value::String(account.idms_token.clone()));
    let url = gs.url("gsService").map_err(other)?;
    let reply = gs.plist_post(&url, &request_body(request), None).await.map_err(gsa_failure)?;
    check(&reply)?;
    let sealed = reply.get("et").and_then(Value::as_data).ok_or_else(|| other("no encrypted token"))?;
    if sealed.len() < 35 || &sealed[..3] != b"XYZ" {
        return Err(other("unknown app token format"));
    }
    type Gcm16 = aes_gcm::AesGcm<aes::Aes256, U16>;
    let cipher = Gcm16::new_from_slice(&account.session_key).map_err(other)?;
    let opened = cipher.decrypt(aes_gcm::aead::generic_array::GenericArray::from_slice(&sealed[3..19]),
                                Payload { msg: &sealed[19..], aad: &sealed[..3] })
        .map_err(|_| other("cannot decrypt the app token"))?;
    let token = bare_plist(&opened).ok_or_else(|| other("the app token is not a plist"))?;
    if token.get("status-code").and_then(Value::as_signed_integer) != Some(200) {
        return Err(other(format!("app token refused: {token:?}")));
    }
    let entry = token.get("t").and_then(Value::as_dictionary).and_then(|t| t.get(app))
        .and_then(Value::as_dictionary).ok_or_else(|| other("no xcode token"))?;
    account.xcode_token = entry.get("token").and_then(Value::as_string).unwrap_or("").to_string();
    account.xcode_expiry_ms = entry.get("expiry").and_then(Value::as_unsigned_integer).unwrap_or(0);
    Ok(())
}

/// Full sign-in on one anisette server.
async fn login_on(gs: &GrandSlam, email: &str, password: &str, ask: &CodeAsker) -> Result<Account, LoginError> {
    let mut stage = srp_login(gs, email, password).await?;
    for _ in 0..2 {
        match stage {
            Stage::LoggedIn(_) => break,
            Stage::TrustedDevice { ref spd, sent } => second_factor(gs, spd, false, sent, ask).await?,
            Stage::Sms(ref spd) => second_factor(gs, spd, true, false, ask).await?,
        }
        stage = srp_login(gs, email, password).await?;
    }
    let Stage::LoggedIn(spd) = stage else {
        return Err(other("Apple kept asking for a second factor"));
    };
    let s = |k: &str| spd.get(k).and_then(Value::as_string).unwrap_or("").to_string();
    let b = |k: &str| spd.get(k).and_then(Value::as_data).map(<[u8]>::to_vec).unwrap_or_default();
    let mut account = Account {
        email: email.to_string(), adsid: s("adsid"), idms_token: s("GsIdmsToken"), session_key: b("sk"),
        c: b("c"), first_name: s("fn"), last_name: s("ln"), xcode_token: String::new(), xcode_expiry_ms: 0,
    };
    app_token(gs, &mut account).await?;
    Ok(account)
}

/// One sign-in attempt's place: a server and the client it claims to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub server: String,
    pub claim: Claim,
}

impl Route {
    fn save(&self) {
        let _ = std::fs::write(state::path("anisette-server"), format!("{}\n{}", self.server, self.claim.name()));
    }

    fn saved() -> Option<Route> {
        let text = std::fs::read_to_string(state::path("anisette-server")).ok()?;
        let mut lines = text.lines().map(str::trim);
        let server = lines.next().filter(|s| !s.is_empty())?.to_string();
        let claim = lines.next().and_then(Claim::from_name).unwrap_or(Claim::Advertised);
        Some(Route { server, claim })
    }
}

/// Signs in through every server and claim in turn until one works.
///
/// * HTTP 503 from Apple, or -80045/-80046/-23782/-22410: Apple refused that
///   client or identity before looking at the password; the next claim or
///   server is tried. A claim refused with 503 on two servers is skipped
///   after that (SideInstaller's AppleRefusals).
/// * No text message (NoCode): the other claim is tried; that claim is not
///   tried again on other servers (Apple decides it by the client).
/// * -20101/-20970/-22406 (credentials) stop at once: more attempts with
///   the same password risk a lock.
/// * 429 and a locked account stop at once.
pub async fn login(email: &str, password: &str, ask: &CodeAsker, status: &dyn Fn(String))
    -> Result<(GrandSlam, Account), LoginError>
{
    let email = email.trim().to_lowercase();
    let mut last = LoginError::Other("no anisette server answered".into());
    let (mut refused, mut no_code): (Vec<Claim>, Vec<Claim>) = (Vec::new(), Vec::new());
    let mut attempts = 0;
    let saved = Route::saved();
    for server in candidate_servers().await {
        let http = http_client();
        let advertised = match Anisette::advertised(&http, &server).await {
            Ok(a) => a,
            Err(e) => {
                logf!("gsa: {e}");
                last = LoginError::Other(e);
                continue;
            }
        };
        let mut claims = Claim::ALL.to_vec();
        if let Some(r) = saved.as_ref().filter(|r| r.server == server) {
            claims.retain(|c| *c != r.claim);
            claims.insert(0, r.claim);
        }
        for claim in claims {
            if refused.iter().filter(|c| **c == claim).count() >= 2 || no_code.contains(&claim) {
                continue;
            }
            let gs = match GrandSlam::connect(&server, &advertised, claim).await {
                Ok(gs) => gs,
                Err(e) => {
                    logf!("gsa: {e}");
                    continue;
                }
            };
            let host = server.trim_start_matches("https://");
            attempts += 1;
            status(if attempts == 1 { "Signing in to Apple".into() }
                   else { format!("Signing in to Apple (attempt {attempts})") });
            logf!("gsa: trying {host} as {} ({})", claim.name(), gs.info.client_info);
            match login_on(&gs, &email, password, ask).await {
                Ok(account) => {
                    Route { server: server.clone(), claim }.save();
                    return Ok((gs, account));
                }
                Err(LoginError::ServiceRefused(why)) => {
                    logf!("gsa: {host} as {}: {why}", claim.name());
                    refused.push(claim);
                    last = LoginError::ServiceRefused(why);
                }
                Err(LoginError::NoCode(why)) => {
                    logf!("gsa: {host} as {}: Apple sent no text message ({why})", claim.name());
                    no_code.push(claim);
                    last = LoginError::NoCode(why);
                }
                Err(LoginError::IdentityRefused(why)) => {
                    logf!("gsa: {host} as {}: identity refused ({why})", claim.name());
                    gs.anisette.lock().await.forget();
                    last = LoginError::IdentityRefused(why);
                }
                Err(e @ (LoginError::WrongPassword(_) | LoginError::Stop(_) | LoginError::Cancelled)) => return Err(e),
                Err(LoginError::Other(why)) => {
                    logf!("gsa: {host} as {}: {why}", claim.name());
                    last = LoginError::Other(why);
                }
            }
        }
    }
    Err(last)
}

/// Reuses the saved session: refreshes the Xcode token without a password,
/// through the server and claim that signed in (the token belongs to that
/// anisette identity).
pub async fn resume() -> Result<(GrandSlam, Account), String> {
    let mut account = Account::load().ok_or("not signed in")?;
    let route = Route::saved().unwrap_or_else(|| Route {
        server: anisette::chosen().unwrap_or_else(|| SERVERS[0].into()), claim: Claim::Advertised,
    });
    let advertised = Anisette::advertised(&http_client(), &route.server).await?;
    let gs = GrandSlam::connect(&route.server, &advertised, route.claim).await?;
    if !account.token_fresh() {
        app_token(&gs, &mut account).await.map_err(|e| e.to_string())?;
        let _ = account.save();
    }
    Ok((gs, account))
}

/// The server chosen in Options alone; else the saved server first, then
/// ours, then macOS- and Windows-library servers from SideStore's live list
/// (kind 2 and 1) we do not know yet.
async fn candidate_servers() -> Vec<String> {
    if let Some(server) = anisette::chosen() {
        return vec![server];
    }
    let mut list: Vec<String> = Route::saved().map(|r| r.server).into_iter().collect();
    for s in SERVERS {
        if !list.iter().any(|l| l == s) {
            list.push(s.to_string());
        }
    }
    let live = async {
        let v: serde_json::Value = crate::store::client().get(SERVER_LIST)
            .timeout(std::time::Duration::from_secs(10)).send().await.ok()?.json().await.ok()?;
        Some(v.get("servers")?.as_array()?.iter()
            .filter(|s| matches!(s.get("kind").and_then(|k| k.as_u64()), Some(1 | 2)))
            .filter_map(|s| s.get("address")?.as_str().map(|a| a.trim_end_matches('/').to_string()))
            .filter(|a| a.starts_with("https://"))
            .collect::<Vec<_>>())
    };
    for s in live.await.unwrap_or_default() {
        if !list.contains(&s) {
            list.push(s);
        }
    }
    list
}


/// Whether Apple takes `claim` through `server` right now, asked with a
/// made-up account (for `baristore doctor`): -20970 means Apple read the
/// request and looked the account up; HTTP 503 or -8004x mean it refused the
/// client or identity. No real account or password is involved.
pub async fn probe(server: &str, claim: Claim) -> String {
    let advertised = match Anisette::advertised(&http_client(), server).await {
        Ok(a) => a,
        Err(e) => return e,
    };
    let gs = match GrandSlam::connect(server, &advertised, claim).await {
        Ok(gs) => gs,
        Err(e) => return e,
    };
    let email = format!("baristore.probe{:08x}@icloud.com", rand::random::<u32>());
    let never: &CodeAsker = &|_| Box::pin(async { None });
    match login_on(&gs, &email, "not-a-password", never).await {
        Err(LoginError::WrongPassword(why)) if why.starts_with("-20970") => "Apple accepts it".into(),
        Err(LoginError::ServiceRefused(why)) => format!("Apple refuses it ({why})"),
        Err(e) => format!("{e:?}"),
        Ok(_) => "signed in?".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The account's ways and Apple's own words, from the JSON answers of
    /// /auth and /auth/verify/phone (bodies as xtool #424 and iloader #814
    /// show them); the menu of other ways and the answers to it.
    #[test]
    fn second_factor_ways() {
        let listed = serde_json::json!({"trustedPhoneNumbers": [
            {"id": 3, "pushMode": "voice", "numberWithDialCode": "+90 (•••) •••-••12"},
            {"id": 2, "pushMode": "sms", "nonFTEU": true}], "authenticationType": "hsa2"});
        let ways = ways_of(&listed);
        assert_eq!(ways.numbers, vec![
            Number { id: 3, shown: "+90 (•••) •••-••12".into(), calls_only: true },
            Number { id: 2, shown: "your trusted number".into(), calls_only: false }]);
        assert!(!ways.no_devices);
        assert_eq!(ways_of(&serde_json::json!({"noTrustedDevices": true})), Ways { numbers: vec![], no_devices: true });
        let menu = prompt(Way::Phone { id: 2, call: false }, &ways, Some("That code was wrong. Try again.".into()));
        assert_eq!(menu.message, "That code was wrong. Try again. Enter the code Apple texted to your trusted number.");
        let values: Vec<&str> = menu.options.iter().map(|(v, _)| v.as_str()).collect();
        assert_eq!(values, ["call:3", "text:2", "call:2", "devices", "again"]);
        assert_eq!(answer_of("call:3"), Answer::Way(Way::Phone { id: 3, call: true }));
        assert_eq!(answer_of("text:2"), Answer::Way(Way::Phone { id: 2, call: false }));
        assert_eq!(answer_of("devices"), Answer::Way(Way::Devices));
        assert_eq!(answer_of("again"), Answer::Again);
        assert_eq!(answer_of(" 123-456 "), Answer::Code("123456".into()));
        let refused = serde_json::json!({"serviceErrors": [{"code": "-28248", "title": "Verification Failed",
            "message": "Verification codes can't be sent to this phone number at this time."}]});
        assert_eq!(service_error(&refused), Some(("-28248".into(),
            "Verification Failed: Verification codes can't be sent to this phone number at this time.".into())));
        assert_eq!(service_error(&serde_json::Value::Null), None);
    }
}
