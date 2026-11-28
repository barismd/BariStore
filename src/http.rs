//! HTTP/1.1 over rootshell's TCP and TLS, in place of reqwest (which needs
//! threads, and a TLS library rootshell already is). What BariStore asks of
//! it: GET, HEAD and POST with headers and a body, redirects followed,
//! gzip and deflate answers unpacked, a body read whole or a piece at a
//! time, a time limit, and a connection kept for the next request to the
//! same host (GrandSlam's client keeps none). Plus the WebSocket anisette
//! provisioning talks over (`websocket`).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::net::TcpStream;
use crate::rt::time;

const MAX_REDIRECTS: usize = 10;
const MAX_HEAD: usize = 64 * 1024;
/// An idle kept connection older than this is dropped (servers close
/// theirs after a minute or so).
const IDLE: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Timeout,
    Connect,
    Io,
    Status(u16),
    Url,
    Body,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub kind: ErrorKind,
    message: String,
}

impl Error {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error { kind, message: message.into() }
    }

    pub fn is_timeout(&self) -> bool {
        self.kind == ErrorKind::Timeout
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// A status, as the answer's first line gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusCode {
    code: u16,
    reason: String,
}

impl StatusCode {
    pub const PARTIAL_CONTENT: u16 = 206;
    pub const PRECONDITION_FAILED: u16 = 412;
    pub const TOO_MANY_REQUESTS: u16 = 429;

    pub fn as_u16(&self) -> u16 {
        self.code
    }
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.code)
    }
    pub fn is_server_error(&self) -> bool {
        (500..600).contains(&self.code)
    }
}

impl PartialEq<u16> for StatusCode {
    fn eq(&self, other: &u16) -> bool {
        self.code == *other
    }
}

impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.reason.is_empty() { write!(f, "{}", self.code) } else { write!(f, "{} {}", self.code, self.reason) }
    }
}

/// Header fields in order; names compare without case.
#[derive(Debug, Clone, Default)]
pub struct HeaderMap {
    fields: Vec<(String, String)>,
}

impl HeaderMap {
    pub fn new() -> HeaderMap {
        HeaderMap::default()
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Sets `name`, replacing what it had (line breaks cannot get in).
    pub fn insert(&mut self, name: &str, value: impl Into<String>) {
        let value: String = value.into().chars().filter(|c| *c != '\r' && *c != '\n').collect();
        self.fields.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        self.fields.push((name.to_string(), value));
    }

    pub fn extend(&mut self, other: HeaderMap) {
        for (k, v) in other.fields {
            self.insert(&k, v);
        }
    }

    fn remove(&mut self, name: &str) {
        self.fields.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    }

    fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }
}

/// Where a request goes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Url {
    tls: bool,
    host: String,
    port: u16,
    path: String,
}

impl Url {
    fn parse(url: &str) -> Result<Url> {
        let bad = || Error::new(ErrorKind::Url, format!("not a web address: {url}"));
        let (tls, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("http://") {
            (false, r)
        } else if let Some(r) = url.strip_prefix("wss://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("ws://") {
            (false, r)
        } else {
            return Err(bad());
        };
        let (authority, path) = match rest.find(['/', '?']) {
            Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], rest[i..].to_string()),
            Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
            None => (rest, "/".to_string()),
        };
        let authority = authority.rsplit('@').next().unwrap_or(authority);
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !h.contains(']') || h.ends_with(']') => (h, p.parse().map_err(|_| bad())?),
            _ => (authority, if tls { 443 } else { 80 }),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
        if host.is_empty() {
            return Err(bad());
        }
        Ok(Url { tls, host, port, path: path.split('#').next().unwrap_or("/").to_string() })
    }

    /// The Host field: the port only when it is not the scheme's.
    fn authority(&self) -> String {
        if self.port == if self.tls { 443 } else { 80 } { self.host.clone() } else { format!("{}:{}", self.host, self.port) }
    }

    fn origin(&self) -> String {
        format!("{}://{}", if self.tls { "https" } else { "http" }, self.authority())
    }

    /// `location` read against this address.
    fn join(&self, location: &str) -> Result<Url> {
        if location.starts_with("http://") || location.starts_with("https://") {
            Url::parse(location)
        } else if let Some(rest) = location.strip_prefix("//") {
            Url::parse(&format!("{}://{rest}", if self.tls { "https" } else { "http" }))
        } else if location.starts_with('/') {
            Url::parse(&format!("{}{location}", self.origin()))
        } else {
            let dir = self.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            Url::parse(&format!("{}{dir}/{location}", self.origin()))
        }
    }
}

