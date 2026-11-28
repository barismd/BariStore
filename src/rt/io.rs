//! Waiting for sockets in rootshell, where every call blocks and there is
//! no poll.
//!
//! * A socket a task wants to read (`Sock`) is read by `turn` with a receive
//!   time limit: alone for as long as nothing else is due, a few milliseconds
//!   each in turn when several wait. What comes is kept in the socket's
//!   buffer, so nothing is lost when the reading future gives up.
//! * A listening socket (`Door`) is only `accept`ed once a connection is
//!   known to be waiting, as rootshell's accept waits until one comes:
//!   `turn` knocks, connecting to the door itself and sending a word only
//!   this run knows. Accepting then returns at once, with connections that
//!   came before the knock and, last, the knock itself (closed). A knock that
//!   cannot connect means iOS took the listener (an app suspended in the
//!   background loses its sockets): it is opened again on its port.
//!   (atelier-install found this way, `door.rs`; written anew here.)

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crate::rootshell::{self as rs, errno, Errno};

/// The longest single wait, so knocks and timers stay on time.
const SLICE: Duration = Duration::from_millis(400);
/// How long each of several waiting sockets is given in a turn.
const SHARED_WAIT_MS: u32 = 4;
/// How long a new connection may take to say its first bytes.
const FIRST_BYTES_MS: u32 = 300;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
struct Incoming {
    buf: VecDeque<u8>,
    eof: bool,
    error: Option<Errno>,
    waker: Option<Waker>,
}

/// A connected TCP socket in rootshell, closed when the last handle goes.
pub struct Sock {
    pub fd: i32,
    incoming: Mutex<Incoming>,
    wanted: AtomicBool,
    /// The receive time limit rootshell has for it (u32::MAX: unknown).
    limit_ms: Mutex<u32>,
}

impl Sock {
    pub fn new(fd: i32) -> Arc<Sock> {
        Arc::new(Sock { fd, incoming: Mutex::default(), wanted: AtomicBool::new(false), limit_ms: Mutex::new(0) })
    }

    /// Bytes already received for it (by the knock that found it).
    fn with_first(fd: i32, first: &[u8], limit_ms: u32) -> Arc<Sock> {
        let s = Sock::new(fd);
        lock(&s.incoming).buf.extend(first);
        *lock(&s.limit_ms) = limit_ms;
        s
    }

    fn set_limit(&self, ms: u32) -> rs::Result<()> {
        let mut limit = lock(&self.limit_ms);
        if *limit != ms {
            rs::set_recv_timeout(self.fd, ms)?;
            *limit = ms;
        }
        Ok(())
    }

    /// One receive, waiting `ms` at most. Whether anything happened.
    fn fill(&self, ms: u32) -> bool {
        let mut chunk = [0u8; rs::MOST_RECEIVED];
        let got = self.set_limit(ms.max(1)).and_then(|()| rs::recv(self.fd, &mut chunk));
        let waker = {
            let mut i = lock(&self.incoming);
            match got {
                Ok(0) => i.eof = true,
                Ok(n) => i.buf.extend(&chunk[..n]),
                Err(Errno(errno::EAGAIN)) => return false,
                Err(e) => i.error = Some(e),
            }
            i.waker.take()
        };
        self.wanted.store(false, Ordering::SeqCst);
        if let Some(w) = waker {
            w.wake();
        }
        true
    }

    /// AsyncRead's poll: what is buffered, else the end or the error, else
    /// a wait in `turn`.
    pub fn poll_read(self: &Arc<Self>, cx: &mut Context<'_>, out: &mut tokio::io::ReadBuf<'_>)
        -> Poll<std::io::Result<()>>
    {
        let mut i = lock(&self.incoming);
        if !i.buf.is_empty() {
            let n = out.remaining().min(i.buf.len());
            let (a, b) = i.buf.as_slices();
            let first = n.min(a.len());
            out.put_slice(&a[..first]);
            out.put_slice(&b[..n - first]);
            i.buf.drain(..n);
            return Poll::Ready(Ok(()));
        }
        if let Some(e) = i.error {
            return Poll::Ready(Err(e.io()));
        }
        if i.eof {
            return Poll::Ready(Ok(()));
        }
        i.waker = Some(cx.waker().clone());
        drop(i);
        if !self.wanted.swap(true, Ordering::SeqCst) {
            READERS.with(|r| r.borrow_mut().push(Arc::downgrade(self)));
        }
        Poll::Pending
    }

    pub fn send(&self, data: &[u8]) -> std::io::Result<()> {
        rs::send_all(self.fd, data).map_err(Errno::io)
    }
}

