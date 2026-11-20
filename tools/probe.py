#!/usr/bin/env python3
# =============================================================================
#  BariStore device probe (MIT)
#
#  Facts we need from the real iPhone/iPad before writing product code.
#  Run through tools/probe.sh (one curl | sh line). Never reads stdin: every
#  step is driven by time or by taps on a page, so it works under a pipe.
#
#  Parts (default: all, in this order):
#    env        Python, cryptography, TLS store, writable folders, commands
#    net        Wi-Fi/VPN addresses, interfaces, DNS/TCP/TLS to the hosts we use,
#               LocalDevVPN loopback ports
#    mdns       what raw mDNS sockets may do (bind 5353, multicast, broadcast)
#    keepalive  does a-Shell keep running in Settings? (none / page audio / play)
#    pair       advertise a "Pair with BariStore Probe" entry; does iOS dial in?
#
#  Nothing here signs in, pairs for real, installs or deletes anything.
#  The report never contains Apple ID data (none is asked for).
# =============================================================================

import json
import os
import platform
import random
import secrets
import shutil
import socket
import ssl
import struct
import sys
import threading
import time
import uuid

PROBE_VERSION = "5"
DIR = os.path.dirname(os.path.abspath(__file__))
REPORT = os.path.join(DIR, "report.txt")
PARTS = ("env", "net", "mdns", "keepalive", "pair")

_lines = []
_t0 = time.time()


def out(text=""):
    line = str(text)
    print(line, flush=True)
    _lines.append(line)
    try:
        with open(REPORT, "a") as handle:
            handle.write(line + "\n")
    except OSError:
        pass


def say(text=""):
    """Instructions for the user: shown, not stored in the report."""
    print(text, flush=True)


def kv(key, value):
    out("  %-22s %s" % (key, value))


def header(title):
    out("")
    out("== %s ==" % title)


def guarded(name, func):
    try:
        func()
    except Exception as exc:                                    # noqa: BLE001
        out("  !! %s crashed: %s: %s" % (name, type(exc).__name__, exc))


def countdown(seconds, text):
    for left in range(seconds, 0, -1):
        print("\r  %s in %2d s " % (text, left), end="", flush=True)
        time.sleep(1)
    print("\r  %s NOW.%s" % (text, " " * 12), flush=True)


# --------------------------------------------------------------------------
# env
# --------------------------------------------------------------------------

def _writable(path):
    if not os.path.isdir(path):
        return "missing"
    try:
        probe = os.path.join(path, ".baristore_probe_%d" % os.getpid())
        with open(probe, "w") as handle:
            handle.write("x")
        os.remove(probe)
        return "writable"
    except OSError as exc:
        return "NO (%s)" % (exc.strerror or exc)


def part_env():
    header("env")
    kv("probe", PROBE_VERSION)
    kv("python", sys.version.replace("\n", " "))
    kv("sys.platform", sys.platform)
    kv("platform", platform.platform())
    try:
        uname = os.uname()
        kv("machine", uname.machine)
        kv("kernel", "%s %s" % (uname.sysname, uname.release))
    except Exception as exc:                                    # noqa: BLE001
        kv("uname", "error %s" % exc)
    ios_ver = getattr(platform, "ios_ver", None)
    if ios_ver:
        try:
            info = ios_ver()
            kv("ios_ver", "%s %s model=%s simulator=%s" % (
                info.system, info.release, info.model, info.is_simulator))
        except Exception as exc:                                # noqa: BLE001
            kv("ios_ver", "error %s" % exc)
    kv("TERM_PROGRAM", os.environ.get("TERM_PROGRAM", "-"))
    kv("HOME", os.environ.get("HOME", "-"))
    kv("cwd", os.getcwd())
    try:
        import cryptography
        kv("cryptography", cryptography.__version__)
        from cryptography.hazmat.primitives.asymmetric import ed25519, x25519
        from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
        ed25519.Ed25519PrivateKey.generate()
        x25519.X25519PrivateKey.generate()
        ChaCha20Poly1305(os.urandom(32))
        kv("crypto primitives", "ed25519 x25519 chacha20poly1305 ok")
    except Exception as exc:                                    # noqa: BLE001
        kv("cryptography", "ERROR %s: %s" % (type(exc).__name__, exc))
    kv("openssl", ssl.OPENSSL_VERSION)
    try:
        kv("default CA store", ssl.create_default_context().cert_store_stats())
        paths = ssl.get_default_verify_paths()
        kv("verify paths", "cafile=%s capath=%s" % (paths.cafile, paths.capath))
    except Exception as exc:                                    # noqa: BLE001
        kv("default CA store", "error %s" % exc)
    try:
        import certifi
        kv("certifi", "installed %s" % certifi.__version__)
    except ImportError:
        kv("certifi", "not installed")
    home = os.path.expanduser("~")
    for label, path in (("~", home), ("~/Documents", os.path.join(home, "Documents")),
                        ("~/Library", os.path.join(home, "Library")),
                        ("~/Library/Caches", os.path.join(home, "Library", "Caches")),
                        ("~/tmp", os.path.join(home, "tmp"))):
        kv("write " + label, _writable(path))
    found = []
    for name in ("play", "say", "openurl", "pbcopy", "pbpaste", "isForeground",
                 "view", "curl", "pip", "wasm", "jsc"):
        found.append("%s=%s" % (name, "yes" if shutil.which(name) else "no"))
    kv("which", " ".join(found))
    kv("isForeground now", _is_foreground())


