#!/usr/bin/env node
// Runs a WASI program the way rootshell (kitknox/rootshell, App Store) runs
// one, for testing BariStore off the device: rootshell's own worker,
// Resources/Wasm/wasm-worker.js (MIT, downloaded at the commit below), in a
// worker thread, answered by stand-ins for rootshell's Swift brokers
// (rootshell/Features/Wasm/WasmFilesystemBroker.swift, WasmSocketBroker.swift,
// WasmUDPSocket.swift) with their rules: one call at a time through one
// 64 KB answer slot, an accept that waits until a connection comes, receive
// time limits answered with EAGAIN, files as FileManager handles them (a
// rename onto a file that exists fails, folders are made one at a time,
// failures are EIO). TLS is Node's here (rootshell's is Network.framework's),
// through $HTTPS_PROXY when one is set. Datagrams to the mDNS group are
// shown and dropped.
//
//   node scripts/rootshell.mjs [--root DIR] [--cwd /virtual/dir] program.wasm [args…]
//
// ROOTSHELL_VERBOSE=1 tells what the stand-ins do; ROOTSHELL_TRACE=1 also
// every file call.
// DIR plays rootshell's Documents folder (HOME=/ inside); default: a new
// temporary one.

import { Worker } from "node:worker_threads";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import net from "node:net";
import tls from "node:tls";
import dgram from "node:dgram";
import dns from "node:dns";
import { randomUUID } from "node:crypto";

const COMMIT = "e160a75d088127f19054c49b81a59fe204f3c3c1";
const WORKER_URL = `https://raw.githubusercontent.com/kitknox/rootshell/${COMMIT}/Resources/Wasm/wasm-worker.js`;

// WasmFilesystemBroker.swift's Errno.
const E = { EACCES: 2, EAGAIN: 6, EBADF: 8, ECONNREFUSED: 14, EEXIST: 20, EINVAL: 28, EIO: 29, ENOENT: 44,
            ENETUNREACH: 50, ENOSYS: 52, ENOTDIR: 54, EPIPE: 64, ETIMEDOUT: 73, EADDRINUSE: 1 };

let args = process.argv.slice(2);
let root = null, cwd = "/";
while (args[0]?.startsWith("--")) {
  const flag = args.shift();
  if (flag === "--root") root = args.shift();
  else if (flag === "--cwd") cwd = args.shift();
  else { console.error(`unknown flag ${flag}`); process.exit(2); }
}
if (!args.length) { console.error("usage: rootshell.mjs [--root DIR] [--cwd DIR] program.wasm [args…]"); process.exit(2); }
root = path.resolve(root || fs.mkdtempSync(path.join(os.tmpdir(), "rootshell-")));
fs.mkdirSync(path.join(root, "tmp"), { recursive: true });
const wasmPath = args.shift();
const trace = !!process.env.ROOTSHELL_TRACE;
const verbose = trace || !!process.env.ROOTSHELL_VERBOSE;
const note = (m) => { if (verbose) process.stderr.write(`[rootshell] ${m}\n`); };

async function workerSource() {
  const cache = path.join(path.dirname(new URL(import.meta.url).pathname), "..", "target", "rootshell", `wasm-worker-${COMMIT.slice(0, 8)}.js`);
  if (fs.existsSync(cache)) return fs.readFileSync(cache, "utf8");
  const r = await fetch(WORKER_URL);
  if (!r.ok) throw new Error(`cannot download rootshell's worker: HTTP ${r.status}`);
  const text = await r.text();
  fs.mkdirSync(path.dirname(cache), { recursive: true });
  fs.writeFileSync(cache, text);
  return text;
}

// ------------------------------------------------------------ the answer slot (wasm-host.js)
const sab = new SharedArrayBuffer(64 * 1024);
const view = new Int32Array(sab);
function deliver(reply) {
  const enc = new TextEncoder().encode(JSON.stringify(reply));
  const bytes = new Uint8Array(sab);
  if (enc.length + 8 > bytes.length) {
    // Too big for the slot: rootshell answers EINVAL, the bytes are lost.
    note(`answer of ${enc.length} bytes does not fit the slot: EINVAL`);
    const fail = new TextEncoder().encode(JSON.stringify({ callID: reply.callID, errno: E.EINVAL }));
    bytes.set(fail, 8);
    Atomics.store(view, 1, fail.length);
  } else {
    bytes.set(enc, 8);
    Atomics.store(view, 1, enc.length);
  }
  Atomics.store(view, 0, 1);
  Atomics.notify(view, 0);
}
const b64 = (buf) => Buffer.from(buf).toString("base64");