impl Drop for Sock {
    fn drop(&mut self) {
        rs::close(self.fd);
    }
}

/// A listening TCP socket on `port` (on every address the device has).
pub struct Door {
    pub port: u16,
    fd: AtomicI32,
    /// What this run's knocks say, and how many it made.
    word: [u8; 16],
    knocks: AtomicU64,
    waiting: Mutex<VecDeque<Arc<Sock>>>,
    waker: Mutex<Option<Waker>>,
    wanted: AtomicBool,
    next_knock: Mutex<Instant>,
    /// A connection came lately: knock often.
    lively_until: Mutex<Instant>,
    opened: Mutex<Instant>,
}

impl Door {
    pub fn open(port: u16) -> std::io::Result<Arc<Door>> {
        let fd = listen(port).map_err(Errno::io)?;
        let now = Instant::now();
        Ok(Arc::new(Door {
            port, fd: AtomicI32::new(fd), word: rand::random(), knocks: AtomicU64::new(0),
            waiting: Mutex::default(), waker: Mutex::default(), wanted: AtomicBool::new(false),
            next_knock: Mutex::new(now), lively_until: Mutex::new(now + Duration::from_secs(10)),
            opened: Mutex::new(now),
        }))
    }

    pub fn poll_accept(self: &Arc<Self>, cx: &mut Context<'_>) -> Poll<Arc<Sock>> {
        if let Some(s) = lock(&self.waiting).pop_front() {
            return Poll::Ready(s);
        }
        *lock(&self.waker) = Some(cx.waker().clone());
        if !self.wanted.swap(true, Ordering::SeqCst) {
            DOORS.with(|d| d.borrow_mut().push(Arc::downgrade(self)));
        }
        Poll::Pending
    }

    fn due(&self, now: Instant) -> bool {
        now >= *lock(&self.next_knock)
    }

    fn next_knock(&self) -> Instant {
        *lock(&self.next_knock)
    }

    /// Knocks, and takes every connection waiting before the knock. Whether
    /// any came.
    fn knock(&self) -> bool {
        let now = Instant::now();
        let lively = now < *lock(&self.lively_until);
        *lock(&self.next_knock) = now + Duration::from_millis(if lively { 150 } else { 400 });
        let n = self.knocks.fetch_add(1, Ordering::SeqCst);
        let mut word = b"BariStore knock ".to_vec();
        word.extend_from_slice(&self.word);
        word.extend_from_slice(&n.to_be_bytes());
        let Ok(client) = rs::socket(rs::SOCK_STREAM) else { return false };
        if let Err(e) = rs::connect(client, "127.0.0.1", self.port).and_then(|()| rs::send_all(client, &word)) {
            rs::close(client);
            // A listener just made may not take connections yet.
            if lock(&self.opened).elapsed() > Duration::from_secs(2) {
                crate::logf!("web: port {} does not take connections ({e}); listening again", self.port);
                self.reopen();
            }
            return false;
        }
        let mut found = Vec::new();
        loop {
            let fd = match rs::accept(self.fd.load(Ordering::SeqCst)) {
                Ok(fd) => fd,
                Err(e) => {
                    crate::logf!("web: accept on port {}: {e}; listening again", self.port);
                    self.reopen();
                    break;
                }
            };
            let mut first = [0u8; 64];
            let mut got = rs::set_recv_timeout(fd, FIRST_BYTES_MS).and_then(|()| rs::recv(fd, &mut first));
            // A knock that came in pieces.
            while let Ok(len) = got {
                if len == 0 || len >= word.len() || !b"BariStore knock ".starts_with(&first[..len.min(16)]) {
                    break;
                }
                match rs::recv(fd, &mut first[len..]) {
                    Ok(0) | Err(_) => break,
                    Ok(more) => got = Ok(len + more),
                }
            }
            match got {
                Ok(len) if len >= 16 && first.starts_with(b"BariStore knock ") => {
                    rs::close(fd);
                    // Ours, this time: everything before it is taken.
                    // Another word is a knock left by an earlier run.
                    if first[..len] == word[..] {
                        break;
                    }
                }
                // A connection that ended before it said anything.
                Ok(0) => rs::close(fd),
                Ok(len) => found.push(Sock::with_first(fd, &first[..len], FIRST_BYTES_MS)),
                // Quiet so far: it may wait for us to speak first.
                Err(Errno(errno::EAGAIN)) => found.push(Sock::with_first(fd, &[], FIRST_BYTES_MS)),
                Err(_) => rs::close(fd),
            }
        }
        rs::close(client);
        if found.is_empty() {
            return false;
        }
        *lock(&self.lively_until) = Instant::now() + Duration::from_secs(3);
        lock(&self.waiting).extend(found);
        self.wanted.store(false, Ordering::SeqCst);
        if let Some(w) = lock(&self.waker).take() {
            w.wake();
        }
        true
    }