def _is_foreground():
    """a-Shell's own command: exit 0 when this window is in front."""
    try:
        code = os.system("isForeground")
    except Exception as exc:                                    # noqa: BLE001
        return "error %s" % exc
    return "yes" if code == 0 else "no (exit %s)" % code


# --------------------------------------------------------------------------
# net
# --------------------------------------------------------------------------

def _route_ip(target):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.connect((target, 9))
        return sock.getsockname()[0]
    except OSError as exc:
        return "none (%s)" % (exc.strerror or exc)
    finally:
        sock.close()


def _tcp(host, port, timeout=4.0):
    start = time.time()
    try:
        sock = socket.create_connection((host, port), timeout)
        sock.close()
        return "open %.0f ms" % ((time.time() - start) * 1000)
    except OSError as exc:
        return "closed (%s)" % (exc.strerror or type(exc).__name__)


def _tls(host):
    start = time.time()
    try:
        context = ssl.create_default_context()
        with socket.create_connection((host, 443), 6) as raw:
            with context.wrap_socket(raw, server_hostname=host) as tls:
                return "verified %s %.0f ms" % (tls.version(), (time.time() - start) * 1000)
    except ssl.SSLCertVerificationError as exc:
        return "handshake ok, verify FAILED (%s)" % exc.verify_message
    except Exception as exc:                                    # noqa: BLE001
        return "error %s: %s" % (type(exc).__name__, exc)