struct Kept {
    key: (bool, String, u16),
    stream: TcpStream,
    since: Instant,
}

struct Inner {
    user_agent: Option<String>,
    timeout: Duration,
    keep: bool,
    idle: Mutex<Vec<Kept>>,
}

#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

pub struct ClientBuilder {
    user_agent: Option<String>,
    timeout: Duration,
    keep: bool,
}

impl ClientBuilder {
    pub fn user_agent(mut self, ua: &str) -> Self {
        self.user_agent = Some(ua.into());
        self
    }
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }
    /// A new connection for every request.
    pub fn fresh_connections(mut self) -> Self {
        self.keep = false;
        self
    }
    pub fn build(self) -> Client {
        Client { inner: Arc::new(Inner { user_agent: self.user_agent, timeout: self.timeout, keep: self.keep,
                                          idle: Mutex::default() }) }
    }
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder { user_agent: None, timeout: Duration::from_secs(30), keep: true }
    }

    fn request(&self, method: &str, url: impl AsRef<str>) -> RequestBuilder {
        RequestBuilder { client: self.clone(), method: method.into(), url: url.as_ref().to_string(),
                         headers: HeaderMap::new(), body: Vec::new(), timeout: None }
    }

    pub fn get(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request("GET", url)
    }
    pub fn post(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request("POST", url)
    }
    pub fn head(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request("HEAD", url)
    }
    pub fn patch(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request("PATCH", url)
    }
    pub fn put(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request("PUT", url)
    }

    fn take_kept(&self, url: &Url) -> Option<TcpStream> {
        let key = (url.tls, url.host.clone(), url.port);
        let mut idle = self.inner.idle.lock().unwrap_or_else(|e| e.into_inner());
        idle.retain(|k| k.since.elapsed() < IDLE);
        let at = idle.iter().rposition(|k| k.key == key)?;
        Some(idle.remove(at).stream)
    }

    fn keep(&self, url: &Url, stream: TcpStream) {
        let mut idle = self.inner.idle.lock().unwrap_or_else(|e| e.into_inner());
        let key = (url.tls, url.host.clone(), url.port);
        if idle.iter().filter(|k| k.key == key).count() < 4 {
            idle.push(Kept { key, stream, since: Instant::now() });
        }
    }
}

#[derive(Clone)]
pub struct RequestBuilder {
    client: Client,
    method: String,
    url: String,
    headers: HeaderMap,
    body: Vec<u8>,
    timeout: Option<Duration>,
}

impl RequestBuilder {
    /// The same request again (always possible: bodies are bytes).
    pub fn try_clone(&self) -> Option<RequestBuilder> {
        Some(self.clone())
    }
    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.insert(name, value);
        self
    }
    pub fn headers(mut self, headers: HeaderMap) -> Self {
        self.headers.extend(headers);
        self
    }
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }
    pub fn json<T: serde::Serialize + ?Sized>(mut self, value: &T) -> Self {
        self.body = serde_json::to_vec(value).unwrap_or_default();
        if !self.headers.has("Content-Type") {
            self.headers.insert("Content-Type", "application/json");
        }
        self
    }
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }

    pub async fn send(self) -> Result<Response> {
        let limit = self.timeout.unwrap_or(self.client.inner.timeout);
        let deadline = Instant::now() + limit;
        let url = self.url.clone();
        match time::timeout(limit, self.follow(deadline)).await {
            Ok(r) => r,
            Err(_) => Err(Error::new(ErrorKind::Timeout, format!("{}: no answer within {} s", short(&url), limit.as_secs()))),
        }
    }

    async fn follow(self, deadline: Instant) -> Result<Response> {
        let RequestBuilder { client, mut method, url, mut headers, mut body, .. } = self;
        let mut url = Url::parse(&url)?;
        for _ in 0..=MAX_REDIRECTS {
            let mut r = exchange(&client, &method, &url, &headers, &body, deadline).await?;
            let status = r.status.code;
            let location = r.headers.get("Location").map(str::to_string);
            let Some(location) = location.filter(|_| matches!(status, 301 | 302 | 303 | 307 | 308)) else {
                return Ok(r);
            };
            r.drain().await;
            let next = url.join(&location)?;
            if status == 303 || (matches!(status, 301 | 302) && method == "POST") {
                method = "GET".into();
                body.clear();
                headers.remove("Content-Type");
            }
            if next.host != url.host {
                headers.remove("Authorization");
                headers.remove("Cookie");
            }
            url = next;
        }
        Err(Error::new(ErrorKind::Io, format!("{}: too many redirects", short(&url.origin()))))
    }
}

