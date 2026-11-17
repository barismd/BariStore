#!/usr/bin/env python3
# =============================================================================
#  BariStore — mDNS / Raw Broadcast Advertiser for RPPairing (MIT)
#
#  a-Shell cannot use the system DNS-SD API (its Info.plist has no
#  NSBonjourServices entry, returning -65555 NoAuth). This module publishes
#  `_remotepairing-pairable-host._tcp.local` directly over UDP 5353 via:
#    * mDNS multicast (224.0.0.251:5353)
#    * Limited broadcast (255.255.255.255:5353)
#    * Directed subnet broadcast (x.x.x.255:5353)
#    * Direct unicast to device Wi-Fi IP, LocalDevVPN (10.7.0.1), and 127.0.0.1
#
#  Both `<identifier>._remotepairing-pairable-host._tcp.local` (as required by
#  Apple's RPPairing responder specification) and `BariStore._remotepairing-...`
#  are announced at startup and once per second while pairing.
# =============================================================================

import asyncio
import socket
import struct
import sys

from . import pairing

MCAST = "224.0.0.251"
BCAST_ALL = "255.255.255.255"
LOOPBACK_VPN = "10.7.0.1"
LOOPBACK_LOCAL = "127.0.0.1"

PAIRABLE_HOST = "_remotepairing-pairable-host._tcp"
REMOTE_PAIRING = "_remotepairing._tcp"
SERVICE = "_remotepairing-pairable-host._tcp.local"


def _txt_record(properties):
    payload = b""
    for key, value in properties.items():
        key_b = str(key).encode("utf-8")
        val_b = str(value).encode("utf-8")
        if len(key_b) > 255 or len(val_b) > 255:
            continue
        entry = key_b + b"=" + val_b
        if len(entry) > 255:
            continue
        payload += bytes([len(entry)]) + entry
    return payload


def _name(text):
    out = b""
    for label in text.rstrip(".").split("."):
        raw = label.encode("utf-8")
        out += bytes([len(raw)]) + raw
    return out + b"\x00"


def _rr(name, rtype, rclass, ttl, rdata):
    return _name(name) + struct.pack(">HHIH", rtype, rclass, ttl, len(rdata)) + rdata


def build_announcement(identity, port, ipv4, name="BariStore", ttl=120):
    """Build an mDNS announcement advertising the pairable host."""
    txt = _txt_record(pairing.mdns_txt(identity, name=name))
    short_id = (identity.identifier or "baristore")[:8].lower()
    host = "baristore-%s.local" % short_id
    id_instance = "%s.%s" % (identity.identifier, SERVICE)
    named_instance = "%s.%s" % (name, SERVICE)

    records = [
        # Primary instance keyed by host identifier (matches idevice PairableHost)
        _rr(SERVICE, 12, 1, ttl, _name(id_instance)),
        _rr(id_instance, 33, 0x8001, ttl, struct.pack(">HHH", 0, 0, int(port)) + _name(host)),
        _rr(id_instance, 16, 0x8001, ttl, txt),
        # Friendly alias instance
        _rr(SERVICE, 12, 1, ttl, _name(named_instance)),
        _rr(named_instance, 33, 0x8001, ttl, struct.pack(">HHH", 0, 0, int(port)) + _name(host)),
        _rr(named_instance, 16, 0x8001, ttl, txt),
        # Host A record
        _rr(host, 1, 0x8001, ttl, socket.inet_aton(ipv4)),
    ]
    header = struct.pack(">HHHHHH", 0, 0x8400, 0, len(records), 0, 0)
    return header + b"".join(records)


def wifi_ipv4():
    """Route-based best guess (may be the VPN tunnel address — use lan_ipv4 for the A record)."""
    for target in (("8.8.8.8", 80), ("1.1.1.1", 80), ("10.7.0.1", 49152)):
        probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            probe.connect(target)
            ip = probe.getsockname()[0]
            if ip and not ip.startswith(("127.", "0.")):
                return ip
        except OSError:
            pass
        finally:
            probe.close()
    return LOOPBACK_VPN


