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
const APPLE_ROOT: &[u8] = include_bytes!("../../assets/certs/AppleRootCA.cer");

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_der(APPLE_ROOT).expect("Apple root"))
        .http1_title_case_headers()
        .http1_only()
        // A fresh connection per request: GSA has answered reused
        // connections with 5xx (SideSign 35993d7, isideload f6a4d5d).
        .pool_max_idle_per_host(0)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("HTTP client")
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeKind {
    TrustedDevice,
    Sms,
}

/// Asks the user for a 2FA code; None cancels.
pub type CodeAsker = dyn Fn(CodeKind, Option<String>) -> Pin<Box<dyn Future<Output = Option<String>> + Send>>
    + Send + Sync;

pub struct GrandSlam {
    pub http: reqwest::Client,
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
    pub fn base_headers(&self, json: bool) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        let kind = if json { "application/json" } else { "text/x-xml-plist" };
        h.insert("Content-Type", kind.parse().unwrap());
        h.insert("Accept", kind.parse().unwrap());
        if let Ok(v) = self.info.client_info.parse() {
            h.insert("X-Mme-Client-Info", v);
        }
        if let Ok(v) = self.info.user_agent.parse() {
            h.insert("User-Agent", v);
        }
        h.insert("X-Xcode-Version", "14.2 (14C18)".parse().unwrap());
        h.insert("X-Apple-App-Info", "com.apple.gs.xcode.auth".parse().unwrap());
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

