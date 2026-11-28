//! The page: a tiny HTTP/1.1 server under a random path, the page's key
//! (other apps and websites on the device can reach a loopback port, but
//! not guess the path), Host checked against the loopback (no DNS
//! rebinding; rootshell's listener takes connections on every address the
//! device has, so this also keeps the network out). Safari opens it (main.rs
//! `open_page`). A file handed to another app (Install to LiveContainer)
//! has an address of its own, see `Share`. The store quits after three
//! minutes without a tap on the page or a task (`IDLE`): rootshell keeps it
//! running behind other apps for as long as it runs (Location Diary Mode).

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::app::App;
use crate::install::Options;
use crate::logf;
use crate::net::{TcpListener, TcpStream};
use crate::rt;

const PAGE: &str = include_str!("../ui/index.html");
const LOGO: &[u8] = include_bytes!("../assets/baristore.svg");
/// A request's head must come this soon (browsers open connections ahead
/// of need and may never use them).
const HEAD_WAIT: Duration = Duration::from_secs(15);
/// An upload that sends nothing for this long has stopped.
const BODY_WAIT: Duration = Duration::from_secs(60);
/// How long the store waits without a tap on its page or a task before it
/// quits.
const IDLE: Duration = Duration::from_secs(180);

/// The page's key: the address's path, kept across restarts (main.rs
/// `Running`).
static KEY: Mutex<String> = Mutex::new(String::new());

pub fn set_key(key: &str) {
    *KEY.lock().unwrap_or_else(|e| e.into_inner()) = key.to_string();
}

/// When the page was last used (a tap, an upload) or a task last ran.
static LAST_USE: Mutex<Option<std::time::Instant>> = Mutex::new(None);

fn used() {
    *LAST_USE.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
}

/// Quits the store once it has been idle for `IDLE`.
async fn quit_when_idle(app: App) {
    used();
    loop {
        rt::time::sleep(Duration::from_secs(5)).await;
        if app.busy() {
            used();
            continue;
        }
        let since = LAST_USE.lock().unwrap_or_else(|e| e.into_inner()).map_or(Duration::ZERO, |t| t.elapsed());
        if since >= IDLE {
            logf!("idle for {} minutes: quitting", IDLE.as_secs() / 60);
            crate::state::end(0);
        }
    }
}

/// Serves the page on `listener`: each connection in a task of its own.
pub async fn serve(app: App, listener: TcpListener) {
    let _ = PORT.set(listener.port());
    rt::spawn(quit_when_idle(app.clone()));
    loop {
        let Ok(stream) = listener.accept().await else { continue };
        let app = app.clone();
        rt::spawn(async move {
            let _ = handle(stream, app).await;
        });
    }
}

async fn respond(stream: &mut TcpStream, status: &str, kind: &str, body: &[u8]) -> std::io::Result<()> {
    respond_cached(stream, status, kind, body, "no-store").await
}

/// One send for head and body: each send is a call to rootshell.
async fn respond_cached(stream: &mut TcpStream, status: &str, kind: &str, body: &[u8], cache: &str)
    -> std::io::Result<()>
{
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: {cache}\r\n\
         X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n", body.len())
        .into_bytes();
    out.extend_from_slice(body);
    stream.write_all(&out).await?;
    stream.flush().await
}

/// One read, or an error once `wait` passed without anything.
async fn read_some(stream: &mut TcpStream, buf: &mut [u8], wait: Duration) -> std::io::Result<usize> {
    rt::time::timeout(wait, stream.read(buf)).await
        .unwrap_or_else(|_| Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "nothing came")))
}