def part_net():
    header("net")
    kv("route to internet", _route_ip("8.8.8.8"))
    kv("route to 10.7.0.1", _route_ip("10.7.0.1"))
    try:
        names = [name for _i, name in socket.if_nameindex()]
        kv("interfaces", " ".join(names))
    except Exception as exc:                                    # noqa: BLE001
        kv("interfaces", "error %s" % exc)
    for host in ("gsa.apple.com", "developerservices2.apple.com", "mac-ani.jkcoxson.com",
                 "ani.sidestore.io", "api.github.com", "raw.githubusercontent.com"):
        try:
            address = socket.getaddrinfo(host, 443, socket.AF_INET)[0][4][0]
        except OSError as exc:
            kv(host, "DNS failed (%s)" % exc)
            continue
        kv(host, "%s tcp %s" % (address, _tcp(host, 443)))
    kv("TLS default store", "gsa.apple.com: %s" % _tls("gsa.apple.com"))

    results = {}

    def check(port):
        results[port] = _tcp("10.7.0.1", port, 1.5).startswith("open")

    threads = [threading.Thread(target=check, args=(port,)) for port in range(49152, 49216)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    open_ports = sorted(port for port, ok in results.items() if ok)
    kv("10.7.0.1 open ports", open_ports or "none (LocalDevVPN off, or nothing listens)")
    for port in open_ports[:3]:
        kv("10.7.0.1:%d first bytes" % port, _peek("10.7.0.1", port))


def _peek(host, port):
    try:
        sock = socket.create_connection((host, port), 2)
        sock.settimeout(2)
        try:
            data = sock.recv(64)
        except socket.timeout:
            data = b""
        sock.close()
        return data[:32].hex() if data else "silent (waits for the client)"
    except OSError as exc:
        return "error %s" % exc


# --------------------------------------------------------------------------
# mdns
# --------------------------------------------------------------------------

MCAST = "224.0.0.251"


def _subnet_broadcast(ipv4):
    parts = ipv4.split(".")
    if len(parts) != 4 or not all(part.isdigit() for part in parts):
        return ""
    return "%s.%s.%s.255" % tuple(parts[:3])


def _query_packet():
    """A harmless PTR query for a name nobody owns."""
    name = b"".join(bytes([len(label)]) + label for label in
                    (b"_baristoreprobe", b"_tcp", b"local")) + b"\x00"
    return struct.pack(">HHHHHH", 0, 0, 1, 0, 0, 0) + name + struct.pack(">HH", 12, 1)


def part_mdns():
    header("mdns")
    wifi = _route_ip("8.8.8.8")
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        kv("SO_REUSEPORT", "ok")
    except (OSError, AttributeError) as exc:
        kv("SO_REUSEPORT", "no (%s)" % exc)
    try:
        sock.bind(("", 5353))
        kv("bind :5353", "ok")
        bound = True
    except OSError as exc:
        kv("bind :5353", "no (%s)" % (exc.strerror or exc))
        bound = False
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        kv("SO_BROADCAST", "ok")
    except OSError as exc:
        kv("SO_BROADCAST", "no (%s)" % exc)
    if bound:
        try:
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP,
                            socket.inet_aton(MCAST) + socket.inet_aton(wifi))
            kv("join 224.0.0.251", "ok")
        except OSError as exc:
            kv("join 224.0.0.251", "no (%s)" % (exc.strerror or exc))
    packet = _query_packet()
    for label, address in (("send multicast", MCAST), ("send 255.255.255.255", "255.255.255.255"),
                           ("send subnet bcast", _subnet_broadcast(wifi))):
        if not address:
            kv(label, "skipped (no Wi-Fi IPv4)")
            continue
        try:
            sock.sendto(packet, (address, 5353))
            kv(label, "ok -> %s" % address)
        except OSError as exc:
            kv(label, "no (%s)" % (exc.strerror or exc))
    if bound:
        sock.settimeout(0.5)
        seen = 0
        deadline = time.time() + 3
        while time.time() < deadline:
            try:
                sock.recvfrom(9000)
                seen += 1
            except (socket.timeout, OSError):
                pass
        kv("mDNS packets heard 3s", seen)
    sock.close()


# --------------------------------------------------------------------------
# keepalive
# --------------------------------------------------------------------------

class Heartbeat:
    """Ticks every 0.25 s; a pause longer than 3 s means iOS suspended us.

    (Probe 1 also polled a-Shell's `isForeground`: on device it never reported
    the background, so the tests ran to a 120 s cap. Timing is all we use now.)
    """

    def __init__(self):
        self.ticks = []
        self.stop = threading.Event()
        self.thread = None
        self.start = 0.0

    def _tick(self):
        while not self.stop.is_set():
            self.ticks.append(time.time())
            time.sleep(0.25)

    def begin(self):
        self.start = time.time()
        self.thread = threading.Thread(target=self._tick, daemon=True)
        self.thread.start()
        return self

    def end(self):
        self.stop.set()
        if self.thread is not None:
            self.thread.join(timeout=3)
        return self.summary()

    def summary(self):
        gaps = []
        previous = self.start
        for tick in self.ticks:
            if tick - previous > 3:
                gaps.append((round(previous - self.start, 1), round(tick - previous, 1)))
            previous = tick
        return {"seconds": round(time.time() - self.start, 1), "ticks": len(self.ticks),
                "max_gap": max([g for _s, g in gaps] or [0.0]), "gaps": gaps[:8]}


