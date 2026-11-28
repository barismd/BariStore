//! Files, in pieces rootshell can carry, and rootshell's file broker's ways.
//!
//! rootshell answers a read through its 64 KB answer slot as base64 in JSON,
//! and a read that brings more fails with EINVAL, its bytes gone
//! (wasm-host.js `writeReplyAndNotify`); each write's bytes become base64 a
//! character at a time (wasm-worker.js `bytesToB64`). std and the crates
//! read whole files into one buffer (`std::fs::read`) and write them out
//! from one. So the program has its own `read`, `readv`, `pread`, `write`,
//! `writev` and `pwrite`, which std and wasi-libc link to in place of
//! wasi-libc's (checked with Rust 1.97: std's file reads go through libc's
//! `read`): each reads 32 KB and writes 256 KB at most, a short read or write
//! every caller takes. (atelier-install found and solved the same.)
//!
//! rootshell's broker renames with FileManager, which refuses to replace a
//! file that exists (EIO): `replace` makes room first. It makes one folder
//! at a time and answers EIO, not EEXIST, for one that exists, which std's
//! `create_dir_all` takes for a failure (Rust 1.97): `make_dirs` looks
//! first.

/// `create_dir_all` as rootshell's broker allows it.
pub fn make_dirs(path: &std::path::Path) -> std::io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        make_dirs(parent)?;
    }
    match std::fs::create_dir(path) {
        Err(_) if path.is_dir() => Ok(()),
        other => other,
    }
}

/// Renames `from` to `to`, replacing `to` when it exists.
pub fn replace(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) if to.exists() => {
            std::fs::remove_file(to)?;
            std::fs::rename(from, to)
        }
        Err(e) => Err(e),
    }
}

#[cfg(target_arch = "wasm32")]
mod chunked {
    use std::ffi::{c_int, c_void};

    use crate::rootshell::{MOST_RECEIVED, MOST_SENT};

    #[repr(C)]
    pub struct Iovec {
        buf: *mut u8,
        len: usize,
    }

    extern "C" {
        // wasi-libc's wrappers of WASI's own calls.
        fn __wasi_fd_read(fd: u32, iovs: *const Iovec, iovs_len: usize, read: *mut usize) -> u16;
        fn __wasi_fd_write(fd: u32, iovs: *const Iovec, iovs_len: usize, written: *mut usize) -> u16;
        fn __wasi_fd_pread(fd: u32, iovs: *const Iovec, iovs_len: usize, offset: u64, read: *mut usize) -> u16;
        fn __wasi_fd_pwrite(fd: u32, iovs: *const Iovec, iovs_len: usize, offset: u64, written: *mut usize) -> u16;
        fn __errno_location() -> *mut c_int;
    }

    /// As wasi-libc's calls end: -1 and errno (ENOTCAPABLE as EBADF).
    fn answer(error: u16, count: usize) -> isize {
        if error != 0 {
            // SAFETY: wasi-libc's errno cell.
            unsafe { *__errno_location() = if error == 76 { 8 } else { c_int::from(error) } };
            return -1;
        }
        count as isize
    }

    /// The first non-empty piece of an iovec list.
    unsafe fn first(iov: *const Iovec, count: c_int) -> Option<(*mut u8, usize)> {
        // SAFETY: the caller's list of `count` entries.
        let list = unsafe { std::slice::from_raw_parts(iov, count.max(0) as usize) };
        list.iter().find(|v| v.len > 0).map(|v| (v.buf, v.len))
    }

    #[no_mangle]
    pub unsafe extern "C" fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize {
        let iov = Iovec { buf: buf.cast(), len: count.min(MOST_RECEIVED) };
        let mut n = 0;
        // SAFETY: one iovec over the caller's buffer.
        answer(unsafe { __wasi_fd_read(fd as u32, &iov, 1, &mut n) }, n)
    }

    #[no_mangle]
    pub unsafe extern "C" fn readv(fd: c_int, iov: *const Iovec, count: c_int) -> isize {
        // SAFETY: the caller's iovecs.
        match unsafe { first(iov, count) } {
            // SAFETY: the piece is the caller's buffer.
            Some((buf, len)) => unsafe { read(fd, buf.cast(), len) },
            None => 0,
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn pread(fd: c_int, buf: *mut c_void, count: usize, offset: i64) -> isize {
        let iov = Iovec { buf: buf.cast(), len: count.min(MOST_RECEIVED) };
        let mut n = 0;
        // SAFETY: as `read`.
        answer(unsafe { __wasi_fd_pread(fd as u32, &iov, 1, offset as u64, &mut n) }, n)
    }

    #[no_mangle]
    pub unsafe extern "C" fn write(fd: c_int, buf: *const c_void, count: usize) -> isize {
        let iov = Iovec { buf: buf as *mut u8, len: count.min(MOST_SENT) };
        let mut n = 0;
        // SAFETY: one iovec over the caller's bytes.
        answer(unsafe { __wasi_fd_write(fd as u32, &iov, 1, &mut n) }, n)
    }

    #[no_mangle]
    pub unsafe extern "C" fn writev(fd: c_int, iov: *const Iovec, count: c_int) -> isize {
        // SAFETY: the caller's iovecs.
        match unsafe { first(iov, count) } {
            // SAFETY: the piece is the caller's bytes.
            Some((buf, len)) => unsafe { write(fd, buf as *const c_void, len) },
            None => 0,
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn pwrite(fd: c_int, buf: *const c_void, count: usize, offset: i64) -> isize {
        let iov = Iovec { buf: buf as *mut u8, len: count.min(MOST_SENT) };
        let mut n = 0;
        // SAFETY: as `write`.
        answer(unsafe { __wasi_fd_pwrite(fd as u32, &iov, 1, offset as u64, &mut n) }, n)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn makes_nested_folders_and_takes_one_that_exists() {
        let dir = crate::state::scratch("dirs");
        let deep = dir.join("a").join("b");
        super::make_dirs(&deep).unwrap();
        super::make_dirs(&deep).unwrap();
        assert!(deep.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replaces_a_file_that_exists() {
        let dir = crate::state::scratch("replace");
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::write(&a, "new").unwrap();
        std::fs::write(&b, "old").unwrap();
        super::replace(&a, &b).unwrap();
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "new");
        assert!(!a.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