// ------------------------------------------------------------ files (WasmFilesystemBroker.swift)
const files = new Map([[3, { url: root, isDir: true, preopen: "/" }]]);
let nextFile = 5;
function resolvePath(p, dirfd) {
  if (p.includes("\0")) return null;
  let full;
  if (p === "~") full = root;
  else if (p.startsWith("~/")) full = path.join(root, p.slice(2));
  else if (p.startsWith("/")) full = path.join(root, p);
  else {
    const d = files.get(dirfd);
    if (!d || !d.isDir) return null;
    full = path.join(d.url, p);
  }
  full = path.resolve(full);
  return full === root || full.startsWith(root + path.sep) ? full : null;
}
function stat(url, hint) {
  try {
    const s = fs.statSync(url);
    const dir = hint ?? s.isDirectory();
    return { errno: 0, size: dir ? 0 : s.size, mtime: Math.floor(s.mtimeMs * 1e6), type: dir ? 3 : 4 };
  } catch { return { errno: E.ENOENT }; }
}
function fsCall(p) {
  const h = files.get(p.fd);
  const io = (f) => { try { return f(); } catch { return { errno: E.EIO }; } };
  switch (p.op) {
    case "path_open": {
      const url = resolvePath(p.path, p.dirfd);
      if (!url) return { errno: E.EACCES };
      const creat = p.oflags & 1, excl = p.oflags & 4, trunc = p.oflags & 8, append = p.fdflags & 1;
      const exists = fs.existsSync(url);
      if (!exists && !creat) return { errno: E.ENOENT };
      if (exists && excl && creat) return { errno: E.EEXIST };
      if (exists && fs.statSync(url).isDirectory()) { const fd = nextFile++; files.set(fd, { url, isDir: true }); return { errno: 0, fd }; }
      return io(() => {
        if (!exists) fs.writeFileSync(url, "");
        if (trunc) fs.truncateSync(url, 0);
        const handle = fs.openSync(url, "r+");
        const fd = nextFile++;
        files.set(fd, { url, isDir: false, handle, pos: append ? fs.fstatSync(handle).size : 0 });
        return { errno: 0, fd };
      });
    }
    case "fd_read":
      if (!h?.handle) return { errno: E.EBADF };
      return io(() => {
        const buf = Buffer.alloc(p.length);
        const n = fs.readSync(h.handle, buf, 0, p.length, h.pos);
        h.pos += n;
        return { errno: 0, data: b64(buf.subarray(0, n)) };
      });
    case "fd_write":
      if (!h?.handle) return { errno: E.EBADF };
      return io(() => {
        const data = Buffer.from(p.data, "base64");
        fs.writeSync(h.handle, data, 0, data.length, h.pos);
        h.pos += data.length;
        return { errno: 0, written: data.length };
      });
    case "fd_pread":
      if (!h?.handle) return { errno: E.EBADF };
      return io(() => {
        const buf = Buffer.alloc(p.length);
        const n = fs.readSync(h.handle, buf, 0, p.length, p.offset);
        return { errno: 0, data: b64(buf.subarray(0, n)) };
      });
    case "fd_pwrite":
      if (!h?.handle) return { errno: E.EBADF };
      return io(() => {
        const data = Buffer.from(p.data, "base64");
        fs.writeSync(h.handle, data, 0, data.length, p.offset);
        return { errno: 0, written: data.length };
      });
    case "fd_close":
      if (p.fd <= 4) return { errno: 0 };
      if (h?.handle != null) fs.closeSync(h.handle);
      files.delete(p.fd);
      return { errno: 0 };
    case "fd_seek": {
      if (!h?.handle) return { errno: E.EBADF };
      const end = fs.fstatSync(h.handle).size;
      const base = p.whence === 0 ? 0 : p.whence === 1 ? h.pos : p.whence === 2 ? end : null;
      if (base == null) return { errno: E.EINVAL };
      h.pos = Math.max(0, base + p.offset);
      return { errno: 0, offset: h.pos };
    }
    case "fd_filestat_get": return h ? stat(h.url, h.isDir) : { errno: E.EBADF };
    case "path_filestat_get": { const url = resolvePath(p.path, p.dirfd); return url ? stat(url) : { errno: E.EACCES }; }
    case "path_create_directory": {
      const url = resolvePath(p.path, p.dirfd);
      if (!url) return { errno: E.EACCES };
      return io(() => { fs.mkdirSync(url); return { errno: 0 }; });
    }
    case "path_remove_directory":
    case "path_unlink_file": {
      const url = resolvePath(p.path, p.dirfd);
      if (!url) return { errno: E.EACCES };
      return io(() => { if (!fs.existsSync(url)) throw new Error("missing"); fs.rmSync(url, { recursive: true }); return { errno: 0 }; });
    }
    case "path_rename": {
      const from = resolvePath(p.oldPath, p.oldDirfd), to = resolvePath(p.newPath, p.newDirfd ?? p.oldDirfd);
      if (!from || !to) return { errno: E.EACCES };
      // FileManager.moveItem refuses a destination that exists.
      return io(() => { if (fs.existsSync(to)) throw new Error("exists"); fs.renameSync(from, to); return { errno: 0 }; });
    }
    case "fd_readdir": {
      if (!h?.isDir) return { errno: E.ENOTDIR };
      if (p.cookie === 0 || !h.entries) {
        h.entries = fs.readdirSync(h.url, { withFileTypes: true }).map((d) => ({ name: d.name, isDir: d.isDirectory() }));
      }
      const budget = Math.min(p.bufLen, 8192);
      const page = [];
      let used = 0;
      for (let i = p.cookie; i < h.entries.length && (used < budget || !page.length); i++) {
        page.push(h.entries[i]);
        used += 24 + Buffer.byteLength(h.entries[i].name);
      }
      return { errno: 0, entries: page };
    }
    case "fd_prestat_get": return h?.preopen ? { errno: 0, type: 0, nameLen: Buffer.byteLength(h.preopen) } : { errno: E.EBADF };
    case "fd_prestat_dir_name": return h?.preopen ? { errno: 0, name: h.preopen } : { errno: E.EBADF };
    case "fd_filestat_set_size":
      if (!h?.handle) return { errno: E.EBADF };
      return io(() => { fs.ftruncateSync(h.handle, p.size); return { errno: 0 }; });
    case "fd_filestat_set_times":
    case "path_filestat_set_times": {
      const url = p.op === "fd_filestat_set_times" ? h?.url : resolvePath(p.path, p.dirfd);
      if (!url) return { errno: p.op === "fd_filestat_set_times" ? E.EBADF : E.EACCES };
      if (!(p.flags & 0x0c)) return { errno: 0 };
      const m = p.flags & 0x04 ? new Date(p.mtim / 1e6) : new Date();
      return io(() => { fs.utimesSync(url, m, m); return { errno: 0 }; });
    }
    case "fd_sync":
      if (!h?.handle) return { errno: E.EBADF };
      return io(() => { fs.fsyncSync(h.handle); return { errno: 0 }; });
    default: return { errno: E.ENOSYS };
  }
}