def _speak(text):
    """`say` plays only while we run: hearing it in Settings proves we are alive."""
    try:
        return os.system('say "%s"' % text)
    except Exception as exc:                                    # noqa: BLE001
        return "error %s" % exc


def _report_heartbeat(label, result):
    if result["max_gap"] > 3:
        verdict = "SUSPENDED (longest pause %.1f s)" % result["max_gap"]
    else:
        verdict = "KEPT RUNNING (no pause; valid only if you really left a-Shell)"
    kv(label, verdict)
    kv("  details", json.dumps(result))


def _trip(label, extra=()):
    """One Settings trip: 8 s to get ready, then 40 s away, a spoken cue to return."""
    say("")
    say("  ==== %s ====" % label)
    for line in extra:
        say("  " + line)
    say("  When it says NOW: swipe to the Home Screen, open Settings and wait there.")
    say("  Come back to a-Shell when you HEAR 'come back', or after ~40 s if you hear nothing.")
    countdown(8, "leave a-Shell")
    hb = Heartbeat().begin()
    time.sleep(38)
    code = _speak("BariStore probe. Come back now.")
    time.sleep(4)
    result = hb.end()
    result["say_exit"] = code
    return result


def _after_trip(result):
    # Suspended: we only get here once the user is back. Alive: give time to return.
    wait = 5 if result["max_gap"] > 3 else 20
    say("  Back in a-Shell? Next step in %d s." % wait)
    countdown(wait, "next step")


def keepalive_none():
    result = _trip("Test 1/3: no keep-alive")
    _report_heartbeat("none", result)
    _after_trip(result)


PAGE = """<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>BariStore probe</title>
<style>body{font:18px -apple-system,sans-serif;margin:24px;background:#0A1B4D;color:#fff}
button{font-size:22px;padding:16px 24px;border-radius:14px;border:0;background:#fff;color:#15337F}
#msg{margin-top:24px;font-size:24px;line-height:1.4;font-weight:600}</style></head><body>
<h2>BariStore probe: test 2/3</h2>
<p>This page plays silence, like the old BariSign page did.</p>
<button id="go">1. Tap here</button>
<div id="msg">Then follow the steps that appear here.</div>
<script>
const T = location.pathname.replace(/\\/$/, "");
async function startAudio(){
  try{
    const AC = window.AudioContext || window.webkitAudioContext;
    window.__ac = window.__ac || new AC();
    if (window.__ac.state === "suspended") await window.__ac.resume();
    const len = window.__ac.sampleRate;
    const buf = window.__ac.createBuffer(1, len, window.__ac.sampleRate);
    const src = window.__ac.createBufferSource();
    src.buffer = buf; src.loop = true; src.connect(window.__ac.destination); src.start();
    document.getElementById("go").style.display = "none";
    await fetch(T + "/ok", {method: "POST", body: window.__ac.state});
  }catch(e){ await fetch(T + "/ok", {method: "POST", body: "error " + e}); }
}
document.getElementById("go").onclick = startAudio;
setInterval(async () => {
  try{
    await fetch(T + "/beat", {method: "POST", body: window.__ac ? window.__ac.state : "none"});
    const r = await fetch(T + "/msg"); document.getElementById("msg").textContent = await r.text();
  }catch(e){}
}, 1000);
</script></body></html>"""