async fn handle(mut stream: TcpStream, app: App) -> std::io::Result<()> {
    let port = PORT.get().copied().unwrap_or(0);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = read_some(&mut stream, &mut chunk, HEAD_WAIT).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 64 * 1024 {
            return respond(&mut stream, "431 Too Large", "text/plain", b"").await;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request = lines.next().unwrap_or("");
    let mut parts = request.split(' ');
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let mut host = "";
    let mut length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "host" => host = v.trim(),
                "content-length" => length = v.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return respond(&mut stream, "403 Forbidden", "text/plain", b"").await;
    }
    if let (Some(rest), "GET") = (path.strip_prefix("/share/"), method) {
        return send_share(&mut stream, rest).await;
    }
    let rest = path.strip_prefix('/').unwrap_or(path);
    let (key, route) = rest.find('/').map_or((rest, ""), |i| (&rest[..i], &rest[i..]));
    let known = { let k = KEY.lock().unwrap_or_else(|e| e.into_inner()); !k.is_empty() && *k == key };
    if !known {
        return respond(&mut stream, "404 Not Found", "text/plain", b"").await;
    }
    if route.is_empty() {
        // The page's address without its last '/': the page's own
        // addresses are relative to it.
        let head = format!("HTTP/1.1 301 Moved Permanently\r\nLocation: /{key}/\r\nContent-Length: 0\r\n\
                            Connection: close\r\n\r\n");
        return stream.write_all(head.as_bytes()).await;
    }
    if method == "POST" && route.starts_with("/api/upload-icon") {
        // A picture for an app's icon (the page sends it square, 512 px).
        if length > 4 << 20 {
            return respond(&mut stream, "413 Too Large", "text/plain", b"").await;
        }
        let mut body = buf[header_end..].to_vec();
        while body.len() < length {
            let n = read_some(&mut stream, &mut chunk, BODY_WAIT).await?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
        let reply = match rt::block_in_place(|| crate::icons::save_custom_icon(&body)) {
            Ok(name) => json!({"ok": true, "icon": name, "url": crate::icons::custom_icon_url(&name)}),
            Err(e) => json!({"ok": false, "message": e}),
        };
        return respond(&mut stream, "200 OK", "application/json", reply.to_string().as_bytes()).await;
    }
    if method == "POST" && route.starts_with("/api/upload") {
        used();
        let reply = upload(&mut stream, &buf[header_end..], length).await;
        used();
        return respond(&mut stream, "200 OK", "application/json", reply.to_string().as_bytes()).await;
    }
    if length > 1 << 20 {
        return respond(&mut stream, "413 Too Large", "text/plain", b"").await;
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < length {
        let n = read_some(&mut stream, &mut chunk, BODY_WAIT).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let (route, query) = route.split_once('?').unwrap_or((route, ""));
    match (method, route) {
        ("GET", "/icon") => {
            let url = query.split('&').find_map(|p| p.strip_prefix("u=")).map(percent_decode).unwrap_or_default();
            let size = query.split('&').find_map(|p| p.strip_prefix("s=")).and_then(|v| v.parse().ok())
                .unwrap_or(crate::icons::ICON);
            match crate::icons::get(&crate::store::client(), &url, size).await {
                Ok(bytes) => respond_cached(&mut stream, "200 OK", crate::icons::kind(&bytes), &bytes,
                                            "private, max-age=604800").await,
                Err(_) => respond(&mut stream, "404 Not Found", "text/plain", b"").await,
            }
        }
        ("GET", "/") => respond(&mut stream, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()).await,
        ("GET", "/logo.svg") => respond(&mut stream, "200 OK", "image/svg+xml", LOGO).await,
        // Another run's check that this store answers (main.rs).
        ("GET", "/api/ping") => {
            let s = json!({"boot": crate::app::boot_id(), "busy": app.busy()}).to_string();
            respond(&mut stream, "200 OK", "application/json", s.as_bytes()).await
        }
        ("GET", "/api/state") => {
            let s = app.snapshot();
            respond(&mut stream, "200 OK", "application/json", s.to_string().as_bytes()).await
        }
        ("GET", "/api/catalog") => {
            let s = app.catalog().to_string();
            respond(&mut stream, "200 OK", "application/json", s.as_bytes()).await
        }
        // A tap or a key on the page (sent at most every 20 s): in use.
        ("POST", "/api/use") => {
            used();
            respond(&mut stream, "200 OK", "application/json", b"{}").await
        }
        ("POST", "/api/action") => {
            let request: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
            let reply = action(&app, &request).await;
            respond(&mut stream, "200 OK", "application/json", reply.to_string().as_bytes()).await
        }
        _ => respond(&mut stream, "404 Not Found", "text/plain", b"").await,
    }
}

/// Add IPA: the page sends the file's bytes as the request body. They go
/// straight to ~/.baristore/ipas/ (never all in memory), 4 MiB at a time,
/// then the app inside is read for the install sheet.
async fn upload(stream: &mut TcpStream, first: &[u8], length: usize) -> Json {
    use std::io::Write;
    const PIECE: usize = 4 << 20;
    let fail = |m: &str| json!({"ok": false, "message": m});
    if length == 0 {
        return fail("The file is empty.");
    }
    let started = std::time::Instant::now();
    let dir = crate::state::path("ipas");
    let _ = crate::rootshell::files::make_dirs(&dir);
    let path = dir.join(format!("added-{}.ipa", hex::encode(rand::random::<[u8; 6]>())));
    let Ok(mut file) = std::fs::File::create(&path) else { return fail("Cannot save the file.") };
    let mut piece = vec![0u8; PIECE];
    let mut filled = first.len().min(length);
    piece[..filled].copy_from_slice(&first[..filled]);
    let mut received = filled;
    let mut saved = true;
    while received < length {
        let want = (PIECE - filled).min(length - received);
        let n = read_some(stream, &mut piece[filled..filled + want], BODY_WAIT).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        filled += n;
        received += n;
        if filled == PIECE {
            saved = rt::block_in_place(|| file.write_all(&piece)).is_ok();
            filled = 0;
            if !saved {
                break;
            }
        }
    }
    if saved && filled > 0 {
        saved = rt::block_in_place(|| file.write_all(&piece[..filled])).is_ok();
    }
    drop(file);
    if !saved {
        let _ = std::fs::remove_file(&path);
        return fail("Cannot save the file (is the device full?).");
    }
    if received < length {
        let _ = std::fs::remove_file(&path);
        return fail("The upload stopped before the end.");
    }
    let secs = started.elapsed().as_secs_f64();
    match rt::block_in_place(|| crate::sign::bundle::peek(&path)) {
        Ok(info) => {
            logf!("upload: {} {} ({} MB in {secs:.1} s)", info.id, info.version, length / 1_000_000);
            json!({"ok": true, "path": path.to_string_lossy(), "name": info.name, "bundle_id": info.id,
                   "version": info.version, "size": length, "icon_url": crate::icons::ipa_icon_url(&path),
                   "info": {"extensions": info.extensions, "removable": info.removable}})
        }
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            fail(&e)
        }
    }
}

/// The port the page is served on (for the addresses of shared files).
static PORT: OnceLock<u16> = OnceLock::new();

/// What a share has sent: the bytes of its latest request, and whether one
/// request got the whole file.
#[derive(Default)]
struct Sent {
    bytes: AtomicU64,
    whole: AtomicBool,
}

/// Shared files by their random id: the file and its size.
type Shares = Mutex<HashMap<String, (std::path::PathBuf, u64, Arc<Sent>)>>;
static SHARES: std::sync::LazyLock<Shares> = std::sync::LazyLock::new(Default::default);

/// A file handed to another app on this device by address (Install to
/// LiveContainer: LiveContainer downloads it like any other IPA). Served at
/// /share/<128-bit random>/<file name> to whoever asks 127.0.0.1 for it,
/// without the page's token, until dropped.
pub struct Share {
    id: String,
    name: String,
    size: u64,
    sent: Arc<Sent>,
}

impl Share {
    pub fn new(path: &std::path::Path) -> Result<Share, String> {
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        let id = hex::encode(rand::random::<[u8; 16]>());
        let sent = Arc::new(Sent::default());
        SHARES.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), (path.to_path_buf(), size, sent.clone()));
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        Ok(Share { id, name, size, sent })
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/share/{}/{}", PORT.get().copied().unwrap_or(0), self.id, self.name)
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn sent(&self) -> u64 {
        self.sent.bytes.load(Ordering::Relaxed)
    }

    pub fn done(&self) -> bool {
        self.sent.whole.load(Ordering::Relaxed)
    }
}

impl Drop for Share {
    fn drop(&mut self) {
        SHARES.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
    }
}

/// Answers a share's address with the file, read a mebibyte at a time.
async fn send_share(stream: &mut TcpStream, rest: &str) -> std::io::Result<()> {
    use std::io::Read;
    let id = rest.split('/').next().unwrap_or("");
    let found = SHARES.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned();
    let Some((path, size, sent)) = found else {
        return respond(stream, "404 Not Found", "text/plain", b"").await;
    };
    let mut file = std::fs::File::open(&path)?;
    let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {size}\r\n\
                        Cache-Control: no-store\r\nConnection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    let mut piece = vec![0u8; 1 << 20];
    let mut done = 0u64;
    sent.bytes.store(0, Ordering::Relaxed);
    while done < size {
        let n = rt::block_in_place(|| file.read(&mut piece))?;
        if n == 0 {
            break;
        }
        stream.write_all(&piece[..n]).await?;
        done += n as u64;
        sent.bytes.store(done, Ordering::Relaxed);
    }
    stream.flush().await?;
    if done == size {
        sent.whole.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// The install sheet's App ID choice: one picked from the list, or a
/// custom one typed (checked, the team appended).
fn reuse(app: &App, r: &Json) -> Result<crate::install::Reuse, String> {
    let custom = s(r, "custom_id");
    let pick = if custom.trim().is_empty() {
        r.get("reuse_id").and_then(Json::as_str).filter(|s| !s.is_empty()).map(str::to_string)
    } else {
        Some(app.custom_app_id(&custom)?)
    };
    Ok(crate::install::Reuse { pick, extensions: r.get("reuse_extensions").and_then(Json::as_bool).unwrap_or(false) })
}

fn options(r: &Json) -> Options {
    Options {
        remove_extensions: r.get("remove_extensions").and_then(Json::as_bool).unwrap_or(false),
        increased_memory: r.get("increased_memory").and_then(Json::as_bool).unwrap_or(false),
        icon: s(r, "icon"),
    }
}

/// Decodes %XX escapes (as encodeURIComponent writes them).
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push(hi << 4 | lo);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A share answers with the whole file while it lives, counts it, and
    /// is gone once dropped (fetched from a thread of the test's own).
    #[test]
    fn shares_a_file_once_asked() {
        let path = crate::state::scratch("share.ipa");
        let data: Vec<u8> = (0..(3 << 20) + 123).map(|i: u32| (i * 7 + i / 991) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let share = Share::new(&path).unwrap();
        assert!(share.url().ends_with(&format!("/share/{}/{}", share.id, share.name)));
        let fetch = |rest: String| rt::block_on(async move {
            let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            let listener = TcpListener::bind(port).unwrap();
            let client = std::thread::spawn(move || {
                use std::io::{Read, Write};
                let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                c.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
                let mut got = Vec::new();
                c.read_to_end(&mut got).unwrap();
                got
            });
            let mut stream = listener.accept().await.unwrap();
            send_share(&mut stream, &rest).await.unwrap();
            drop(stream);
            client.join().unwrap()
        });
        let got = fetch(format!("{}/x.ipa", share.id));
        let end = got.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let head = String::from_utf8_lossy(&got[..end]);
        assert!(head.starts_with("HTTP/1.1 200") && head.contains(&format!("Content-Length: {}", data.len())), "{head}");
        assert!(got[end..] == data[..]);
        assert!(share.done() && share.sent() == data.len() as u64);
        let id = share.id.clone();
        drop(share);
        assert!(fetch(format!("{id}/x.ipa")).starts_with(b"HTTP/1.1 404"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(super::percent_decode("https%3A%2F%2Fa.b%2Fx%20y.png"), "https://a.b/x y.png");
        assert_eq!(super::percent_decode("100%"), "100%");
        assert_eq!(super::percent_decode("%zz%4"), "%zz%4");
    }
}

fn s(v: &Json, k: &str) -> String {
    v.get(k).and_then(Json::as_str).unwrap_or("").to_string()
}

async fn action(app: &App, r: &Json) -> Json {
    let name = s(r, "action");
    let ok = |msg: &str| json!({"ok": true, "message": msg});
    let fail = |msg: String| json!({"ok": false, "message": msg});
    if name != "answer" {
        logf!("page: {name}");
    }
    match name.as_str() {
        "check" => {
            if let Err(e) = app.check_route().await {
                return fail(e);
            }
            if crate::pairing::flow::load_record().is_some() {
                if let Err(e) = app.device().await {
                    return fail(e);
                }
                // What is on the device (whether LiveContainer is, for one).
                app.load_device_apps().await.ok();
            }
            ok("connected")
        }
        "pair" => {
            app.spawn("Pairing", Some(json!({"action": "pair"})), |a| async move { a.pair().await });
            ok("started")
        }
        "login" => {
            let (email, password) = (s(r, "email"), s(r, "password"));
            if email.is_empty() || password.is_empty() {
                return fail("Enter your Apple ID email and password".into());
            }
            app.spawn("Signing in", None, move |a| async move { a.login(email, password).await });
            ok("started")
        }
        "sign_out" => { app.sign_out().await; ok("signed out") }
        "answer" => {
            let id = r.get("id").and_then(Json::as_u64).unwrap_or(0);
            let value = r.get("value").and_then(Json::as_str).map(str::to_string);
            app.answer(id, value);
            ok("")
        }
        "dismiss" => { app.dismiss_task(); ok("") }
        "catalog_refresh" => { app.refresh_catalog(); ok("") }
        "source_add" => match app.add_source(&s(r, "url")) { Ok(()) => ok("added"), Err(e) => fail(e) },
        "source_remove" => { app.remove_source(&s(r, "url")); ok("removed") }
        "install" => {
            let key = s(r, "key");
            let version = r.get("version").and_then(Json::as_u64).unwrap_or(0) as usize;
            let reuse = match reuse(app, r) { Ok(reuse) => reuse, Err(e) => return fail(e) };
            let opts = options(r);
            let title = s(r, "name");
            app.spawn(&format!("Installing {title}"), Some(r.clone()),
                           move |a| async move { a.install_version(&key, version, opts, reuse).await });
            ok("started")
        }
        "install_local" => {
            let path = s(r, "path");
            let reuse = match reuse(app, r) { Ok(reuse) => reuse, Err(e) => return fail(e) };
            let opts = options(r);
            app.spawn(&format!("Installing {}", s(r, "name")), Some(r.clone()),
                           move |a| async move { a.install_local(&path, opts, reuse).await });
            ok("started")
        }
        "lc_install" => {
            let key = s(r, "key");
            let version = r.get("version").and_then(Json::as_u64).unwrap_or(0) as usize;
            app.spawn(&format!("{} to LiveContainer", s(r, "name")), Some(r.clone()),
                      move |a| async move { a.lc_install(&key, version).await });
            ok("started")
        }
        "lc_install_local" => {
            let path = s(r, "path");
            app.spawn(&format!("{} to LiveContainer", s(r, "name")), Some(r.clone()),
                           move |a| async move { a.lc_install_local(&path).await });
            ok("started")
        }
        "app_options" => {
            let (id, name) = (s(r, "bundle_id"), s(r, "name"));
            let memory = r.get("increased_memory").and_then(Json::as_bool).unwrap_or(false);
            let icon = r.get("icon").and_then(Json::as_str);
            match app.set_app_options(&id, memory, icon) {
                Ok(true) => {
                    app.spawn(&format!("Signing {name} again"), Some(json!({"action": "sign_again", "bundle_id": id,
                        "name": name})), move |a| async move { a.sign_again(&id).await });
                    ok("started")
                }
                Ok(false) => ok(&format!("Saved: {name} gets it with its next update. BariStore no longer has its IPA \
                                          and downloads nothing for this.")),
                Err(e) => fail(e),
            }
        }
        "sign_again" => {
            let id = s(r, "bundle_id");
            app.spawn(&format!("Signing {} again", s(r, "name")), Some(r.clone()),
                           move |a| async move { a.sign_again(&id).await });
            ok("started")
        }
        "clear_cache" => ok(&app.clear_cache()),
        "clear_ipas" => ok(&app.clear_ipas()),
        "cache_size" => {
            let (cache, ipas) = app.cache_sizes();
            json!({"ok": true, "cache": cache, "ipas": ipas})
        }
        "refresh" => {
            let id = s(r, "bundle_id");
            app.spawn(&format!("Refreshing {}", s(r, "name")), Some(r.clone()), move |a| async move { a.refresh_app(&id).await });
            ok("started")
        }
        "refresh_all" => { app.spawn("Refreshing all apps", Some(r.clone()), |a| async move { a.refresh_all().await }); ok("started") }
        "delete" => {
            let id = s(r, "bundle_id");
            app.spawn(&format!("Deleting {}", s(r, "name")), Some(r.clone()), move |a| async move { a.delete_app(&id).await });
            ok("started")
        }
        "jit" => {
            let id = s(r, "bundle_id");
            app.spawn(&format!("JIT for {}", s(r, "name")), Some(r.clone()), move |a| async move { a.enable_jit(&id).await });
            ok("started")
        }
        "app_ids" => match app.load_app_ids().await { Ok(()) => ok(""), Err(e) => fail(e) },
        "check_self_update" => match app.check_self_update().await { Ok(()) => ok(""), Err(e) => fail(e) },
        "check_updates" => match app.check_updates().await { Ok(()) => ok(""), Err(e) => fail(e) },
        "ipa_info" => {
            let version = r.get("version").and_then(Json::as_u64).unwrap_or(0) as usize;
            match app.ipa_info(&s(r, "key"), version).await { Ok(info) => json!({"ok": true, "info": info}), Err(e) => fail(e) }
        }
        "set_channel" => match app.set_channel(&s(r, "bundle_id"), &s(r, "channel")) { Ok(m) => ok(&m), Err(e) => fail(e) },
        "anisette" => match app.set_anisette(&s(r, "url")).await { Ok(m) => ok(&m), Err(e) => fail(e) },
        "anisette_add" => match app.add_anisette(&s(r, "url")).await { Ok(m) => ok(&m), Err(e) => fail(e) },
        "anisette_remove" => { app.remove_anisette(&s(r, "url")); ok("removed") }
        "update_app" => {
            let id = s(r, "bundle_id");
            app.spawn(&format!("Updating {}", s(r, "name")), Some(r.clone()), move |a| async move { a.update_app(&id).await });
            ok("started")
        }
        "update_all" => { app.spawn("Updating apps", Some(r.clone()), |a| async move { a.update_all().await }); ok("started") }
        "update" => { app.spawn("Updating BariStore", Some(r.clone()), |a| async move { a.update().await }); ok("started") }
        "place_pairing" => {
            let id = s(r, "bundle_id");
            app.spawn("Pairing file", Some(r.clone()), move |a| async move { a.place_pairing(&id).await });
            ok("started")
        }
        "export_pairing" => match app.export_pairing() { Ok(file) => json!({"ok": true, "file": file}), Err(e) => fail(e) },
        "certificates" => match app.certificates().await { Ok(list) => json!({"ok": true, "list": list}), Err(e) => fail(e) },
        "revoke_certificate" => {
            let serial = s(r, "serial");
            app.spawn("Revoking a certificate", Some(r.clone()), move |a| async move { a.revoke_certificate(&serial).await });
            ok("started")
        }
        "export_certificate" => {
            app.spawn("Exporting the certificate", Some(r.clone()), |a| async move {
                let file = a.export_certificate().await?;
                a.offer_next(json!({"label": "Share the file", "share": file}));
                Ok("BariStore-Certificate.p12 is ready. BariStore on your other device (Certificates › Import) and \
                    other apps that import it ask for this password:".into())
            });
            ok("started")
        }
        "import_certificate" => {
            let (p12, password) = (s(r, "base64"), s(r, "password"));
            app.spawn("Importing a certificate", None, move |a| async move { a.import_certificate(&p12, &password).await });
            ok("started")
        }
        "lc_certificate" => {
            app.spawn("Certificate for LiveContainer", Some(r.clone()), |a| async move { a.lc_certificate().await });
            ok("started")
        }
        "settings" => { crate::state::save_settings(r.get("settings").unwrap_or(&Json::Null)); ok("saved") }
        "log" => ok(&crate::doctor::report(false).await),
        "checks" => ok(&crate::doctor::report(true).await),
        "reset" => { app.reset().await; ok("reset") }
        "quit" => {
            rt::spawn(async {
                rt::time::sleep(Duration::from_millis(300)).await;
                logf!("quit from the page");
                crate::state::end(0)
            });
            ok("bye")
        }
        _ => fail(format!("unknown action {name}")),
    }
}
