//! TCP and UDP through rootshell, as tokio's types are used: streams that
//! read through `crate::rt::io` (so a waiting read lets other tasks run),
//! listeners that never block on `accept`. Connecting and sending are
//! rootshell calls that block until Network.framework answers.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::rootshell::{self as rs, Errno};
use crate::rt::io::{Door, Sock};

/// A connected TCP stream (plain, or TLS by rootshell).
pub struct TcpStream {
    sock: Arc<Sock>,
}

fn opened(fd: i32, connected: rs::Result<()>) -> io::Result<TcpStream> {
    match connected {
        Ok(()) => Ok(TcpStream { sock: Sock::new(fd) }),
        Err(e) => {
            rs::close(fd);
            Err(e.io())
        }
    }
}

impl TcpStream {
    /// To a host name or an address literal.
    pub async fn connect(host: &str, port: u16) -> io::Result<TcpStream> {
        let fd = rs::socket(rs::SOCK_STREAM).map_err(Errno::io)?;
        opened(fd, rs::connect(fd, host, port))
    }

    /// TLS to `host`, rootshell's: SNI and the certificate checked with
    /// Apple's trust store. Reads and writes are then the plain bytes.
    pub async fn connect_tls(host: &str, port: u16) -> io::Result<TcpStream> {
        let fd = rs::socket(rs::SOCK_STREAM).map_err(Errno::io)?;
        opened(fd, rs::connect_tls(fd, host, port))
    }

    pub fn into_split(self) -> (OwnedReadHalf, OwnedWriteHalf) {
        (OwnedReadHalf { sock: self.sock.clone() }, OwnedWriteHalf { sock: self.sock })
    }
}

fn read(sock: &Arc<Sock>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
    sock.poll_read(cx, buf)
}

fn write(sock: &Sock, buf: &[u8]) -> Poll<io::Result<usize>> {
    Poll::Ready(sock.send(buf).map(|()| buf.len()))
}

impl AsyncRead for TcpStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        read(&self.sock, cx, buf)
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        write(&self.sock, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub struct OwnedReadHalf {
    sock: Arc<Sock>,
}

pub struct OwnedWriteHalf {
    sock: Arc<Sock>,
}

impl AsyncRead for OwnedReadHalf {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        read(&self.sock, cx, buf)
    }
}

impl AsyncWrite for OwnedWriteHalf {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        write(&self.sock, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// A TCP listener. rootshell's takes connections on every address the
/// device has: the page checks who asks (web.rs).
pub struct TcpListener {
    door: Arc<Door>,
}

impl TcpListener {
    pub fn bind(port: u16) -> io::Result<TcpListener> {
        Ok(TcpListener { door: Door::open(port)? })
    }

    pub fn port(&self) -> u16 {
        self.door.port
    }

    pub async fn accept(&self) -> io::Result<TcpStream> {
        let sock = std::future::poll_fn(|cx| self.door.poll_accept(cx)).await;
        Ok(TcpStream { sock })
    }
}

/// Whether something listens on 127.0.0.1:`port` (rootshell's listener
/// takes a port another program already listens on, so look first).
pub fn in_use(port: u16) -> bool {
    let Ok(fd) = rs::socket(rs::SOCK_STREAM) else { return false };
    let taken = rs::connect(fd, "127.0.0.1", port).is_ok();
    rs::close(fd);
    taken
}

/// `data` sent from a UDP port of its own to that same port at `host`, and
/// whether it comes back to it within `wait`. LocalDevVPN hands every packet
/// for its address back to the device with only the addresses swapped
/// (device/session.rs `vpn_up`), so through it the datagram arrives at this
/// very socket; without it nothing does. The other tasks run while it waits.
pub async fn udp_echo(host: &str, data: &[u8], wait: Duration) -> io::Result<bool> {
    let fd = rs::socket(rs::SOCK_DGRAM).map_err(Errno::io)?;
    let sock = Sock::new(fd);
    let port = (0..8)
        .map(|_| 49152 + rand::random::<u16>() % 16383)
        .find(|&p| p != 62078 && rs::bind(fd, [0, 0, 0, 0], p).is_ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::AddrInUse, "no free UDP port"))?;
    // Connected, so that only what comes from host:port is received.
    rs::connect(fd, host, port).and_then(|()| rs::send_all(fd, data)).map_err(Errno::io)?;
    let until = Instant::now() + wait;
    let mut got = Vec::new();
    loop {
        let left = until.saturating_duration_since(Instant::now());
        let mut buf = [0u8; 256];
        let mut read = ReadBuf::new(&mut buf);
        match crate::rt::time::timeout(left, std::future::poll_fn(|cx| sock.poll_read(cx, &mut read))).await {
            Err(_) => return Ok(false),
            Ok(Err(e)) => return Err(e),
            Ok(Ok(())) if read.filled().is_empty() => return Ok(false),
            Ok(Ok(())) => {
                got.extend_from_slice(read.filled());
                if got.windows(data.len()).any(|w| w == data) {
                    return Ok(true);
                }
            }
        }
    }
}

/// A UDP socket, used as BSD's are, each call blocking: the mDNS
/// announcements and the look at what they bring back.
pub struct UdpSocket {
    fd: i32,
}

impl UdpSocket {
    /// Bound to `port` (0: any) on every address.
    pub fn bind(port: u16) -> io::Result<UdpSocket> {
        let fd = rs::socket(rs::SOCK_DGRAM).map_err(Errno::io)?;
        if port != 0 {
            if let Err(e) = rs::bind(fd, [0, 0, 0, 0], port) {
                rs::close(fd);
                return Err(e.io());
            }
        }
        Ok(UdpSocket { fd })
    }