def keepalive_page():
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

    token = "/" + secrets.token_hex(16)
    state = {"ok": None, "beats": [], "msg": "Then follow the steps that appear here."}
    ready = threading.Event()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def _send(self, body, kind="text/plain"):
            data = body.encode()
            self.send_response(200)
            self.send_header("Content-Type", kind + "; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path in (token, token + "/"):
                self._send(PAGE, "text/html")
            elif self.path == token + "/msg":
                self._send(state["msg"])
            else:
                self.send_error(404)

        def do_POST(self):
            length = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(length).decode(errors="replace") if length else ""
            if self.path == token + "/ok":
                state["ok"] = body
                ready.set()
            elif self.path == token + "/beat":
                state["beats"].append(time.time())
            else:
                self.send_error(404)
                return
            self._send("ok")

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = "http://127.0.0.1:%d%s/" % (server.server_address[1], token)
    say("")
    say("  ==== Test 2/3: silent audio in a web page ====")
    say("  A page opens over the terminal. Tap its button and follow the page.")
    say("  If no page opens: BROWSER=internalbrowser openurl %s" % url)
    os.environ["BROWSER"] = "internalbrowser"
    kv("page openurl exit", os.system("openurl " + url))
    if not ready.wait(120):
        kv("page audio", "never started (no tap within 120 s)")
        server.shutdown()
        return
    kv("page audio state", state["ok"])
    for left in range(8, 0, -1):
        state["msg"] = "2. In %d s: go to Settings and wait there until you hear 'come back' (or ~40 s)." % left
        time.sleep(1)
    state["msg"] = "NOW: go to Settings. Come back when you hear 'come back' (or after ~40 s)."
    hb = Heartbeat().begin()
    before = len(state["beats"])
    time.sleep(38)
    code = _speak("BariStore probe. Come back now.")
    time.sleep(4)
    result = hb.end()
    result["say_exit"] = code
    _report_heartbeat("page audio", result)
    beats = state["beats"][before:]
    page_gap = max([b - a for a, b in zip(beats, beats[1:])] or [0.0])
    kv("  page longest pause", "%.1f s (%d page beats)" % (page_gap, len(beats)))
    for left in range(20, 0, -1):
        state["msg"] = ("3. Test 2 done (no error). Close this page now: Done / swipe down. "
                        "Test 3 starts in %d s." % left)
        time.sleep(1)
    server.shutdown()


SILENCE = os.path.join(DIR, "silence_15min.wav")
_PLAYING = {"started": False}


def _silence_wav(path, seconds):
    rate = 8000
    data_len = rate * seconds
    with open(path, "wb") as handle:
        handle.write(b"RIFF" + struct.pack("<I", 36 + data_len) + b"WAVEfmt ")
        handle.write(struct.pack("<IHHIIHH", 16, 1, 1, rate, rate, 1, 8))
        handle.write(b"data" + struct.pack("<I", data_len))
        handle.write(b"\x80" * data_len)


def start_play():
    """Start a-Shell's player once per probe run (a second `play` is ignored
    by a-Shell while the first player is still on screen)."""
    if _PLAYING["started"]:
        return "already playing"
    if not os.path.isfile(SILENCE):
        _silence_wav(SILENCE, 15 * 60)
    code = os.system('play "%s"' % SILENCE)
    _PLAYING["started"] = True
    return code


def keepalive_play():
    say("")
    say("  ==== Test 3/3: a-Shell's own player ====")
    say("  A FULL-SCREEN player opens and plays 15 min of silence. That is expected.")
    say("  Do NOT close it (the pair test uses it too). Just swipe to Home as usual.")
    kv("play exit", start_play())
    time.sleep(4)
    result = _trip("Test 3/3: play (player on screen)")
    say("  Close the player now (Done): the 'come back' voice may have stopped it.")
    _report_heartbeat("play", result)
    _after_trip(result)


def part_keepalive():
    header("keepalive")
    say("")
    say("  Three tests (~4 min). Each one: 8 s to get ready, then ~40 s in Settings.")
    say("  If you hear 'come back' while in Settings, a-Shell was still running.")
    guarded("none", keepalive_none)
    guarded("page", keepalive_page)
    guarded("play", keepalive_play)


# --------------------------------------------------------------------------
# pair (advertise only; no pairing happens)
# --------------------------------------------------------------------------

def siphash24(key, data):
    """SipHash-2-4 (public-domain algorithm), returns a u64."""
    mask = (1 << 64) - 1
    k0, k1 = struct.unpack("<QQ", key)
    v = [k0 ^ 0x736F6D6570736575, k1 ^ 0x646F72616E646F6D,
         k0 ^ 0x6C7967656E657261, k1 ^ 0x7465646279746573]

    def rotl(value, bits):
        return ((value << bits) | (value >> (64 - bits))) & mask

    def rounds(count):
        for _ in range(count):
            v[0] = (v[0] + v[1]) & mask
            v[1] = rotl(v[1], 13) ^ v[0]
            v[0] = rotl(v[0], 32)
            v[2] = (v[2] + v[3]) & mask
            v[3] = rotl(v[3], 16) ^ v[2]
            v[0] = (v[0] + v[3]) & mask
            v[3] = rotl(v[3], 21) ^ v[0]
            v[2] = (v[2] + v[1]) & mask
            v[1] = rotl(v[1], 17) ^ v[2]
            v[2] = rotl(v[2], 32)

    length = len(data)
    end = length - (length % 8)
    for offset in range(0, end, 8):
        m = struct.unpack_from("<Q", data, offset)[0]
        v[3] ^= m
        rounds(2)
        v[0] ^= m
    last = (length & 0xFF) << 56
    for index in range(length - end):
        last |= data[end + index] << (8 * index)
    v[3] ^= last
    rounds(2)
    v[0] ^= last
    v[2] ^= 0xFF
    rounds(4)
    return (v[0] ^ v[1] ^ v[2] ^ v[3]) & mask


def _dns_name(text):
    return b"".join(bytes([len(label.encode())]) + label.encode()
                    for label in text.rstrip(".").split(".")) + b"\x00"


def _rr(name, rtype, rclass, ttl, rdata):
    return _dns_name(name) + struct.pack(">HHIH", rtype, rclass, ttl, len(rdata)) + rdata


def announcement(identifier, txt, port, ipv4, ttl):
    service = "_remotepairing-pairable-host._tcp.local"
    instance = "%s.%s" % (identifier, service)
    host = "baristore-probe.local"
    items = [("%s=%s" % pair).encode() for pair in txt.items()]
    txt_raw = b"".join(bytes([len(item)]) + item for item in items)
    records = [
        _rr(service, 12, 1, ttl, _dns_name(instance)),
        _rr(instance, 33, 0x8001, ttl, struct.pack(">HHH", 0, 0, port) + _dns_name(host)),
        _rr(instance, 16, 0x8001, ttl, txt_raw),
        _rr(host, 1, 0x8001, ttl, socket.inet_aton(ipv4)),
    ]
    return struct.pack(">HHHHHH", 0, 0x8400, 0, len(records), 0, 0) + b"".join(records)


def _read_name(data, offset, depth=0):
    labels = []
    while offset < len(data) and depth < 20:
        length = data[offset]
        if length == 0:
            return ".".join(labels), offset + 1
        if length & 0xC0 == 0xC0:
            pointer = ((length & 0x3F) << 8) | data[offset + 1]
            name, _end = _read_name(data, pointer, depth + 1)
            labels.append(name)
            return ".".join(labels), offset + 2
        labels.append(data[offset + 1:offset + 1 + length].decode("utf-8", "replace"))
        offset += 1 + length
    return ".".join(labels), offset


def _mdns_summary(data):
    """'Q qu=1 _foo._tcp.local' or 'R name' for the first question/answer."""
    try:
        flags, qd, an = struct.unpack(">HHH", data[2:8])
        offset = 12
        if qd:
            name, offset = _read_name(data, offset)
            qclass = struct.unpack(">H", data[offset + 2:offset + 4])[0]
            kind = "R" if flags & 0x8000 else "Q"
            return "%s qu=%d %s (+%d q, %d an)" % (kind, qclass >> 15, name, qd - 1, an)
        if an:
            name, offset = _read_name(data, offset)
            return "R %s (%d an)" % (name, an)
        return "flags=%04x empty" % flags
    except Exception:                                           # noqa: BLE001
        return "unparsable %dB" % len(data)


IP_RECVDSTADDR = getattr(socket, "IP_RECVDSTADDR", 7)      # Darwin
IP_PKTINFO = getattr(socket, "IP_PKTINFO", 8)               # Linux (tests)


def _enable_dst(sock):
    try:
        if sys.platform.startswith("linux"):
            sock.setsockopt(socket.IPPROTO_IP, IP_PKTINFO, 1)
        else:
            sock.setsockopt(socket.IPPROTO_IP, IP_RECVDSTADDR, 1)
        return True
    except OSError:
        return False


def _recv_with_dst(sock):
    data, ancillary, _flags, source = sock.recvmsg(9000, 256)
    dst = "?"
    for level, kind, payload in ancillary:
        if level != socket.IPPROTO_IP:
            continue
        if kind == IP_PKTINFO and sys.platform.startswith("linux") and len(payload) >= 12:
            dst = socket.inet_ntoa(payload[8:12])
        elif kind == IP_RECVDSTADDR and len(payload) >= 4:
            dst = socket.inet_ntoa(payload[:4])
    return data, source, dst


def part_pair():
    header("pair (advertise only)")
    wifi = _route_ip("8.8.8.8")
    if not wifi or wifi.startswith(("none", "127.", "169.254.")):
        kv("wifi", "no Wi-Fi IPv4 (%s): skipped" % wifi)
        return
    identifier = str(uuid.uuid4()).upper()
    alt_irk = os.urandom(16)
    tag = struct.pack("<Q", siphash24(alt_irk, identifier.encode()))
    import base64
    txt = {
        "name": "BariStore Probe", "identifier": identifier,
        "authTag": base64.b64encode(bytes(tag[5 - i] for i in range(6))).decode(),
        "model": "Mac17,7", "flags": "1", "ver": "26", "minVer": "17",
    }
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("0.0.0.0", 0))
    listener.listen(4)
    listener.settimeout(0.25)
    port = listener.getsockname()[1]
    packet = announcement(identifier, txt, port, wifi, 75 * 60)
    goodbye = announcement(identifier, txt, port, wifi, 0)

    # Source port 5353 (RFC 6762 §6: answers from other ports may be ignored),
    # joined to the group so Settings' own queries reach us and get answers.
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    udp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        udp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
    except (OSError, AttributeError):
        pass
    udp.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    try:
        udp.bind(("", 5353))
        kv("udp source", "port 5353")
    except OSError as exc:
        kv("udp source", "ephemeral (bind 5353 failed: %s)" % exc)
    try:
        udp.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP,
                       socket.inet_aton(MCAST) + socket.inet_aton(wifi))
    except OSError:
        pass
    kv("dst address option", "on" if _enable_dst(udp) else "unavailable")
    udp.setblocking(False)
    targets = [("multicast", MCAST), ("subnet bcast", _subnet_broadcast(wifi)),
               ("own Wi-Fi IP", wifi), ("loopback", "127.0.0.1"),
               ("LocalDevVPN 10.7.0.1", "10.7.0.1")]
    sent_ok = {}
    heard = {"packets": 0, "our_queries": 0, "from": set(), "kinds": {}, "log": []}

    def send(data, extra=None):
        for label, target in targets + ([("querier", extra)] if extra else []):
            if not target:
                continue
            try:
                udp.sendto(data, (target, 5353))
                sent_ok[label] = sent_ok.get(label, 0) + 1
            except OSError:
                sent_ok.setdefault(label, 0)

    def listen():
        while True:
            try:
                data, source, dst = _recv_with_dst(udp)
            except (BlockingIOError, InterruptedError, OSError):
                return
            heard["packets"] += 1
            own = data == packet or data == goodbye
            summary = "own announcement" if own else _mdns_summary(data)
            key = "%s -> %s : %s" % (source[0], dst, summary if not own else "own")
            heard["kinds"][key] = heard["kinds"].get(key, 0) + 1
            if len(heard["log"]) < 30 and not own:
                heard["log"].append("+%5.1fs %s:%d -> %s  %s" % (
                    time.time() - start, source[0], source[1], dst, summary))
            is_query = len(data) > 3 and not data[2] & 0x80
            if is_query and b"_remotepairing-pairable-host" in data:
                heard["our_queries"] += 1
                heard["from"].add(source[0])
                send(packet, source[0])

    kv("listening", "%s:%d" % (wifi, port))
    kv("route to 10.7.0.1", _route_ip("10.7.0.1"))
    kv("instance", "identifier (like idevice/siboot), TTL 75 min")
    say("")
    say("  ==== Pair entry test (nothing is paired) ====")
    say("  Keep LocalDevVPN ON and Wi-Fi ON for this test.")
    say("  If a player from an earlier test is still on screen, close it now.")
    countdown(10, "starting the player")
    _PLAYING["started"] = False
    kv("play exit", start_play())
    time.sleep(3)
    say("  BEFORE NOW: in Settings go back to its first page (so the Developer Mode")
    say("  page is closed). When it says NOW:")
    say("    1. Settings > Privacy & Security > Developer Mode, scroll to the bottom.")
    say("    2. Go back one page, open Developer Mode again. Repeat 3 times,")
    say("       ~10 s apart (each opening starts a new search for hosts).")
    say("  If 'BariStore Probe' appears, TAP it (it will fail: expected).")
    say("  Stay in Settings until you hear 'come back' (about 90 s).")
    start = time.time()
    for _ in range(3):
        send(packet)
        time.sleep(0.2)
    countdown(5, "go to Settings")
    hb = Heartbeat().begin()
    start = time.time()
    connection = None
    last_send = time.time()
    while time.time() - start < 90 and connection is None:
        try:
            connection = listener.accept()
        except socket.timeout:
            pass
        except OSError as exc:
            kv("accept", "error %s" % exc)
            break
        listen()
        if time.time() - last_send > 0.5:
            send(packet)
            last_send = time.time()
    if connection is None:
        kv("iOS dialled in", "NO within %.0f s" % (time.time() - start))
    else:
        sock, peer = connection
        kv("iOS dialled in", "YES after %.1f s from %s" % (time.time() - start, peer[0]))
        sock.settimeout(5)
        try:
            data = b""
            while len(data) < 11:
                chunk = sock.recv(4096)
                if not chunk:
                    break
                data += chunk
            if data[:9] == b"RPPairing":
                length = struct.unpack(">H", data[9:11])[0]
                while len(data) < 11 + length:
                    chunk = sock.recv(4096)
                    if not chunk:
                        break
                    data += chunk
                body = json.loads(data[11:11 + length])
                plain = body.get("message", {}).get("plain", {}).get("_0", {})
                hs = plain.get("request", {}).get("_0", {}).get("handshake", {}).get("_0", {})
                kv("first frame", "RPPairing handshake wire=%s options=%s" % (
                    hs.get("wireProtocolVersion"), json.dumps(hs.get("hostOptions"))))
            else:
                kv("first frame", "not RPPairing: %r" % data[:24])
        except Exception as exc:                                # noqa: BLE001
            kv("first frame", "error %s" % exc)
        sock.close()
    kv("announce sent", json.dumps(sent_ok))
    kv("mDNS heard", "%d packets, %d queries for our type, from %s" % (
        heard["packets"], heard["our_queries"], sorted(heard["from"]) or "-"))
    out("  packet kinds (source -> destination : what, count):")
    for key, count in sorted(heard["kinds"].items(), key=lambda item: -item[1])[:25]:
        out("    %4d  %s" % (count, key))
    out("  first packets that were not ours:")
    for line in heard["log"]:
        out("    " + line)
    for _ in range(2):
        send(goodbye)
        time.sleep(0.2)
    listener.close()
    udp.close()
    _report_heartbeat("during pair test", hb.end())
    _speak("BariStore probe. Pair test over. Come back now.")
    say("  You can close the player now (Done).")


# --------------------------------------------------------------------------

def main(argv):
    ref = ""
    if "--ref" in argv:
        index = argv.index("--ref")
        ref = argv[index + 1] if index + 1 < len(argv) else ""
        del argv[index:index + 2]
    wanted = [part for part in argv if part in PARTS] or list(PARTS)
    try:
        os.remove(REPORT)
    except OSError:
        pass
    out("BariStore probe %s  ref=%s  %s" % (PROBE_VERSION, ref[:12] or "-",
                                             time.strftime("%Y-%m-%d %H:%M:%S %z")))
    out("parts: %s" % " ".join(wanted))
    for part in wanted:
        guarded(part, globals()["part_" + part])
    out("")
    out("done in %.0f s" % (time.time() - _t0))
    copied = os.system('pbcopy < "%s"' % REPORT) == 0
    say("")
    say("  Report: %s" % REPORT)
    say("  %s" % ("Copied to the clipboard: paste it into the chat."
                  if copied else "Open it in Files > a-Shell > baristore_probe and share it."))


if __name__ == "__main__":
    random.seed()
    main(sys.argv[1:])
