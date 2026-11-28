//! What rootshell (kitknox/rootshell, App Store) lends the WebAssembly
//! program BariStore is, besides WASI Preview 1: its socket calls,
//! `rootshell_socket_*` (wasm/README.md, Resources/Wasm/wasm-worker.js and
//! rootshell/Features/Wasm/WasmSocketBroker.swift, MIT, read 2026-11-27).
//!
//! How rootshell runs a program, and what that means here:
//! * One thread. Every call goes from a Web Worker through rootshell's hidden
//!   web view to Swift and back, and blocks the program until Network.framework
//!   answers: there is no poll, so `crate::rt` waits on one socket at a time
//!   with receive time limits, and `accept` (which only returns with a
//!   connection) is only called once a connection is known to be waiting
//!   (`crate::net::Listener`).
//! * A call's answer comes back through one 64 KB slot as base64 in JSON; a
//!   receive that brings more fails with EINVAL and its bytes are gone, so a
//!   receive asks for `MOST_RECEIVED` at most (files: `files.rs`).
//! * TLS is rootshell's own (Network.framework, Apple's trust store):
//!   `tls_connect_host`, then plain send/receive.
//! * `getsockname` always says 0.0.0.0; a TCP `bind` only takes the port and
//!   the listener takes connections on every address the device has.
//! * Socket numbers start at 100 and are not WASI file descriptors.
//!
//! Off WebAssembly (`cargo test`) the same calls run on the system's own
//! sockets with rootshell's rules (`native.rs`).

pub mod files;
pub mod osc;
#[cfg(not(target_arch = "wasm32"))]
mod native;

pub const AF_INET: i32 = 2;
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;

/// The WASI errno numbers rootshell answers with (WasmFilesystemBroker.swift
/// `Errno`; EADDRINUSE is its own 1).
pub mod errno {
    pub const EADDRINUSE: i32 = 1;
    pub const EACCES: i32 = 2;
    pub const EAGAIN: i32 = 6;
    pub const EBADF: i32 = 8;
    pub const ECONNREFUSED: i32 = 14;
    pub const EINTR: i32 = 27;
    pub const EINVAL: i32 = 28;
    pub const EIO: i32 = 29;
    pub const ENOBUFS: i32 = 42;
    pub const ENOENT: i32 = 44;
    pub const ENETUNREACH: i32 = 50;
    pub const ENOSYS: i32 = 52;
    pub const EPIPE: i32 = 64;
    pub const ETIMEDOUT: i32 = 73;
}

/// The most one receive asks for: its base64 (43 692 bytes) and the JSON
/// around it fit rootshell's 64 KB answer slot with room to spare.
pub const MOST_RECEIVED: usize = 32 * 1024;
/// The most one send hands over: rootshell builds each send's base64 a
/// character at a time, so big ones are slow, never refused.
pub const MOST_SENT: usize = 256 * 1024;

/// A rootshell errno, with what it means in words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Errno(pub i32);

impl Errno {
    pub fn io(self) -> std::io::Error {
        use std::io::ErrorKind as K;
        let kind = match self.0 {
            errno::ECONNREFUSED => K::ConnectionRefused,
            errno::ETIMEDOUT => K::TimedOut,
            errno::EAGAIN => K::WouldBlock,
            errno::EADDRINUSE => K::AddrInUse,
            errno::EACCES => K::PermissionDenied,
            errno::EPIPE => K::BrokenPipe,
            errno::EINTR => K::Interrupted,
            errno::ENOENT => K::NotFound,
            errno::ENETUNREACH => K::NetworkUnreachable,
            _ => K::Other,
        };
        std::io::Error::new(kind, self.to_string())
    }
}

impl std::fmt::Display for Errno {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let words = match self.0 {
            errno::EADDRINUSE => "address in use",
            errno::EACCES => "not allowed (Local Network access for rootshell?)",
            errno::EAGAIN => "nothing came in time",
            errno::EBADF => "no such socket",
            errno::ECONNREFUSED => "refused",
            errno::EINTR => "interrupted",
            errno::EINVAL => "invalid",
            errno::EIO => "connection failed",
            errno::ENOBUFS => "answer too long",
            errno::ENOENT => "name not found",
            errno::ENETUNREACH => "no route",
            errno::ENOSYS => "not offered by this rootshell",
            errno::EPIPE => "closed",
            errno::ETIMEDOUT => "timed out",
            _ => "error",
        };
        write!(f, "{words} (rootshell errno {})", self.0)
    }
}

impl std::error::Error for Errno {}

pub type Result<T> = std::result::Result<T, Errno>;