/// An address cut for messages.
fn short(url: &str) -> String {
    let u = url.split('?').next().unwrap_or(url);
    if u.len() > 90 { format!("{}…", &u[..90]) } else { u.to_string() }
}

/// One request on a kept connection (asked again on a new one when the
/// kept one turns out closed) or a new one; the answer's head.
async fn exchange(client: &Client, method: &str, url: &Url, headers: &HeaderMap, body: &[u8], deadline: Instant)
    -> Result<Response>
{
    let mut head = format!("{method} {} HTTP/1.1\r\nHost: {}\r\n", url.path, url.authority());
    if !headers.has("User-Agent") {
        if let Some(ua) = &client.inner.user_agent {
            head.push_str(&format!("User-Agent: {ua}\r\n"));
        }
    }
    if !headers.has("Accept") {
        head.push_str("Accept: */*\r\n");
    }
    if !headers.has("Accept-Encoding") {
        head.push_str("Accept-Encoding: gzip, deflate\r\n");
    }
    for (k, v) in &headers.fields {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() || matches!(method, "POST" | "PUT" | "PATCH") {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str(if client.inner.keep { "Connection: keep-alive\r\n\r\n" } else { "Connection: close\r\n\r\n" });
    let mut request = head.into_bytes();
    request.extend_from_slice(body);

    if client.inner.keep {
        if let Some(stream) = client.take_kept(url) {
            match on(stream, &request, client, url, method, deadline).await {
                Ok(r) => return Ok(r),
                Err((false, _)) => {} // nothing came back: a closed kept connection
                Err((true, e)) => return Err(e),
            }
        }
    }
    let stream = connect(url).await?;
    on(stream, &request, client, url, method, deadline).await.map_err(|(_, e)| e)
}

async fn connect(url: &Url) -> Result<TcpStream> {
    let connected = if url.tls {
        TcpStream::connect_tls(&url.host, url.port).await
    } else {
        TcpStream::connect(&url.host, url.port).await
    };
    connected.map_err(|e| Error::new(ErrorKind::Connect, format!("{}: cannot connect: {e}", url.origin())))
}

/// Sends `request` on `stream` and reads the head. The error says whether
/// any answer came (false: the connection was closed before it).
async fn on(mut stream: TcpStream, request: &[u8], client: &Client, url: &Url, method: &str, deadline: Instant)
    -> std::result::Result<Response, (bool, Error)>
{
    let io = |answered: bool, e: std::io::Error| (answered, Error::new(ErrorKind::Io, format!("{}: {e}", url.origin())));
    stream.write_all(request).await.map_err(|e| io(false, e))?;
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    let end = loop {
        let n = stream.read(&mut chunk).await.map_err(|e| io(!buf.is_empty(), e))?;
        if n == 0 {
            let why = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the connection closed without an answer");
            return Err(io(!buf.is_empty(), why));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > MAX_HEAD {
            return Err((true, Error::new(ErrorKind::Io, format!("{}: answer head too long", url.origin()))));
        }
    };
    let text = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    let code: u16 = parts.next().and_then(|c| c.parse().ok())
        .ok_or_else(|| (true, Error::new(ErrorKind::Io, format!("{}: not an HTTP answer: {first}", url.origin()))))?;
    let status = StatusCode { code, reason: parts.next().unwrap_or("").to_string() };
    let mut headers = HeaderMap::new();
    for line in lines.filter(|l| !l.is_empty()) {
        if let Some((k, v)) = line.split_once(':') {
            headers.fields.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let close = version == "HTTP/1.0" || headers.get("Connection").is_some_and(|c| c.eq_ignore_ascii_case("close"));
    let framing = if method == "HEAD" || code == 204 || code == 304 || (100..200).contains(&code) {
        Framing::Length(0)
    } else if headers.get("Transfer-Encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked")) {
        Framing::Chunked(ChunkState::Size)
    } else if let Some(n) = headers.get("Content-Length").and_then(|l| l.parse().ok()) {
        Framing::Length(n)
    } else {
        Framing::Close
    };
    let encoding = headers.get("Content-Encoding").map(str::to_ascii_lowercase).unwrap_or_default();
    let decoder = match encoding.as_str() {
        "gzip" | "x-gzip" => Some(Decoder::Gzip(flate2::write::GzDecoder::new(Vec::new()))),
        "deflate" => Some(Decoder::Deflate(flate2::write::ZlibDecoder::new(Vec::new()))),
        _ => None,
    };
    let keep = client.inner.keep && !close && !matches!(framing, Framing::Close);
    let body = Body { stream: Some(stream), pending: buf[end..].to_vec(), framing, decoder, keep,
                      client: client.clone(), url: url.clone(), deadline, done: false };
    Ok(Response { status, headers, body, url: url.origin() + &url.path })
}

enum Framing {
    Length(u64),
    Chunked(ChunkState),
    Close,
}

#[derive(Clone, Copy)]
enum ChunkState {
    Size,
    Data(u64),
    /// The CRLF after a chunk's data.
    DataEnd,
    Trailer,
}

enum Decoder {
    Gzip(flate2::write::GzDecoder<Vec<u8>>),
    Deflate(flate2::write::ZlibDecoder<Vec<u8>>),
}

impl Decoder {
    fn feed(&mut self, data: &[u8]) -> std::io::Result<Vec<u8>> {
        use std::io::Write;
        match self {
            Decoder::Gzip(d) => {
                d.write_all(data)?;
                Ok(std::mem::take(d.get_mut()))
            }
            Decoder::Deflate(d) => {
                d.write_all(data)?;
                Ok(std::mem::take(d.get_mut()))
            }
        }
    }

    fn finish(self) -> std::io::Result<Vec<u8>> {
        match self {
            Decoder::Gzip(d) => d.finish(),
            Decoder::Deflate(d) => d.finish(),
        }
    }
}

struct Body {
    stream: Option<TcpStream>,
    /// Bytes read with the head, or past the last chunk size.
    pending: Vec<u8>,
    framing: Framing,
    decoder: Option<Decoder>,
    keep: bool,
    client: Client,
    url: Url,
    deadline: Instant,
    done: bool,
}

impl Body {
    async fn more(&mut self) -> Result<usize> {
        let Some(stream) = self.stream.as_mut() else { return Ok(0) };
        let mut chunk = vec![0u8; 32 * 1024];
        let left = self.deadline.saturating_duration_since(Instant::now());
        let n = match time::timeout(left, stream.read(&mut chunk)).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(Error::new(ErrorKind::Body, format!("{}: the download broke: {e}", self.url.origin()))),
            Err(_) => return Err(Error::new(ErrorKind::Timeout, format!("{}: the answer stopped coming", self.url.origin()))),
        };
        self.pending.extend_from_slice(&chunk[..n]);
        Ok(n)
    }

    /// The next raw piece of the body (None at its end).
    async fn raw(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            match self.framing {
                Framing::Length(0) => return Ok(None),
                Framing::Length(left) => {
                    if self.pending.is_empty() && self.more().await? == 0 {
                        return Err(Error::new(ErrorKind::Body, format!("{}: the answer ended early", self.url.origin())));
                    }
                    let n = (left as usize).min(self.pending.len());
                    let piece: Vec<u8> = self.pending.drain(..n).collect();
                    self.framing = Framing::Length(left - n as u64);
                    return Ok(Some(piece));
                }
                Framing::Close => {
                    if self.pending.is_empty() && self.more().await? == 0 {
                        return Ok(None);
                    }
                    return Ok(Some(std::mem::take(&mut self.pending)));
                }
                Framing::Chunked(state) => match state {
                    ChunkState::Size | ChunkState::DataEnd | ChunkState::Trailer => {
                        let Some(at) = self.pending.windows(2).position(|w| w == b"\r\n") else {
                            if self.more().await? == 0 {
                                return Err(Error::new(ErrorKind::Body, format!("{}: the answer ended early", self.url.origin())));
                            }
                            continue;
                        };
                        let line: Vec<u8> = self.pending.drain(..at + 2).collect();
                        let line = String::from_utf8_lossy(&line[..at]).to_string();
                        self.framing = Framing::Chunked(match state {
                            ChunkState::Size => {
                                let size = u64::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16)
                                    .map_err(|_| Error::new(ErrorKind::Body, "bad chunk size"))?;
                                if size == 0 { ChunkState::Trailer } else { ChunkState::Data(size) }
                            }
                            ChunkState::DataEnd => ChunkState::Size,
                            _ if line.is_empty() => {
                                self.framing = Framing::Length(0);
                                return Ok(None);
                            }
                            _ => ChunkState::Trailer,
                        });
                    }
                    ChunkState::Data(left) => {
                        if self.pending.is_empty() && self.more().await? == 0 {
                            return Err(Error::new(ErrorKind::Body, format!("{}: the answer ended early", self.url.origin())));
                        }
                        let n = (left as usize).min(self.pending.len());
                        let piece: Vec<u8> = self.pending.drain(..n).collect();
                        self.framing = Framing::Chunked(if left == n as u64 { ChunkState::DataEnd } else { ChunkState::Data(left - n as u64) });
                        return Ok(Some(piece));
                    }
                },
            }
        }
    }

    /// The next piece of the body as the server meant it (unpacked).
    async fn chunk(&mut self) -> Result<Option<Vec<u8>>> {
        if self.done {
            return Ok(None);
        }
        loop {
            match self.raw().await? {
                Some(piece) => match self.decoder.as_mut() {
                    Some(d) => {
                        let out = d.feed(&piece).map_err(|e| Error::new(ErrorKind::Body, format!("bad gzip answer: {e}")))?;
                        if !out.is_empty() {
                            return Ok(Some(out));
                        }
                    }
                    None if piece.is_empty() => {}
                    None => return Ok(Some(piece)),
                },
                None => {
                    self.done = true;
                    if self.keep && self.pending.is_empty() {
                        if let Some(stream) = self.stream.take() {
                            self.client.keep(&self.url, stream);
                        }
                    }
                    self.stream = None;
                    let rest = match self.decoder.take() {
                        Some(d) => d.finish().map_err(|e| Error::new(ErrorKind::Body, format!("bad gzip answer: {e}")))?,
                        None => Vec::new(),
                    };
                    return Ok((!rest.is_empty()).then_some(rest));
                }
            }
        }
    }
}

pub struct Response {
    status: StatusCode,
    headers: HeaderMap,
    body: Body,
    url: String,
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} from {}", self.status, self.url)
    }
}