// ------------------------------------------------------------ sockets (WasmSocketBroker.swift)
const sockets = new Map();
let nextSocket = 100;
let multicast = 0;
const local = (host) => /^(127\.|localhost$|0\.0\.0\.0$|::1$)/.test(host);
const proxy = process.env.HTTPS_PROXY || process.env.https_proxy;

function connectErrno(e) {
  switch (e?.code) {
    case "ECONNREFUSED": return E.ECONNREFUSED;
    case "ETIMEDOUT": return E.ETIMEDOUT;
    case "ENETUNREACH": case "EHOSTUNREACH": case "ENETDOWN": return E.ENETUNREACH;
    case "ENOTFOUND": case "EAI_AGAIN": return E.EIO;
    default: return e?.tls ? E.ECONNREFUSED : E.EIO;
  }
}

/** A TCP connection to host:port, through the proxy for hosts not on this machine. */
function tcp(host, port) {
  return new Promise((resolve, reject) => {
    if (!proxy || local(host)) {
      const s = net.connect({ host, port });
      s.once("connect", () => resolve(s));
      s.once("error", reject);
      return;
    }
    const u = new URL(proxy);
    const s = net.connect({ host: u.hostname, port: Number(u.port) });
    s.once("error", reject);
    s.once("connect", () => s.write(`CONNECT ${host}:${port} HTTP/1.1\r\nHost: ${host}:${port}\r\n\r\n`));
    let head = Buffer.alloc(0);
    const onData = (d) => {
      head = Buffer.concat([head, d]);
      const end = head.indexOf("\r\n\r\n");
      if (end < 0) return;
      s.off("data", onData);
      const status = head.subarray(0, head.indexOf("\r\n")).toString();
      if (!/^HTTP\/1\.[01] 200/.test(status)) { s.destroy(); reject(Object.assign(new Error(status), { code: "ECONNREFUSED" })); return; }
      if (head.length > end + 4) s.unshift(head.subarray(end + 4));
      resolve(s);
    };
    s.on("data", onData);
  });
}

