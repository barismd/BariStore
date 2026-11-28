//! rootshell's socket calls on the system's own sockets, for `cargo test`
//! and debug builds off WebAssembly. They keep rootshell's rules: blocking
//! calls, `accept` without a time limit, receive time limits answered with
//! EAGAIN, socket numbers from 100, no TLS (rootshell's own, which only the
//! device has). Datagrams to the mDNS group are counted and dropped, so a
//! test never announces "Pair with …" on the network.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::errno::*;

enum Entry {
    /// TCP, before connect or listen: the port `bind` gave.
    Tcp(u16),
    Listener(TcpListener),
    Stream(TcpStream),
    Udp(Option<UdpSocket>, u16),
}

struct Socket {
    entry: Entry,
    timeout: Option<Duration>,
}

fn table() -> &'static Mutex<HashMap<i32, Socket>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, Socket>>> = OnceLock::new();
    TABLE.get_or_init(Default::default)
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<i32, Socket>> {
    table().lock().unwrap_or_else(|e| e.into_inner())
}

fn insert(entry: Entry) -> i32 {
    static NEXT: AtomicI32 = AtomicI32::new(100);
    let fd = NEXT.fetch_add(1, Ordering::Relaxed);
    lock().insert(fd, Socket { entry, timeout: None });
    fd
}

fn errno_of(e: &std::io::Error) -> i32 {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::ConnectionRefused => ECONNREFUSED,
        K::AddrInUse => EADDRINUSE,
        K::WouldBlock | K::TimedOut => EAGAIN,
        K::BrokenPipe => EPIPE,
        _ => EIO,
    }
}

/// What a blocking call works on, cloned so the table is free meanwhile.
enum Handle {
    Listener(TcpListener),
    Stream(TcpStream, Option<Duration>),
    Udp(UdpSocket, Option<Duration>),
}

fn handle(fd: i32) -> Result<Handle, i32> {
    let mut t = lock();
    let s = t.get_mut(&fd).ok_or(EBADF)?;
    Ok(match &mut s.entry {
        Entry::Listener(l) => Handle::Listener(l.try_clone().map_err(|e| errno_of(&e))?),
        Entry::Stream(st) => Handle::Stream(st.try_clone().map_err(|e| errno_of(&e))?, s.timeout),
        Entry::Udp(sock, port) => {
            if sock.is_none() {
                *sock = Some(UdpSocket::bind((Ipv4Addr::UNSPECIFIED, *port)).map_err(|e| errno_of(&e))?);
            }
            Handle::Udp(sock.as_ref().unwrap().try_clone().map_err(|e| errno_of(&e))?, s.timeout)
        }
        Entry::Tcp(_) => return Err(EINVAL),
    })
}

unsafe fn text<'a>(ptr: *const u8, len: i32) -> &'a str {
    // SAFETY: the callers pass a str's pointer and length.
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len as usize)) }
}

pub unsafe fn rootshell_socket_socket(_domain: i32, kind: i32, fd_out: *mut i32) -> i32 {
    let entry = match kind {
        super::SOCK_STREAM => Entry::Tcp(0),
        super::SOCK_DGRAM => Entry::Udp(None, 0),
        _ => return EINVAL,
    };
    // SAFETY: the caller's out-parameter.
    unsafe { *fd_out = insert(entry) };
    0
}

pub unsafe fn rootshell_socket_bind(fd: i32, addr: *const u8, addrlen: i32) -> i32 {
    if addrlen < 8 {
        return EINVAL;
    }
    // SAFETY: the caller's 16-byte sockaddr_in.
    let port = unsafe { u16::from_be_bytes([*addr.add(2), *addr.add(3)]) };
    match lock().get_mut(&fd).map(|s| &mut s.entry) {
        Some(Entry::Tcp(p)) | Some(Entry::Udp(None, p)) => {
            *p = port;
            0
        }
        Some(_) => EINVAL,
        None => EBADF,
    }
}

pub unsafe fn rootshell_socket_listen(fd: i32, _backlog: i32) -> i32 {
    let mut t = lock();
    let Some(s) = t.get_mut(&fd) else { return EBADF };
    let Entry::Tcp(port) = s.entry else { return EINVAL };
    // Loopback only: tests must not listen on the network.
    match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
        Ok(l) => {
            s.entry = Entry::Listener(l);
            0
        }
        Err(e) => errno_of(&e),
    }
}

pub unsafe fn rootshell_socket_accept(fd: i32, fd_out: *mut i32) -> i32 {
    let Handle::Listener(l) = (match handle(fd) { Ok(h) => h, Err(e) => return e }) else { return EINVAL };
    match l.accept() {
        Ok((stream, _)) => {
            // SAFETY: the caller's out-parameter.
            unsafe { *fd_out = insert(Entry::Stream(stream)) };
            0
        }
        Err(e) => errno_of(&e),
    }
}

pub unsafe fn rootshell_socket_connect_host(fd: i32, host: *const u8, host_len: i32, port: u16) -> i32 {
    // SAFETY: the caller's string.
    let host = unsafe { text(host, host_len) };
    if let Ok(Handle::Udp(u, _)) = handle(fd) {
        return match u.connect((host, port)) { Ok(()) => 0, Err(e) => errno_of(&e) };
    }
    if !matches!(lock().get(&fd).map(|s| &s.entry), Some(Entry::Tcp(_))) {
        return EINVAL;
    }
    let addrs: Vec<SocketAddr> = match (host, port).to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(_) => return ENOENT,
    };
    let mut last = ECONNREFUSED;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(10)) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                if let Some(s) = lock().get_mut(&fd) {
                    s.entry = Entry::Stream(stream);
                }
                return 0;
            }
            Err(e) => last = errno_of(&e),
        }
    }
    last
}