    fn reopen(&self) {
        rs::close(self.fd.load(Ordering::SeqCst));
        match listen(self.port) {
            Ok(fd) => {
                self.fd.store(fd, Ordering::SeqCst);
                *lock(&self.opened) = Instant::now();
            }
            Err(e) => crate::logf!("web: cannot listen on port {} again: {e}", self.port),
        }
    }
}

impl Drop for Door {
    fn drop(&mut self) {
        rs::close(self.fd.load(Ordering::SeqCst));
    }
}

fn listen(port: u16) -> rs::Result<i32> {
    let fd = rs::socket(rs::SOCK_STREAM)?;
    let opened = rs::bind(fd, [0, 0, 0, 0], port).and_then(|()| rs::listen(fd));
    if let Err(e) = opened {
        rs::close(fd);
        return Err(e);
    }
    Ok(fd)
}

thread_local! {
    static READERS: RefCell<Vec<Weak<Sock>>> = const { RefCell::new(Vec::new()) };
    static DOORS: RefCell<Vec<Weak<Door>>> = const { RefCell::new(Vec::new()) };
    static SLEEPER: Cell<i32> = const { Cell::new(-1) };
    static CURSOR: Cell<usize> = const { Cell::new(0) };
}

/// Waits in rootshell for up to `wait` on a socket nothing is sent to.
fn idle(wait: Duration) {
    let ms = wait.as_millis().clamp(1, u32::MAX as u128) as u32;
    let mut fd = SLEEPER.with(Cell::get);
    if fd < 0 {
        match rs::socket(rs::SOCK_DGRAM) {
            Ok(s) => {
                fd = s;
                SLEEPER.with(|c| c.set(s));
            }
            Err(_) => return std::thread::sleep(wait),
        }
    }
    let mut buf = [0u8; 16];
    if rs::set_recv_timeout(fd, ms).and_then(|()| rs::recv(fd, &mut buf)).is_err_and(|e| e.0 != errno::EAGAIN) {
        std::thread::sleep(wait);
    }
}

/// One wait for sockets, until something happens or `deadline` (at once
/// when it has passed; `SLICE` at most).
pub fn turn(deadline: Option<Instant>) {
    let now = Instant::now();
    let end = deadline.unwrap_or(now + SLICE).min(now + SLICE);
    let quick = end <= now;
    let doors: Vec<Arc<Door>> = DOORS.with(|d| {
        let mut d = d.borrow_mut();
        let mut live: Vec<Arc<Door>> = d.iter().filter_map(Weak::upgrade)
            .filter(|door| door.wanted.load(Ordering::SeqCst)).collect();
        live.sort_by_key(|door| Arc::as_ptr(door) as usize);
        live.dedup_by_key(|door| Arc::as_ptr(door) as usize);
        *d = live.iter().map(Arc::downgrade).collect();
        live
    });
    let mut limit = end;
    for door in &doors {
        if door.due(now) && door.knock() {
            return;
        }
        limit = limit.min(door.next_knock());
    }
    let readers: Vec<Arc<Sock>> = READERS.with(|r| {
        let mut r = r.borrow_mut();
        let mut live: Vec<Arc<Sock>> = r.iter().filter_map(Weak::upgrade)
            .filter(|s| s.wanted.load(Ordering::SeqCst)).collect();
        live.sort_by_key(|s| Arc::as_ptr(s) as usize);
        live.dedup_by_key(|s| Arc::as_ptr(s) as usize);
        *r = live.iter().map(Arc::downgrade).collect();
        live
    });
    let now = Instant::now();
    if readers.is_empty() {
        if !quick && limit > now {
            idle(limit - now);
        }
        return;
    }
    let ms = |until: Instant| until.saturating_duration_since(Instant::now()).as_millis().clamp(1, 60_000) as u32;
    let start = CURSOR.with(|c| {
        let s = c.get();
        c.set(s.wrapping_add(1));
        s
    });
    for k in 0..readers.len() {
        let s = &readers[(start + k) % readers.len()];
        let wait = if quick { 1 } else if readers.len() == 1 { ms(limit) } else { SHARED_WAIT_MS };
        if s.fill(wait) || Instant::now() >= limit {
            return;
        }
    }
}