function startReading(fd, h, conn) {
  h.conn = conn;
  conn.on("data", (d) => { h.reads = Buffer.concat([h.reads, d]); flush(h); });
  conn.on("end", () => { h.eof = true; flush(h); });
  conn.on("error", () => { h.failed = true; flush(h); });
  conn.on("close", () => { h.eof = true; flush(h); });
}

function flush(h) {
  if (h.kind === "udp") {
    while (h.waiters.length && h.datagrams.length) {
      const w = h.waiters.shift();
      clearTimeout(w.timer);
      const d = h.datagrams.shift();
      w.reply({ errno: 0, data: b64(d.data.subarray(0, w.maxLen)), peerHost: d.host, peerPort: d.port });
    }
    return;
  }
  while (h.waiters.length && h.reads.length) {
    const w = h.waiters.shift();
    clearTimeout(w.timer);
    const take = h.reads.subarray(0, w.maxLen);
    h.reads = h.reads.subarray(take.length);
    w.reply({ errno: 0, data: b64(take) });
  }
  if (h.failed) {
    for (const w of h.waiters.splice(0)) { clearTimeout(w.timer); w.reply({ errno: E.EIO }); }
  } else if (h.eof && !h.reads.length) {
    for (const w of h.waiters.splice(0)) { clearTimeout(w.timer); w.reply({ errno: 0, data: "" }); }
  }
}