impl Response {
    pub fn status(&self) -> StatusCode {
        self.status.clone()
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// The body's length as sent (None when packed or not said).
    pub fn content_length(&self) -> Option<u64> {
        match self.body.framing {
            Framing::Length(n) if self.body.decoder.is_none() => Some(n),
            _ => None,
        }
    }

    /// A 4xx or 5xx as an error.
    pub fn error_for_status(self) -> Result<Response> {
        if self.status.code >= 400 {
            return Err(Error::new(ErrorKind::Status(self.status.code), format!("HTTP {} for {}", self.status, short(&self.url))));
        }
        Ok(self)
    }

    pub async fn chunk(&mut self) -> Result<Option<Vec<u8>>> {
        self.body.chunk().await
    }

    pub async fn bytes(mut self) -> Result<Vec<u8>> {
        let mut all = Vec::new();
        while let Some(piece) = self.body.chunk().await? {
            all.extend_from_slice(&piece);
        }
        Ok(all)
    }

    pub async fn text(self) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.bytes().await?).into_owned())
    }

    pub async fn json<T: serde::de::DeserializeOwned>(self) -> Result<T> {
        let url = self.url.clone();
        let bytes = self.bytes().await?;
        serde_json::from_slice(&bytes).map_err(|e| Error::new(ErrorKind::Body, format!("{}: not JSON: {e}", short(&url))))
    }

    /// Reads a redirect's body so its connection can be kept.
    async fn drain(&mut self) {
        while let Ok(Some(_)) = self.body.chunk().await {}
    }
}