_VPN_PREFIXES = (
    "utun", "ipsec", "stf", "gif", "lo", "awdl", "llw", "pktap",
    "anpi", "bridge", "tap", "docker", "veth", "virbr", "feth",
)
_VIRTUAL_IPS = ("127.", "0.", "169.254.", "10.7.0.")


def interface_ips():
    """Return {interface_name: ipv4} for all local interfaces."""
    found = {}
    try:
        import fcntl
        is_apple = sys.platform == "darwin"
        SIOCGIFADDR = 0xC0206911 if is_apple else 0x8915
        ifreq_size = 32 if is_apple else 40
        probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            names = [name for _idx, name in socket.if_nameindex()]
        except Exception:
            names = []
        for name in names:
            try:
                raw = fcntl.ioctl(
                    probe.fileno(),
                    SIOCGIFADDR,
                    struct.pack("16s", name.encode()[:15]) + b"\0" * (ifreq_size - 16),
                )
                ip = socket.inet_ntoa(raw[20:24])
                if ip and not ip.startswith(_VIRTUAL_IPS):
                    found[name] = ip
            except Exception:
                pass
        probe.close()
    except Exception:
        pass
    return found


def lan_ipv4():
    """Pick a real LAN address (Wi-Fi/cellular), NEVER a VPN utun address.

    The mDNS A record must point at an address the iOS device can reach on the
    local network. If it points into the LocalDevVPN tunnel, Settings connects
    into the tunnel and the PIN prompt hangs forever.
    """
    ips = interface_ips()
    if ips:
        for name in sorted(ips):
            if name.startswith("en") or name.startswith("eth"):
                return ips[name]
        for name in sorted(ips):
            if not name.startswith(_VPN_PREFIXES):
                return ips[name]
        for name in sorted(ips):
            return ips[name]
    guess = wifi_ipv4()
    # Reject tunnel/loopback guesses; fall back to a documented placeholder.
    if guess.startswith(_VIRTUAL_IPS) or guess.startswith("10."):
        return LOOPBACK_VPN
    return guess


def subnet_broadcast(ipv4):
    parts = (ipv4 or "").split(".")
    if len(parts) != 4:
        return ""
    try:
        octets = [int(p) for p in parts]
    except ValueError:
        return ""
    if any(o < 0 or o > 255 for o in octets):
        return ""
    if octets[0] in (10, 127):
        return ""
    return "%d.%d.%d.255" % (octets[0], octets[1], octets[2])


class Advertiser:
    """Send rate-limited mDNS announcements for on-device pairing."""

    def __init__(self, identity, port, name="BariStore"):
        self.identity = identity
        self.port = int(port)
        self.name = name
        self.ipv4 = lan_ipv4()
        self.extra_ips = [ip for ip in interface_ips().values() if ip != self.ipv4]
        self.sock = None
        self.packets = []
        self.destinations = []
        self._task = None

    def start(self):
        primary_ip = self.ipv4 or LOOPBACK_VPN
        self.packets = [build_announcement(self.identity, self.port, primary_ip, name=self.name)]

        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        except OSError:
            pass
        if hasattr(socket, "SO_REUSEPORT"):
            try:
                sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
            except OSError:
                pass
        try:
            sock.bind(("", 5353))
        except OSError:
            pass
        try:
            sock.setsockopt(
                socket.IPPROTO_IP, socket.IP_MULTICAST_IF, socket.inet_aton(primary_ip)
            )
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 255)
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_LOOP, 1)
        except OSError:
            pass
        candidates = [
            ((MCAST, 5353), "multicast"),
            ((BCAST_ALL, 5353), "broadcast"),
        ]
        sub_bcast = subnet_broadcast(primary_ip)
        if sub_bcast:
            candidates.append(((sub_bcast, 5353), "broadcast"))
        for u_ip in {primary_ip, LOOPBACK_VPN, LOOPBACK_LOCAL} | set(self.extra_ips):
            if u_ip:
                candidates.append(((u_ip, 5353), "unicast"))

        active = []
        for dest, label in candidates:
            try:
                sock.sendto(self.packets[0], dest)
                active.append((dest, label))
            except OSError:
                pass

        if not active:
            sock.close()
            raise RuntimeError("Could not send mDNS packets on any local interface.")

        self.destinations = active
        sock.setblocking(False)
        self.sock = sock
        self._task = asyncio.create_task(self._loop())
        return self

    def _burst_once(self):
        if not self.sock:
            return
        for (address, port), _label in self.destinations:
            try:
                self.sock.sendto(self.packets[0], (address, port))
            except OSError:
                pass

    async def _loop(self):
        # start() already announces immediately. Reading the same mDNS socket
        # can feed our own announcements back into the send loop.
        while True:
            await asyncio.sleep(1.0)
            self._burst_once()

    def stop(self):
        if self._task is not None:
            self._task.cancel()
            self._task = None
        if self.sock is not None:
            try:
                self.sock.close()
            except OSError:
                pass
            self.sock = None