function socketCall(p, reply) {
  const h = sockets.get(p.fd);
  switch (p.op) {
    case "socket": {
      const fd = nextSocket++;
      const s = { kind: p.type === 2 ? "udp" : "tcp", reads: Buffer.alloc(0), waiters: [], datagrams: [], accepts: [],
                  acceptWaiters: [], timeout: 0, port: 0 };
      if (s.kind === "udp") {
        s.udp = dgram.createSocket({ type: "udp4", reuseAddr: true });
        s.udp.on("message", (data, rinfo) => { s.datagrams.push({ data, host: rinfo.address, port: rinfo.port }); flush(s); });
        // Like BSD so_error: an ICMP refusal is the next receive's answer.
        s.udp.on("error", (e) => {
          if (e.code !== "ECONNREFUSED") return;
          const w = s.waiters.shift();
          if (w) { clearTimeout(w.timer); w.reply({ errno: E.ECONNREFUSED }); } else s.pendingError = E.ECONNREFUSED;
        });
      }
      sockets.set(fd, s);
      return reply({ errno: 0, fd });
    }
    case "bind":
      if (!h) return reply({ errno: E.EBADF });
      if (h.kind === "udp") {
        h.udp.once("error", () => reply({ errno: E.EADDRINUSE }));
        h.udp.bind(p.port, () => reply({ errno: 0 }));
        return;
      }
      h.port = p.port;
      return reply({ errno: 0 });
    case "listen": {
      if (!h || h.kind !== "tcp") return reply({ errno: h ? E.EINVAL : E.EBADF });
      h.kind = "listener";
      h.server = net.createServer({ allowHalfOpen: true }, (conn) => {
        h.accepts.push(conn);
        while (h.acceptWaiters.length && h.accepts.length) h.acceptWaiters.shift()(h.accepts.shift());
      });
      // NWListener says nothing of a port in use: rootshell answers 0 either way.
      h.server.on("error", (e) => note(`listen on ${h.port}: ${e.message}`));
      h.server.listen(h.port, "0.0.0.0");
      return reply({ errno: 0 });
    }
    case "accept": {
      if (!h || h.kind !== "listener") return reply({ errno: E.EBADF });
      const install = (conn) => {
        const fd = nextSocket++;
        const s = { kind: "tcp", reads: Buffer.alloc(0), waiters: [], timeout: 0 };
        sockets.set(fd, s);
        startReading(fd, s, conn);
        reply({ errno: 0, fd, peerHost: conn.remoteAddress, peerPort: conn.remotePort });
      };
      if (h.accepts.length) return install(h.accepts.shift());
      h.acceptWaiters.push(install);
      return;
    }
    case "connect":
    case "tls_connect": {
      if (!h) return reply({ errno: E.EBADF });
      if (h.kind === "udp") {
        h.udp.once("connect", () => reply({ errno: 0 }));
        h.udp.connect(p.port, p.host);
        return;
      }
      tcp(p.host, p.port).then((raw) => {
        if (p.op === "connect") { startReading(p.fd, h, raw); reply({ errno: 0 }); return; }
        const t = tls.connect({ socket: raw, servername: p.host, ALPNProtocols: ["http/1.1"] });
        t.once("secureConnect", () => { startReading(p.fd, h, t); reply({ errno: 0 }); });
        t.once("error", (e) => { note(`tls ${p.host}: ${e.message}`); reply({ errno: E.ECONNREFUSED }); });
      }, (e) => { note(`connect ${p.host}:${p.port}: ${e.message}`); reply({ errno: connectErrno(e) }); });
      return;
    }
    case "send": {
      if (!h) return reply({ errno: E.EBADF });
      const data = Buffer.from(p.data, "base64");
      if (h.kind === "udp") { h.udp.send(data, (e) => reply(e ? { errno: E.EIO } : { errno: 0, sent: data.length })); return; }
      if (!h.conn || h.conn.destroyed) return reply({ errno: E.EIO });
      h.conn.write(data, (e) => reply(e ? { errno: E.EIO } : { errno: 0, sent: data.length }));
      return;
    }
    case "sendto": {
      if (!h || h.kind !== "udp") return reply({ errno: E.EINVAL });
      const data = Buffer.from(p.data, "base64");
      if (p.host === "224.0.0.251") {
        multicast++;
        note(`mDNS datagram ${multicast} (${data.length} bytes) dropped`);
        return reply({ errno: 0, sent: data.length });
      }
      h.udp.send(data, p.port, p.host, (e) => reply(e ? { errno: E.EIO } : { errno: 0, sent: data.length }));
      return;
    }
    case "recv":
    case "recvfrom": {
      if (!h) return reply({ errno: E.EBADF });
      if (h.kind === "udp" && h.pendingError) {
        const errno = h.pendingError;
        h.pendingError = 0;
        return reply({ errno });
      }
      if (h.kind === "udp" && h.datagrams.length) {
        const d = h.datagrams.shift();
        return reply({ errno: 0, data: b64(d.data.subarray(0, p.maxLen)), peerHost: d.host, peerPort: d.port });
      }
      if (h.kind !== "udp") {
        if (h.reads.length) {
          const take = h.reads.subarray(0, p.maxLen);
          h.reads = h.reads.subarray(take.length);
          return reply({ errno: 0, data: b64(take) });
        }
        if (h.failed) return reply({ errno: E.EIO });
        if (h.eof) return reply({ errno: 0, data: "" });
      }
      const w = { maxLen: p.maxLen, reply };
      h.waiters.push(w);
      if (h.timeout) {
        w.timer = setTimeout(() => {
          const i = h.waiters.indexOf(w);
          if (i >= 0) { h.waiters.splice(i, 1); reply({ errno: E.EAGAIN }); }
        }, h.timeout);
      }
      return;
    }
    case "set_recv_timeout":
      if (!h) return reply({ errno: E.EBADF });
      h.timeout = p.timeoutMs;
      return reply({ errno: 0 });
    case "shutdown":
      if (!h) return reply({ errno: E.EBADF });
      h.conn?.destroy();
      return reply({ errno: 0 });
    case "close":
      if (!h) return reply({ errno: E.EBADF });
      h.conn?.destroy();
      h.server?.close();
      try { h.udp?.close(); } catch {}
      for (const w of h.waiters ?? []) { clearTimeout(w.timer); w.reply({ errno: E.EBADF }); }
      sockets.delete(p.fd);
      return reply({ errno: 0 });
    case "getsockname": return reply(h ? { errno: 0, host: "0.0.0.0", port: h.port || 0 } : { errno: E.EBADF });
    case "getpeername": return reply(h ? { errno: 0, host: h.conn?.remoteAddress || "", port: h.conn?.remotePort || 0 } : { errno: E.EBADF });
    case "resolve":
      dns.lookup(p.host, { all: true, family: p.family === "v6" ? 6 : 4 }, (e, list) => {
        if (e || !list.length) return reply({ errno: E.ENOENT });
        reply({ errno: 0, addrs: list.slice(0, p.maxResults).map((a) => b64(p.family === "v6" ? [] : a.address.split(".").map(Number))) });
      });
      return;
    case "bonjour_lookup":
      // No Bonjour here: nothing is found in the time given.
      setTimeout(() => reply({ errno: E.ETIMEDOUT, text: "" }), Math.min(p.timeoutMs, 60000));
      return;
    default: return reply({ errno: E.ENOSYS });
  }
}