/// A WebSocket's text messages (RFC 6455, client side).
pub struct WebSocket {
    stream: TcpStream,
    pending: Vec<u8>,
}

/// Opens a WebSocket (`wss://` through rootshell's TLS, `ws://` plain).
pub async fn websocket(url: &str) -> Result<WebSocket> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use sha1::{Digest, Sha1};
    let u = Url::parse(url)?;
    let mut stream = connect(&u).await?;
    let key = B64.encode(rand::random::<[u8; 16]>());
    let request = format!("GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                           Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n", u.path, u.authority());
    let fail = |m: String| Error::new(ErrorKind::Io, format!("{}: {m}", u.origin()));
    stream.write_all(request.as_bytes()).await.map_err(|e| fail(e.to_string()))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        let n = stream.read(&mut chunk).await.map_err(|e| fail(e.to_string()))?;
        if n == 0 {
            return Err(fail("the WebSocket closed during its handshake".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > MAX_HEAD {
            return Err(fail("WebSocket handshake too long".into()));
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    if !head.starts_with("HTTP/1.1 101") {
        return Err(fail(format!("no WebSocket: {}", head.lines().next().unwrap_or(""))));
    }
    let want = B64.encode(Sha1::digest(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11")));
    let accepted = head.lines().filter_map(|l| l.split_once(':'))
        .any(|(k, v)| k.trim().eq_ignore_ascii_case("Sec-WebSocket-Accept") && v.trim() == want);
    if !accepted {
        return Err(fail("the WebSocket answer has a wrong accept key".into()));
    }
    Ok(WebSocket { stream, pending: buf[end..].to_vec() })
}

impl WebSocket {
    async fn frame(&mut self, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
        let mut f = vec![0x80 | opcode];
        match payload.len() {
            n if n < 126 => f.push(0x80 | n as u8),
            n if n < 65536 => {
                f.push(0x80 | 126);
                f.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                f.push(0x80 | 127);
                f.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let mask: [u8; 4] = rand::random();
        f.extend_from_slice(&mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.stream.write_all(&f).await
    }

    pub async fn send_text(&mut self, text: &str) -> std::io::Result<()> {
        self.frame(0x1, text.as_bytes()).await
    }

    pub async fn close(&mut self) {
        let _ = self.frame(0x8, &[]).await;
    }

    async fn need(&mut self, n: usize) -> std::io::Result<()> {
        let mut chunk = [0u8; 16 * 1024];
        while self.pending.len() < n {
            let got = self.stream.read(&mut chunk).await?;
            if got == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the WebSocket closed"));
            }
            self.pending.extend_from_slice(&chunk[..got]);
        }
        Ok(())
    }

    /// The next text message (pings are answered; None once it closed).
    pub async fn next_text(&mut self) -> std::io::Result<Option<String>> {
        let mut message = Vec::new();
        loop {
            self.need(2).await?;
            let (b0, b1) = (self.pending[0], self.pending[1]);
            let (fin, opcode, masked) = (b0 & 0x80 != 0, b0 & 0x0f, b1 & 0x80 != 0);
            let (mut len, mut at) = ((b1 & 0x7f) as u64, 2usize);
            if len == 126 {
                self.need(4).await?;
                len = u16::from_be_bytes([self.pending[2], self.pending[3]]) as u64;
                at = 4;
            } else if len == 127 {
                self.need(10).await?;
                len = u64::from_be_bytes(self.pending[2..10].try_into().unwrap());
                at = 10;
            }
            let mask_at = at;
            if masked {
                at += 4;
            }
            self.need(at + len as usize).await?;
            let mut payload: Vec<u8> = self.pending[at..at + len as usize].to_vec();
            if masked {
                let mask: [u8; 4] = self.pending[mask_at..mask_at + 4].try_into().unwrap();
                payload.iter_mut().enumerate().for_each(|(i, b)| *b ^= mask[i % 4]);
            }
            self.pending.drain(..at + len as usize);
            match opcode {
                0x8 => return Ok(None),
                0x9 => self.frame(0xA, &payload).await?,
                0xA => {}
                _ => {
                    message.extend_from_slice(&payload);
                    if fin {
                        return Ok(Some(String::from_utf8_lossy(&message).into_owned()));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt;

    #[test]
    fn urls() {
        let u = Url::parse("https://example.com/a/b?c=d#e").unwrap();
        assert_eq!((u.tls, u.host.as_str(), u.port, u.path.as_str()), (true, "example.com", 443, "/a/b?c=d"));
        assert_eq!(Url::parse("http://127.0.0.1:8080").unwrap().path, "/");
        assert_eq!(Url::parse("http://h:81?x=1").unwrap().path, "/?x=1");
        assert_eq!(u.join("/z").unwrap().path, "/z");
        assert_eq!(u.join("c").unwrap().path, "/a/c");
        assert_eq!(u.join("https://other.org/q").unwrap().host, "other.org");
        assert_eq!(Url::parse("wss://ani.example/v3/x").unwrap().port, 443);
        assert!(Url::parse("ftp://x").is_err());
    }

    /// One server answering in turn: a redirect, a chunked gzip answer, and
    /// a plain one on the same kept connection.
    #[test]
    fn redirects_chunks_gzip_and_kept_connections() {
        use std::io::{BufRead, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut connections = 0;
            for stream in listener.incoming() {
                connections += 1;
                let mut stream = stream.unwrap();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let path = line.split(' ').nth(1).unwrap_or("").to_string();
                    let mut length = 0;
                    loop {
                        let mut h = String::new();
                        reader.read_line(&mut h).unwrap();
                        if h == "\r\n" {
                            break;
                        }
                        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                            length = v.trim().parse().unwrap();
                        }
                    }
                    let mut body = vec![0u8; length];
                    reader.read_exact(&mut body).unwrap();
                    match path.as_str() {
                        "/old" => stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /new\r\nContent-Length: 0\r\n\r\n").unwrap(),
                        "/new" => {
                            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                            gz.write_all(b"hello, unpacked").unwrap();
                            let packed = gz.finish().unwrap();
                            let (a, b) = packed.split_at(packed.len() / 2);
                            let mut out = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Encoding: gzip\r\n\r\n".to_vec();
                            for part in [a, b] {
                                out.extend(format!("{:x}\r\n", part.len()).as_bytes());
                                out.extend(part);
                                out.extend(b"\r\n");
                            }
                            out.extend(b"0\r\n\r\n");
                            stream.write_all(&out).unwrap();
                        }
                        _ => {
                            let reply = format!("{} {} {}", String::from_utf8_lossy(&body), connections, path);
                            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{reply}", reply.len()).as_bytes()).unwrap();
                        }
                    }
                }
                if connections == 2 {
                    break;
                }
            }
        });
        rt::block_on(async move {
            let http = Client::builder().build();
            let r = http.get(format!("http://127.0.0.1:{port}/old")).send().await.unwrap();
            assert_eq!(r.status().as_u16(), 200);
            assert_eq!(r.text().await.unwrap(), "hello, unpacked");
            let r = http.post(format!("http://127.0.0.1:{port}/echo")).body("posted").send().await.unwrap();
            // Still the first connection: kept after each answer.
            assert_eq!(r.text().await.unwrap(), "posted 1 /echo");
            // The kept connection closes with its client.
            drop(http);
            let fresh = Client::builder().fresh_connections().build();
            let r = fresh.get(format!("http://127.0.0.1:{port}/x")).send().await.unwrap();
            assert_eq!(r.text().await.unwrap(), " 2 /x");
        });
        server.join().unwrap();
    }
}