# --- DNS Packet Parser (for unit tests) -------------------------------------

def decode_name(data, offset):
    labels = []
    jumped = False
    end = offset
    guard = 0
    while True:
        guard += 1
        if guard > 128 or offset >= len(data):
            raise ValueError("bad DNS name")
        length = data[offset]
        if length == 0:
            offset += 1
            if not jumped:
                end = offset
            return ".".join(labels), end
        if length & 0xC0 == 0xC0:
            if offset + 1 >= len(data):
                raise ValueError("bad DNS pointer")
            pointer = ((length & 0x3F) << 8) | data[offset + 1]
            if not jumped:
                end = offset + 2
            jumped = True
            offset = pointer
            continue
        if offset + 1 + length > len(data):
            raise ValueError("bad DNS label")
        labels.append(data[offset + 1:offset + 1 + length].decode("utf-8", "replace"))
        offset += 1 + length
        if not jumped:
            end = offset


def parse_txt(raw):
    result = {}
    pos = 0
    while pos < len(raw):
        length = raw[pos]
        pos += 1
        entry = raw[pos:pos + length].decode("utf-8", "replace")
        pos += length
        if "=" in entry:
            key, value = entry.split("=", 1)
            result[key] = value
        elif entry:
            result[entry] = ""
    return result


def parse_packet(data):
    if len(data) < 12:
        return []
    _, _, qd, an, ns, ar = struct.unpack(">HHHHHH", data[:12])
    offset = 12
    for _ in range(qd):
        _, offset = decode_name(data, offset)
        offset += 4
    records = []
    for _ in range(an + ns + ar):
        name, offset = decode_name(data, offset)
        if offset + 10 > len(data):
            break
        rtype, rclass, ttl, rdlength = struct.unpack(">HHIH", data[offset:offset + 10])
        offset += 10
        rdata = data[offset:offset + rdlength]
        offset += rdlength
        record = {"name": name, "type": rtype, "ttl": ttl}
        if rtype == 12:
            record["target"], _ = decode_name(data, offset - rdlength)
        elif rtype == 33 and len(rdata) >= 6:
            priority, weight, port = struct.unpack(">HHH", rdata[:6])
            target, _ = decode_name(data, offset - rdlength + 6)
            record.update({"port": port, "target": target, "priority": priority, "weight": weight})
        elif rtype == 16:
            record["txt"] = parse_txt(rdata)
        elif rtype == 1 and len(rdata) == 4:
            record["address"] = socket.inet_ntop(socket.AF_INET, rdata)
        elif rtype == 28 and len(rdata) == 16:
            record["address"] = socket.inet_ntop(socket.AF_INET6, rdata)
        records.append(record)
    return records