// ------------------------------------------------------------ the process
const source = await workerSource();
const bootstrap = `
const { parentPort } = require("node:worker_threads");
globalThis.self = globalThis;
self.postMessage = (m) => parentPort.postMessage(m);
parentPort.on("message", (m) => { if (self.onmessage) self.onmessage({ data: m }); });
${source}`;
const worker = new Worker(bootstrap, { eval: true, resourceLimits: { maxOldGenerationSizeMb: 4096 } });
const stdinQueue = [];
let stdinWaiter = null;
process.stdin.on("data", (d) => {
  stdinQueue.push(d);
  if (stdinWaiter) { const w = stdinWaiter; stdinWaiter = null; serveStdin(w); }
});
process.stdin.on("end", () => { stdinQueue.push(null); if (stdinWaiter) { const w = stdinWaiter; stdinWaiter = null; serveStdin(w); } });
function serveStdin(msg) {
  if (!stdinQueue.length) { stdinWaiter = msg; return; }
  const head = stdinQueue[0];
  if (head === null) return deliver({ callID: msg.callID, errno: 0, data: "" });
  const take = head.subarray(0, msg.maxLen);
  if (take.length === head.length) stdinQueue.shift(); else stdinQueue[0] = head.subarray(take.length);
  deliver({ callID: msg.callID, errno: 0, data: b64(take) });
}
worker.on("message", (msg) => {
  switch (msg.kind) {
    case "stdout": process.stdout.write(Buffer.from(msg.data, "base64")); break;
    case "stderr": process.stderr.write(Buffer.from(msg.data, "base64")); break;
    case "fs": {
      const r = fsCall(msg.payload);
      if (trace) note(`fs ${msg.payload.op} ${msg.payload.path ?? msg.payload.oldPath ?? msg.payload.fd ?? ""} -> ${r.errno}`);
      deliver({ callID: msg.callID, ...r });
      break;
    }
    case "socket": socketCall(msg.payload, (r) => deliver({ callID: msg.callID, ...r })); break;
    case "terminal": deliver({ callID: msg.callID, errno: 0 }); break;
    case "stdinRead": serveStdin(msg); break;
    case "log": note(msg.message); break;
    case "exit": note(`exit ${msg.code}`); process.exit(msg.code); break;
    case "error": process.stderr.write(`wasm: ${msg.message}\n`); process.exit(1); break;
  }
});
worker.on("error", (e) => { process.stderr.write(`wasm: worker error: ${e.message}\n`); process.exit(1); });
note(`Documents: ${root}`);
const env = { HOME: "/", PWD: cwd, USER: "mobile", PATH: process.env.PATH || "", COLUMNS: "80", LINES: "24" };
for (const k of ["BARISTORE_ECHO", "BARISTORE_DEVICE"]) if (process.env[k]) env[k] = process.env[k];
worker.postMessage({ kind: "start", processID: randomUUID(), wasm: fs.readFileSync(wasmPath).toString("base64"),
                     argv: [path.basename(wasmPath), ...args], env, cwd: path.join(root, cwd), sandboxRoot: root, sab });