    pub fn send_to(&self, data: &[u8], host: &str, port: u16) -> io::Result<()> {
        rs::send_to(self.fd, data, host, port).map_err(Errno::io)
    }



    /// One datagram and its sender, or None when nothing came in `wait`.
    pub fn recv_from(&self, buf: &mut [u8], wait: Duration) -> io::Result<Option<(usize, Option<[u8; 4]>)>> {
        let ms = wait.as_millis().clamp(1, u32::MAX as u128) as u32;
        match rs::set_recv_timeout(self.fd, ms).and_then(|()| rs::recv_from(self.fd, buf)) {
            Ok(got) => Ok(Some(got)),
            Err(Errno(rs::errno::EAGAIN)) => Ok(None),
            Err(e) => Err(e.io()),
        }
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        rs::close(self.fd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A free port (as `free_port` in main.rs: there is no port 0 to ask for).
    fn port() -> u16 {
        loop {
            let p = 40000 + rand::random::<u16>() % 20000;
            if !in_use(p) {
                return p;
            }
        }
    }

    /// A listener found by knocking, while another task keeps ticking: the
    /// server reads, answers and the client reads the end.
    #[test]
    fn listen_accept_and_echo_alongside_a_timer() {
        rt::block_on(async {
            let listener = TcpListener::bind(port()).unwrap();
            let p = listener.port();
            let ticks = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let t = ticks.clone();
            let ticker = rt::spawn(async move {
                loop {
                    rt::time::sleep(Duration::from_millis(20)).await;
                    t.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            });
            let server = rt::spawn(async move {
                for _ in 0..2 {
                    let mut s = listener.accept().await.unwrap();
                    let mut got = vec![0u8; 5];
                    s.read_exact(&mut got).await.unwrap();
                    s.write_all(&[got.as_slice(), b"!"].concat()).await.unwrap();
                }
            });
            for word in [&b"hello"[..], b"again"] {
                rt::time::sleep(Duration::from_millis(100)).await;
                let mut c = TcpStream::connect("127.0.0.1", p).await.unwrap();
                c.write_all(word).await.unwrap();
                let mut back = Vec::new();
                c.read_to_end(&mut back).await.unwrap();
                assert_eq!(back, [word, b"!"].concat());
            }
            server.await.unwrap();
            ticker.abort();
            assert!(ticks.load(std::sync::atomic::Ordering::SeqCst) >= 5);
        });
    }

    /// A datagram sent to its own port comes back (as LocalDevVPN sends it
    /// back); one sent where nothing sends it back does not.
    #[test]
    fn udp_echoes() {
        rt::block_on(async {
            assert!(udp_echo("127.0.0.1", b"BariStore 42", Duration::from_secs(2)).await.unwrap());
            // TEST-NET-1 (RFC 5737): nothing sends it back from there.
            assert!(!udp_echo("192.0.2.1", b"BariStore 43", Duration::from_millis(600)).await.unwrap_or(false));
        });
    }

    /// A transfer larger than one receive arrives whole. Sent in pieces
    /// smaller than one receive, waiting between them: a send blocks until
    /// the system took the bytes, and the reader is this same thread (sent
    /// whole, it hung on CI's smaller socket buffers).
    #[test]
    fn large_transfer() {
        rt::block_on(async {
            let listener = TcpListener::bind(port()).unwrap();
            let p = listener.port();
            let data: Vec<u8> = (0..3_000_000u32).map(|i| (i * 31 + i / 7) as u8).collect();
            let sent = data.clone();
            let server = rt::spawn(async move {
                let mut s = listener.accept().await.unwrap();
                for piece in sent.chunks(16 * 1024) {
                    s.write_all(piece).await.unwrap();
                    rt::time::sleep(Duration::from_millis(1)).await;
                }
            });
            let mut c = TcpStream::connect("127.0.0.1", p).await.unwrap();
            let mut got = Vec::new();
            c.read_to_end(&mut got).await.unwrap();
            server.await.unwrap();
            assert!(got == data, "{} of {} bytes", got.len(), data.len());
        });
    }
}