    pub async fn plist_post(&self, url: &str, body: &Dictionary, extra: Option<reqwest::header::HeaderMap>)
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
                Some(d) if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                    logf!("gsa: 429 from Apple; asking again in {d} s");
                    tokio::time::sleep(std::time::Duration::from_secs(d)).await;
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
    /// a round trip (and Minis a TLS handshake).
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
    let au = status.get("au").and_then(Value::as_string);
    let hsc = status.get("hsc").and_then(Value::as_signed_integer);
    // What Apple asked for, in the log; `repair` means no second factor.
    logf!("gsa: after sign-in Apple asked au={} hsc={}",
          au.unwrap_or("-"), hsc.map(|h| h.to_string()).unwrap_or_else(|| "-".into()));
    match au {
        Some("trustedDeviceSecondaryAuth") => {
            logf!("gsa: Apple asks for a code from the trusted devices");
            return Ok(Stage::TrustedDevice { spd, sent: false });
        }
        Some("secondaryAuth") => {
            logf!("gsa: Apple asks for a code by phone");
            return Ok(Stage::Sms(spd));
        }
        // No second factor set up on the account: nothing more to do;
        // `au=repair` wins over `hsc==409` (SideStore, isideload).
        Some("repair") => {
            logf!("gsa: no second factor on this Apple ID (repair)");
            return Ok(Stage::LoggedIn(spd));
        }
        None => {}
        Some(other_step) => logf!("gsa: unknown extra step {other_step}; continuing"),
    }
    if hsc == Some(409) {
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

async fn second_factor_headers(gs: &GrandSlam, spd: &Dictionary, json: bool) -> Result<reqwest::header::HeaderMap, LoginError> {
    let a = gs.headers().await.map_err(other)?;
    let mut h = gs.base_headers(json);
    for (k, v) in a.pairs() {
        h.insert(k, v.parse().map_err(other)?);
    }
    let s = |k: &str| spd.get(k).and_then(Value::as_string).unwrap_or("").to_string();
    let identity = B64.encode(format!("{}:{}", s("adsid"), s("GsIdmsToken")));
    h.insert("X-Apple-Identity-Token", identity.parse().map_err(other)?);
    h.insert("X-Apple-I-MD-RINFO", a.routing_info.parse().map_err(other)?);
    Ok(h)
}

/// The trusted-device code. Apple may turn the request away (a bare 403)
/// and still have pushed a code, so the user is asked either way; answering
/// "sms" switches to a text message.
async fn trusted_device(gs: &GrandSlam, spd: &Dictionary, sent: bool, ask: &CodeAsker) -> Result<(), LoginError> {
    let send = gs.url("trustedDeviceSecondaryAuth").map_err(other)?;
    let validate = gs.url("validateCode").map_err(other)?;
    // No trusted device and no trusted number (an Apple ID made without a
    // phone): no code can be sent; the caller tries another client.
    // A failed GET /auth does not block asking for a code; only Apple
    // saying there is no trusted number or device does.
    if no_way_to_get_a_code(&account_ways(gs, spd).await.unwrap_or_default()) {
        return Err(LoginError::NoCode(NO_WAY_TO_GET_A_CODE.into()));
    }
    if !sent {
        let status = gs.http.get(&send).headers(second_factor_headers(gs, spd, false).await?).send().await
            .map_err(other)?.status();
        logf!("gsa: trusted-device code requested ({status})");
    }
    let mut hint = None;
    for _ in 0..5 {
        let code = ask(CodeKind::TrustedDevice, hint.take()).await.ok_or(LoginError::Cancelled)?;
        if code.trim() == "sms" {
            return sms(gs, spd, ask).await;
        }
        let text = gs.http.get(&validate).headers(second_factor_headers(gs, spd, false).await?)
            .header("security-code", code.trim()).send().await.map_err(other)?
            .text().await.map_err(other)?;
        let reply: Dictionary = plist::from_bytes(text.as_bytes()).unwrap_or_default();
        match check(&reply) {
            Ok(()) => return Ok(()),
            Err(LoginError::Other(e)) if e.contains("-21669") => hint = Some("That code was wrong. Try again.".into()),
            Err(e) => return Err(e),
        }
    }
    Err(other("too many wrong codes"))
}

/// One trusted number as /auth lists it: its id and whether it takes text
/// messages (pushMode "sms").
#[derive(Debug, PartialEq, Eq)]
struct Number {
    id: u64,
    sms: bool,
}

/// The account's ways to get a second-factor code, from GET /auth: its
/// trusted numbers, and whether Apple says it has no trusted devices.
#[derive(Debug, Default, PartialEq, Eq)]
struct Ways {
    numbers: Vec<Number>,
    no_devices: bool,
}

fn ways_of(reply: &serde_json::Value) -> Ways {
    let numbers = reply.get("trustedPhoneNumbers").and_then(|n| n.as_array()).map(|list| {
        list.iter().filter_map(|n| Some(Number {
            id: n.get("id")?.as_u64()?,
            sms: n.get("pushMode").and_then(|m| m.as_str()) == Some("sms"),
        })).collect()
    }).unwrap_or_default();
    Ways { numbers, no_devices: reply.get("noTrustedDevices").and_then(|v| v.as_bool()).unwrap_or(false) }
}

/// The id to text in `trustedPhoneNumbers` (GET /auth, or a 412's body):
/// the first that takes text messages, else the first; None when /auth
/// lists no trusted number.
fn phone_to_text(ways: &Ways) -> Option<u64> {
    ways.numbers.iter().find(|n| n.sms).or(ways.numbers.first()).map(|n| n.id)
}

/// No second-factor code can be sent: no trusted number and Apple says no
/// trusted device (an Apple ID made without a phone). An absent
/// `noTrustedDevices` is not proof of no device.
fn no_way_to_get_a_code(ways: &Ways) -> bool {
    ways.numbers.is_empty() && ways.no_devices
}

/// `LoginError::NoCode`'s `why` when Apple has nowhere to send a code.
const NO_WAY_TO_GET_A_CODE: &str = "this Apple ID has no trusted phone number or Apple device";

/// GET https://gsa.apple.com/auth with the second-factor identity: the
/// account's trusted numbers and devices.
async fn account_ways(gs: &GrandSlam, spd: &Dictionary) -> Result<Ways, LoginError> {
    let resp = gs.http.get("https://gsa.apple.com/auth")
        .headers(second_factor_headers(gs, spd, true).await?).send().await.map_err(other)?;
    let status = resp.status();
    let reply: serde_json::Value = resp.json().await.unwrap_or_default();
    let ways = ways_of(&reply);
    logf!("gsa: the account's ways ({status}): {} trusted number(s), noTrustedDevices {}, type {}",
          ways.numbers.len(), ways.no_devices,
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

/// The text-message code, sent when asked for (PUT /auth/verify/phone).
/// Apple refusing to send it (a bare 403 since October 2026 for the old
/// Mac's client info, or e.g. -28248 "codes can't be sent to this phone
/// number") is NoCode: no code will come, so the user is not asked for one.
/// A 412 names the number Apple has when ours is not it (its id is not
/// always 1, xtool #424), else a code is on its way; -22979/-22981 (codes
/// sent recently) leave the last one valid.
async fn sms(gs: &GrandSlam, spd: &Dictionary, ask: &CodeAsker) -> Result<(), LoginError> {
    let numbers = gs.http.get("https://gsa.apple.com/auth")
        .headers(second_factor_headers(gs, spd, true).await?).send().await.map_err(other)?;
    let listed = numbers.status();
    let listed_reply = numbers.json().await.unwrap_or_default();
    // No trusted number listed (and no trusted device): no text message
    // can be sent (not id 1); `noTrustedDevices` needs no extra check.
    let ways = ways_of(&listed_reply);
    let mut id = phone_to_text(&ways).ok_or_else(|| LoginError::NoCode(NO_WAY_TO_GET_A_CODE.into()))?;
    logf!("gsa: trusted phone numbers asked ({listed}); texting number {id}");
    for retried in [false, true] {
        let body = serde_json::json!({"phoneNumber": {"id": id}, "mode": "sms"});
        let resp = gs.http.put("https://gsa.apple.com/auth/verify/phone")
            .headers(second_factor_headers(gs, spd, true).await?).json(&body).send().await.map_err(other)?;
        let status = resp.status();
        let reply: serde_json::Value = resp.json().await.unwrap_or_default();
        let error = service_error(&reply);
        logf!("gsa: text-message code requested ({status}){}",
              error.as_ref().map(|(code, words)| format!(": {code} {words}")).unwrap_or_default());
        if status.is_success() {
            break;
        }
        if status == reqwest::StatusCode::PRECONDITION_FAILED {
            match phone_to_text(&ways_of(&reply)) {
                Some(theirs) if theirs != id && !retried => {
                    id = theirs;
                    continue;
                }
                _ => break,
            }
        }
        match error {
            Some((code, _)) if code == "-22979" || code == "-22981" => break,
            Some((code, words)) => return Err(LoginError::NoCode(format!("{code} {words}"))),
            None => return Err(LoginError::NoCode(format!("HTTP {status}"))),
        }
    }
    let mut hint = None;
    for _ in 0..5 {
        let code = ask(CodeKind::Sms, hint.take()).await.ok_or(LoginError::Cancelled)?;
        let body = serde_json::json!({"securityCode": {"code": code.trim()}, "phoneNumber": {"id": id}, "mode": "sms"});
        let resp = gs.http.post("https://gsa.apple.com/auth/verify/phone/securitycode")
            .headers(second_factor_headers(gs, spd, true).await?).json(&body).send().await.map_err(other)?;
        if resp.status().is_success() {
            return Ok(());
        }
        match service_error(&resp.json().await.unwrap_or_default()) {
            Some((code, words)) if code != "-21669" => return Err(other(format!("Apple refused the code: {code} {words}"))),
            _ => hint = Some("That code was wrong. Try again.".into()),
        }
    }
    Err(other("too many wrong codes"))
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
            Stage::TrustedDevice { ref spd, sent } => trusted_device(gs, spd, sent, ask).await?,
            Stage::Sms(ref spd) => sms(gs, spd, ask).await?,
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
pub async fn login(email: &str, password: &str, ask: &CodeAsker, status: &(dyn Fn(String) + Send + Sync))
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
                    logf!("gsa: {host} as {}: no code can be sent ({why})", claim.name());
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
        let v: serde_json::Value = reqwest::Client::new().get(SERVER_LIST)
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
    let never: &CodeAsker = &|_, _| Box::pin(async { None });
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

    /// The number to text and Apple's own words, from the JSON answers of
    /// /auth and /auth/verify/phone (bodies as xtool #424 and iloader #814
    /// show them).
    #[test]
    fn text_message_answers() {
        let listed = serde_json::json!({"trustedPhoneNumbers": [
            {"id": 3, "pushMode": "voice"}, {"id": 2, "pushMode": "sms", "nonFTEU": true}]});
        assert_eq!(phone_to_text(&ways_of(&listed)), Some(2));
        assert_eq!(phone_to_text(&ways_of(&serde_json::json!({"trustedPhoneNumbers": [{"id": 4}]}))), Some(4));
        assert_eq!(phone_to_text(&ways_of(&serde_json::Value::Null)), None);
        let refused = serde_json::json!({"serviceErrors": [{"code": "-28248", "title": "Verification Failed",
            "message": "Verification codes can't be sent to this phone number at this time."}]});
        assert_eq!(service_error(&refused), Some(("-28248".into(),
            "Verification Failed: Verification codes can't be sent to this phone number at this time.".into())));
        assert_eq!(service_error(&serde_json::Value::Null), None);
    }

    /// GET /auth's account ways for the second factor: trusted numbers and
    /// whether Apple says no trusted device exists; an Apple ID with no
    /// trusted number or device (one made without a phone) has no way to
    /// receive a code.
    #[test]
    fn account_ways_from_auth() {
        let reply = serde_json::json!({"authenticationType": "hsa2", "noTrustedDevices": true, "trustedPhoneNumbers": [
            {"id": 3, "pushMode": "voice", "numberWithDialCode": "+90 (\u{2022}\u{2022}\u{2022}) \u{2022}\u{2022}\u{2022}-\u{2022}\u{2022}12"},
            {"id": 2, "pushMode": "sms"}]});
        let ways = ways_of(&reply);
        assert_eq!(ways.numbers, vec![Number { id: 3, sms: false }, Number { id: 2, sms: true }]);
        assert!(ways.no_devices);
        assert!(!no_way_to_get_a_code(&ways)); // a trusted number can still receive a code
        assert_eq!(phone_to_text(&ways), Some(2)); // the number that takes texts
    }

    /// `noTrustedDevices` without any trusted number means no code can be
    /// sent; an absent flag is not proof of no device.
    #[test]
    fn no_way_when_no_number_and_no_device() {
        assert!(no_way_to_get_a_code(&ways_of(&serde_json::json!({"noTrustedDevices": true}))));
        assert!(no_way_to_get_a_code(&ways_of(&serde_json::json!({"noTrustedDevices": true, "trustedPhoneNumbers": []}))));
        assert!(!no_way_to_get_a_code(&ways_of(&serde_json::json!({}))));
        assert!(!no_way_to_get_a_code(&ways_of(&serde_json::json!({"noTrustedDevices": false}))));
        assert!(!no_way_to_get_a_code(&ways_of(
            &serde_json::json!({"noTrustedDevices": true, "trustedPhoneNumbers": [{"id": 1}]}))));
        assert!(!no_way_to_get_a_code(&Ways { numbers: vec![], no_devices: false }));
    }
}