/// rootshell's TLS is Network.framework's: not here.
pub unsafe fn rootshell_socket_tls_connect_host(_fd: i32, _host: *const u8, _len: i32, _port: u16) -> i32 {
    ENOSYS
}

pub unsafe fn rootshell_socket_send(fd: i32, buf: *const u8, len: i32, sent: *mut u32) -> i32 {
    // SAFETY: the caller's buffer.
    let data = unsafe { std::slice::from_raw_parts(buf, len as usize) };
    match handle(fd) {
        Ok(Handle::Stream(mut s, _)) => match s.write_all(data) {
            Ok(()) => {
                // SAFETY: the caller's out-parameter.
                unsafe { *sent = len as u32 };
                0
            }
            Err(_) => EIO,
        },
        Ok(Handle::Udp(u, _)) => match u.send(data) {
            Ok(n) => {
                // SAFETY: as above.
                unsafe { *sent = n as u32 };
                0
            }
            Err(e) => errno_of(&e),
        },
        Ok(_) => EINVAL,
        Err(e) => e,
    }
}

pub unsafe fn rootshell_socket_recv(fd: i32, buf: *mut u8, len: i32, got: *mut u32) -> i32 {
    // SAFETY: the caller's buffer.
    let buf = unsafe { std::slice::from_raw_parts_mut(buf, len as usize) };
    let result = match handle(fd) {
        Ok(Handle::Stream(mut s, timeout)) => {
            let _ = s.set_read_timeout(timeout);
            s.read(buf)
        }
        Ok(Handle::Udp(u, timeout)) => {
            let _ = u.set_read_timeout(timeout);
            u.recv(buf)
        }
        Ok(Handle::Listener(_)) => return EINVAL,
        Err(e) => return e,
    };
    match result {
        // rootshell's 64 KB answer slot: a receive this big fails.
        Ok(n) if n > 48 * 1024 => EINVAL,
        Ok(n) => {
            // SAFETY: the caller's out-parameter.
            unsafe { *got = n as u32 };
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {
            // SAFETY: as above.
            unsafe { *got = 0 };
            0
        }
        Err(e) => errno_of(&e),
    }
}

pub unsafe fn rootshell_socket_set_recv_timeout(fd: i32, timeout_ms: u32) -> i32 {
    match lock().get_mut(&fd) {
        Some(s) => {
            s.timeout = (timeout_ms > 0).then(|| Duration::from_millis(timeout_ms.into()));
            0
        }
        None => EBADF,
    }
}

static MULTICAST: AtomicUsize = AtomicUsize::new(0);

/// How many datagrams went to the mDNS group (and were dropped).
#[cfg(test)]
pub fn multicast_sent() -> usize {
    MULTICAST.load(Ordering::SeqCst)
}

pub unsafe fn rootshell_socket_sendto_host(fd: i32, buf: *const u8, len: i32, host: *const u8, host_len: i32,
                                           port: u16, sent: *mut u32) -> i32 {
    // SAFETY: the caller's string and buffer.
    let (host, data) = unsafe { (text(host, host_len), std::slice::from_raw_parts(buf, len as usize)) };
    let Ok(Handle::Udp(u, _)) = handle(fd) else { return EBADF };
    if host == "224.0.0.251" {
        MULTICAST.fetch_add(1, Ordering::SeqCst);
    } else if let Err(e) = u.send_to(data, (host, port)) {
        return errno_of(&e);
    }
    // SAFETY: the caller's out-parameter.
    unsafe { *sent = len as u32 };
    0
}

pub unsafe fn rootshell_socket_recvfrom(fd: i32, buf: *mut u8, len: i32, addr: *mut u8, addrlen_io: *mut i32,
                                        got: *mut u32) -> i32 {
    let Ok(Handle::Udp(u, timeout)) = handle(fd) else { return EBADF };
    let _ = u.set_read_timeout(timeout);
    // SAFETY: the caller's buffer.
    let buf = unsafe { std::slice::from_raw_parts_mut(buf, len as usize) };
    match u.recv_from(buf) {
        Ok((n, from)) => {
            // SAFETY: the caller's out-parameters, `addr` with 16 bytes room.
            unsafe {
                *got = n as u32;
                if let SocketAddr::V4(v4) = from {
                    *addr = super::AF_INET as u8;
                    std::ptr::copy_nonoverlapping(v4.port().to_be_bytes().as_ptr(), addr.add(2), 2);
                    std::ptr::copy_nonoverlapping(v4.ip().octets().as_ptr(), addr.add(4), 4);
                    *addrlen_io = 16;
                } else {
                    *addrlen_io = 0;
                }
            }
            0
        }
        Err(e) => errno_of(&e),
    }
}

pub unsafe fn rootshell_socket_close(fd: i32) -> i32 {
    match lock().remove(&fd) {
        Some(Socket { entry: Entry::Stream(s), .. }) => {
            let _ = s.shutdown(std::net::Shutdown::Both);
            0
        }
        Some(_) => 0,
        None => EBADF,
    }
}

/// No Bonjour here: nothing found.
pub unsafe fn rootshell_socket_bonjour_lookup(_kind: *const u8, _kind_len: i32, _timeout_ms: u32, _out: *mut u8,
                                              _out_max: i32, _out_len: *mut u32) -> i32 {
    ETIMEDOUT
}