#[cfg(target_arch = "wasm32")]
mod calls {
    #[link(wasm_import_module = "rootshell_socket")]
    extern "C" {
        pub fn rootshell_socket_socket(domain: i32, kind: i32, fd_out: *mut i32) -> i32;
        pub fn rootshell_socket_bind(fd: i32, addr: *const u8, addrlen: i32) -> i32;
        pub fn rootshell_socket_listen(fd: i32, backlog: i32) -> i32;
        pub fn rootshell_socket_accept(fd: i32, fd_out: *mut i32) -> i32;
        pub fn rootshell_socket_connect_host(fd: i32, host: *const u8, host_len: i32, port: u16) -> i32;
        pub fn rootshell_socket_tls_connect_host(fd: i32, host: *const u8, host_len: i32, port: u16) -> i32;
        pub fn rootshell_socket_send(fd: i32, buf: *const u8, len: i32, sent: *mut u32) -> i32;
        pub fn rootshell_socket_recv(fd: i32, buf: *mut u8, len: i32, got: *mut u32) -> i32;
        pub fn rootshell_socket_sendto_host(fd: i32, buf: *const u8, len: i32, host: *const u8, host_len: i32,
                                            port: u16, sent: *mut u32) -> i32;
        pub fn rootshell_socket_recvfrom(fd: i32, buf: *mut u8, len: i32, addr: *mut u8, addrlen_io: *mut i32,
                                         got: *mut u32) -> i32;
        pub fn rootshell_socket_close(fd: i32) -> i32;
    }
    // rootshell 1.0.16-177 added these two. Under `env` an older rootshell
    // still starts the program (it answers a call it lacks with 0), so
    // `check` can say what to update instead of a link error.
    #[link(wasm_import_module = "env")]
    extern "C" {
        pub fn rootshell_socket_set_recv_timeout(fd: i32, timeout_ms: u32) -> i32;
        pub fn rootshell_socket_bonjour_lookup(kind: *const u8, kind_len: i32, timeout_ms: u32, out: *mut u8,
                                               out_max: i32, out_len: *mut u32) -> i32;
    }
}

#[cfg(not(target_arch = "wasm32"))]
use native as calls;

use calls::*;

fn check_errno(code: i32) -> Result<()> {
    if code == 0 { Ok(()) } else { Err(Errno(code)) }
}

pub fn socket(kind: i32) -> Result<i32> {
    let mut fd = -1;
    // SAFETY: `fd` outlives the call, which writes one i32 into it.
    check_errno(unsafe { rootshell_socket_socket(AF_INET, kind, &mut fd) })?;
    Ok(fd)
}

/// BSD `sockaddr_in` as rootshell reads it: the family in the first byte.
fn sockaddr(ip: [u8; 4], port: u16) -> [u8; 16] {
    let mut a = [0u8; 16];
    a[0] = AF_INET as u8;
    a[2..4].copy_from_slice(&port.to_be_bytes());
    a[4..8].copy_from_slice(&ip);
    a
}

pub fn bind(fd: i32, ip: [u8; 4], port: u16) -> Result<()> {
    let a = sockaddr(ip, port);
    // SAFETY: `a` is a 16-byte sockaddr_in for the length of the call.
    check_errno(unsafe { rootshell_socket_bind(fd, a.as_ptr(), a.len() as i32) })
}

pub fn listen(fd: i32) -> Result<()> {
    // SAFETY: plain integers.
    check_errno(unsafe { rootshell_socket_listen(fd, 64) })
}

/// Blocks until a connection comes: see `crate::net::Listener`.
pub fn accept(fd: i32) -> Result<i32> {
    let mut out = -1;
    // SAFETY: `out` outlives the call.
    check_errno(unsafe { rootshell_socket_accept(fd, &mut out) })?;
    Ok(out)
}

/// TCP to a host name or address literal (rootshell resolves names).
pub fn connect(fd: i32, host: &str, port: u16) -> Result<()> {
    // SAFETY: `host` is valid UTF-8 for the length of the call.
    check_errno(unsafe { rootshell_socket_connect_host(fd, host.as_ptr(), host.len() as i32, port) })
}

/// TLS over TCP, rootshell's (SNI and the checks of Apple's trust store).
pub fn connect_tls(fd: i32, host: &str, port: u16) -> Result<()> {
    // SAFETY: as `connect`.
    check_errno(unsafe { rootshell_socket_tls_connect_host(fd, host.as_ptr(), host.len() as i32, port) })
}

/// Sends `data` whole, `MOST_SENT` at a time.
pub fn send_all(fd: i32, mut data: &[u8]) -> Result<()> {
    while !data.is_empty() {
        let piece = &data[..data.len().min(MOST_SENT)];
        let mut sent = 0u32;
        // SAFETY: `piece` and `sent` outlive the call.
        check_errno(unsafe { rootshell_socket_send(fd, piece.as_ptr(), piece.len() as i32, &mut sent) })?;
        if sent == 0 {
            return Err(Errno(errno::EPIPE));
        }
        data = &data[sent as usize..];
    }
    Ok(())
}

/// One receive into `buf` (`MOST_RECEIVED` at most): Ok(0) is the end of
/// the stream, EAGAIN that the socket's time limit passed first.
pub fn recv(fd: i32, buf: &mut [u8]) -> Result<usize> {
    let len = buf.len().min(MOST_RECEIVED);
    let mut got = 0u32;
    // SAFETY: `buf` has room for `len` bytes; `got` outlives the call.
    check_errno(unsafe { rootshell_socket_recv(fd, buf.as_mut_ptr(), len as i32, &mut got) })?;
    Ok(got as usize)
}

/// Receives on `fd` give up after `ms` with EAGAIN (0: wait for ever).
pub fn set_recv_timeout(fd: i32, ms: u32) -> Result<()> {
    // SAFETY: plain integers.
    check_errno(unsafe { rootshell_socket_set_recv_timeout(fd, ms) })
}

pub fn send_to(fd: i32, data: &[u8], host: &str, port: u16) -> Result<()> {
    let mut sent = 0u32;
    // SAFETY: `data`, `host` and `sent` outlive the call.
    check_errno(unsafe {
        rootshell_socket_sendto_host(fd, data.as_ptr(), data.len() as i32, host.as_ptr(), host.len() as i32, port,
                                     &mut sent)
    })
}

/// One datagram and the IPv4 address it came from (None for another kind).
pub fn recv_from(fd: i32, buf: &mut [u8]) -> Result<(usize, Option<[u8; 4]>)> {
    let len = buf.len().min(MOST_RECEIVED);
    let mut addr = [0u8; 28];
    let mut addr_len = addr.len() as i32;
    let mut got = 0u32;
    // SAFETY: every pointer has the room the call is told about.
    check_errno(unsafe {
        rootshell_socket_recvfrom(fd, buf.as_mut_ptr(), len as i32, addr.as_mut_ptr(), &mut addr_len, &mut got)
    })?;
    let from = (addr_len >= 8 && addr[0] == AF_INET as u8).then(|| [addr[4], addr[5], addr[6], addr[7]]);
    Ok((got as usize, from))
}

pub fn close(fd: i32) {
    // SAFETY: plain integer; closing an unknown socket only answers EBADF.
    unsafe { rootshell_socket_close(fd) };
}

/// One Bonjour browse (rootshell 1.0.16-177): each instance of `kind` as
/// (name, host, port, addresses). Only the types rootshell declares can be
/// browsed (`_ssh._tcp`, `_rfb._tcp`, `_remotepairing._tcp`).
pub fn bonjour_lookup(kind: &str, timeout_ms: u32) -> Result<Vec<(String, String, u16, Vec<String>)>> {
    let mut out = vec![0u8; MOST_RECEIVED];
    let mut len = 0u32;
    // SAFETY: `out` has the room the call is told about.
    check_errno(unsafe {
        rootshell_socket_bonjour_lookup(kind.as_ptr(), kind.len() as i32, timeout_ms, out.as_mut_ptr(),
                                        out.len() as i32, &mut len)
    })?;
    let text = String::from_utf8_lossy(&out[..len as usize]).into_owned();
    Ok(text.lines().filter_map(|line| {
        let mut f = line.split('\t');
        let (name, host, port, addrs) = (f.next()?, f.next()?, f.next()?, f.next().unwrap_or(""));
        Some((name.to_string(), host.to_string(), port.parse().ok()?,
              addrs.split(',').filter(|a| !a.is_empty()).map(str::to_string).collect()))
    }).collect())
}

/// Whether this rootshell has the receive time limits BariStore waits with
/// (1.0.16-177 and later): a limit on a socket that does not exist is EBADF
/// there; an older one answers its stand-in's 0.
pub fn has_recv_timeouts() -> bool {
    set_recv_timeout(-1, 1) == Err(Errno(errno::EBADF))
}

/// A call to rootshell that changes nothing. rootshell runs its programs in
/// a web view that is in no window, and WebKit can stop such a view's work
/// while nothing passes between it and the app (atelier-install saw a hash
/// of a big file stop until a key was pressed): long computations make a
/// call at least every few dozen milliseconds (`crate::rt::check_in` looks
/// at the sockets; the allocator nudges).
#[cfg(target_arch = "wasm32")]
pub fn nudge() {
    let _ = set_recv_timeout(-1, 1);
}

/// The allocator, nudging rootshell every 64th allocation once 40 ms passed
/// since the last nudge: computations no code of BariStore's runs inside
/// (RSA keys, pictures) allocate as they go. Never more than a call to
/// rootshell, which calls nothing back.
#[cfg(target_arch = "wasm32")]
pub struct Nudging;

#[cfg(target_arch = "wasm32")]
mod nudging {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    static COUNT: AtomicU32 = AtomicU32::new(0);
    static LAST_MS: AtomicU64 = AtomicU64::new(0);

    fn tick() {
        if COUNT.fetch_add(1, Ordering::Relaxed) % 64 != 0 {
            return;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if now.saturating_sub(LAST_MS.load(Ordering::Relaxed)) >= 40 {
            LAST_MS.store(now, Ordering::Relaxed);
            super::nudge();
        }
    }

    // SAFETY: System's allocator, with a call to rootshell beside it.
    unsafe impl GlobalAlloc for super::Nudging {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            tick();
            // SAFETY: as GlobalAlloc::alloc.
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            tick();
            // SAFETY: as GlobalAlloc::alloc_zeroed.
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            tick();
            // SAFETY: as GlobalAlloc::realloc.
            unsafe { System.realloc(ptr, layout, new_size) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: as GlobalAlloc::dealloc.
            unsafe { System.dealloc(ptr, layout) }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub use native::multicast_sent;
