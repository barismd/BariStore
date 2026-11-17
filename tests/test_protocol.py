#!/usr/bin/env python3
"""Unit tests for BariStore protocol layers (no device, no external network)."""
import asyncio
import base64
import os
import plistlib
import secrets
import struct
import sys
import tempfile
import zipfile
from types import SimpleNamespace
from unittest.mock import Mock, patch

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))

from baristore import device, mdns, pairing, remote_xpc


def test_siphash_vectors():
    key = bytes(range(16))
    assert pairing.siphash24(key, b"") == 0x726FDB47DD0E0E31
    assert pairing.siphash24(key, bytes([0])) == 0x74F839C593DC67FD
    assert pairing.siphash24(key, bytes(range(7))) == 0xAB0200F58B01D137


def test_auth_tag_vector():
    alt_irk = base64.b64decode("Mgp6ZGPzXM2ku9br46vsiw==")
    identifier = "2BE6E510-0325-4365-923E-B14C6F57DB3A"
    tag = pairing.auth_tag(alt_irk, identifier)
    assert base64.b64encode(tag).decode() == "kXjlTr2l"


def test_tlv_roundtrip_chunking():
    value = bytes(range(256)) * 3
    encoded = pairing.tlv_encode([(3, value), (6, b"\x01")])
    decoded = pairing.tlv_decode(encoded)
    assert decoded[3] == value
    assert decoded[6] == b"\x01"


def test_opack_roundtrip():
    value = {
        "altIRK": bytes(range(16)),
        "btAddr": "11:22:33:44:55:66",
        "mac": b"\x11\x22\x33\x44\x55\x66",
        "remotepairing_serial_number": "AAAAAAAAAAAA",
        "accountID": "ABC",
        "model": "iPhone17,1",
        "name": "Target",
        "zero": 0,
        "big": 70000,
        "yes": True,
    }
    assert pairing.opack_decode(pairing.opack_encode(value)) == value


def test_opack_backref():
    payload = b"\xE2" + b"\x44name" + b"\x43ABC" + b"\x49accountID" + b"\xA1"
    assert pairing.opack_decode(payload) == {"name": "ABC", "accountID": "ABC"}


def test_xpc_encoding():
    payload = remote_xpc.encode_payload({})
    assert payload == (
        struct.pack("<II", 0x42133742, 5)
        + struct.pack("<II", 0xF000, 4)
        + struct.pack("<I", 0)
    )
    wrapper = remote_xpc.encode_wrapper({}, message_id=0, flags=0x0001)
    assert wrapper[:24] == struct.pack("<IIQQ", 0x29B00B92, 0x0001, len(payload), 0)
    flags, message_id, value = remote_xpc.decode_wrapper(wrapper)
    assert flags == 1 and message_id == 0 and value == {}


def test_xpc_roundtrip():
    value = {
        "MessageType": "Handshake",
        "Services": {"a": {"Port": "1234"}},
        "N": 7,
        "U": 2**40,
        "B": True,
        "D": b"\x00\x01\x02",
        "A": [1, "x"],
    }
    flags, message_id, decoded = remote_xpc.decode_wrapper(
        remote_xpc.encode_wrapper(value, message_id=9, wanting_reply=True)
    )
    assert flags == (0x0001 | 0x0100 | 0x10000)
    assert message_id == 9
    assert decoded == value


def test_mdns_announcement_and_parser():
    identity = pairing.HostIdentity.generate()
    packet = mdns.build_announcement(identity, 54321, "192.168.1.50", name="BariStore")
    records = mdns.parse_packet(packet)
    ptrs = [r["target"] for r in records if r["type"] == 12]
    assert "%s.%s" % (identity.identifier, mdns.SERVICE) in ptrs
    assert "BariStore.%s" % mdns.SERVICE in ptrs
    srvs = [r for r in records if r["type"] == 33]
    assert any(r["port"] == 54321 for r in srvs)
    txts = [r for r in records if r["type"] == 16]
    assert any(r["txt"].get("identifier") == identity.identifier for r in txts)
    assert any(r["txt"].get("name") == "BariStore" for r in txts)


def test_mdns_advertiser_sends_on_a_fixed_schedule():
    async def main():
        fake_socket = Mock()
        fake_socket.recvfrom.side_effect = AssertionError("advertiser should only send")
        socket_api = SimpleNamespace(**vars(mdns.socket))
        socket_api.socket = Mock(return_value=fake_socket)
        waits = asyncio.Queue()

        async def pause(seconds):
            resume = asyncio.Event()
            await waits.put((seconds, resume))
            await resume.wait()

        advertiser_asyncio = SimpleNamespace(create_task=asyncio.create_task, sleep=pause)
        with (
            patch.object(mdns, "asyncio", advertiser_asyncio),
            patch.object(mdns, "interface_ips", return_value={"en0": "192.168.1.50", "en1": "192.168.2.50"}),
            patch.object(mdns, "socket", socket_api),
        ):
            advertiser = mdns.Advertiser(pairing.HostIdentity.generate(), 54321).start()
            task = advertiser._task
            destinations = {
                (mdns.MCAST, 5353), (mdns.BCAST_ALL, 5353), ("192.168.1.255", 5353),
                ("192.168.1.50", 5353), ("192.168.2.50", 5353),
                (mdns.LOOPBACK_VPN, 5353), (mdns.LOOPBACK_LOCAL, 5353),
            }

            def check_burst():
                assert {call.args[1] for call in fake_socket.sendto.call_args_list} == destinations
                assert fake_socket.sendto.call_count == len(destinations)
                fake_socket.sendto.reset_mock()

            try:
                check_burst()
                for _ in range(2):
                    seconds, resume = await asyncio.wait_for(waits.get(), timeout=1)
                    assert seconds == 1.0
                    resume.set()
                    await asyncio.sleep(0)
                    check_burst()
                fake_socket.recvfrom.assert_not_called()
            finally:
                advertiser.stop()
                await asyncio.gather(task, return_exceptions=True)
            assert task.cancelled()
            fake_socket.close.assert_called_once()
            fake_socket.sendto.assert_not_called()

    asyncio.run(main())


class DirectConnection:
    def __init__(self, reader, writer):
        self.reader = reader
        self.writer = writer

    async def readexactly(self, size):
        return await self.reader.readexactly(size)

    def write(self, data):
        self.writer.write(data)

    async def drain(self):
        await self.writer.drain()

    def close(self):
        self.writer.close()

    async def wait_closed(self):
        await self.writer.wait_closed()


class DirectStack:
    async def connect(self, host, port, timeout=10):
        reader, writer = await asyncio.open_connection(host, port)
        return DirectConnection(reader, writer)


class DummySession:
    def __init__(self, stack, address, services, udid="a" * 40):
        self.stack = stack
        self.address = address
        self.services = services
        self.udid = udid


async def _read_plist(reader):
    length = struct.unpack(">I", await reader.readexactly(4))[0]
    return plistlib.loads(await reader.readexactly(length))


async def _send_plist(writer, value):
    payload = plistlib.dumps(value, fmt=plistlib.FMT_XML)
    writer.write(struct.pack(">I", len(payload)) + payload)
    await writer.drain()


async def _afc_server(reader, writer):
    while True:
        try:
            header = await reader.readexactly(40)
        except (asyncio.IncompleteReadError, ConnectionError):
            return
        entire = struct.unpack("<Q", header[8:16])[0]
        op = struct.unpack("<Q", header[32:40])[0]
        if entire > 40:
            await reader.readexactly(entire - 40)
        body = struct.pack("<Q", 77) if op == device.OP_FILE_OPEN else b""
        writer.write(
            device.AFC_MAGIC
            + struct.pack("<QQQQ", 40 + len(body), 40 + len(body), 0, device.OP_DATA)
            + body
        )
        await writer.drain()


def test_afc_upload():
    async def main():
        server = await asyncio.start_server(_afc_server, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        afc = device.Afc(device.LockdownConnection(reader, writer))
        await afc.upload("/PublicStaging/x.ipa", b"hello" * 50000)
        writer.close()
        await writer.wait_closed()
        server.close()
        await server.wait_closed()

    asyncio.run(main())


def test_rsd_handshake():
    async def handler(reader, writer):
        assert await reader.readexactly(len(remote_xpc.HTTP2_MAGIC)) == remote_xpc.HTTP2_MAGIC
        for _ in range(7):
            header = await reader.readexactly(9)
            length = int.from_bytes(header[:3], "big")
            if length:
                await reader.readexactly(length)
        writer.write(remote_xpc.Frame(remote_xpc.TYPE_SETTINGS, 0, 0, b"").build())
        peer = {
            "Services": {"com.apple.afc.shim.remote": {"Port": "1234"}},
            "Properties": {"UniqueDeviceID": "a" * 40, "ProductType": "iPhone17,1"},
        }
        payload = remote_xpc.encode_wrapper(peer, message_id=1, flags=0x0001)
        writer.write(remote_xpc.Frame(remote_xpc.TYPE_DATA, 0, 1, payload).build())
        await writer.drain()
        await asyncio.sleep(0.2)
        writer.close()

    async def main():
        server = await asyncio.start_server(handler, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        services, properties, client = await remote_xpc.connect_rsd(reader, writer)
        assert properties["UniqueDeviceID"] == "a" * 40
        assert remote_xpc.service_port(services, "com.apple.afc.shim.remote") == 1234
        await client.close()
        server.close()
        await server.wait_closed()

    asyncio.run(main())


def test_install_uninstall_and_lookup():
    import datetime

    install_commands = []

    async def afc_handler(reader, writer):
        await _send_plist(writer, {"Request": "RSDCheckin"})
        await _read_plist(reader)
        await _send_plist(writer, {"Request": "StartService"})
        await _afc_server(reader, writer)

    async def install_handler(reader, writer):
        await _send_plist(writer, {"Request": "RSDCheckin"})
        request = await _read_plist(reader)
        assert request["Request"] == "RSDCheckin"
        await _send_plist(writer, {"Request": "StartService"})
        command = await _read_plist(reader)
        install_commands.append(command)
        if command["Command"] == "Lookup":
            await _send_plist(
                writer,
                {
                    "LookupResult": {
                        "com.example.App": {
                            "CFBundleExecutable": "App",
                            "Path": "/private/var/containers/Bundle/Application/123/App.app",
                        }
                    }
                },
            )
        else:
            await _send_plist(writer, {"Status": "Complete"})

    async def main():
        afc_server = await asyncio.start_server(afc_handler, "127.0.0.1", 0)
        install_server = await asyncio.start_server(install_handler, "127.0.0.1", 0)
        services = {
            device.AFC_SERVICE: {"Port": afc_server.sockets[0].getsockname()[1]},
            device.INSTALL_SERVICE: {"Port": install_server.sockets[0].getsockname()[1]},
        }
        sess = DummySession(DirectStack(), "127.0.0.1", services)

        now = datetime.datetime.now(datetime.timezone.utc)
        profile_blob = plistlib.dumps(
            {
                "ExpirationDate": now + datetime.timedelta(days=7),
                "ProvisionedDevices": ["a" * 40],
            },
            fmt=plistlib.FMT_XML,
        )
        with tempfile.NamedTemporaryFile(suffix=".ipa", delete=False) as tf:
            ipa_path = tf.name
        with zipfile.ZipFile(ipa_path, "w") as zf:
            zf.writestr(
                "Payload/App.app/Info.plist",
                plistlib.dumps({"CFBundleIdentifier": "com.example.App"}, fmt=plistlib.FMT_XML),
            )
            zf.writestr("Payload/App.app/embedded.mobileprovision", profile_blob)

        try:
            await device.install_ipa(sess, ipa_path, auto_inject_pairing=False)
            assert install_commands[0]["Command"] == "Install"
            looked = await device.lookup_installed_apps(sess, ["com.example.App"])
            assert "com.example.App" in looked
            await device.uninstall_app(sess, "com.example.App")
            assert install_commands[-1]["Command"] == "Uninstall"
        finally:
            os.unlink(ipa_path)
            afc_server.close()
            install_server.close()
            await afc_server.wait_closed()
            await install_server.wait_closed()

    asyncio.run(main())


def test_srp_roundtrip():
    pin = "123456"
    server = pairing.prepare_srp_server(pin)
    salt = server.salt
    n, g, k = pairing.SRP_N, pairing.SRP_G, pairing.SRP_K
    a = secrets.randbits(256)
    A = pow(g, a, n)
    server.process(pairing._min_bytes(A))
    u = pairing._hi(pairing._pad(A), pairing._pad(server.public))
    x = pairing._hi(salt, pairing._h(("Pair-Setup:%s" % pin).encode()))
    S = pow((server.public - k * pow(g, x, n)) % n, a + u * x, n)
    K = pairing._h(pairing._min_bytes(S))
    assert K == server.key
    group = pairing._hi(pairing._min_bytes(n)) ^ pairing._hi(pairing._min_bytes(g))
    M1 = pairing._h(
        pairing._min_bytes(group),
        pairing._min_bytes(pairing._hi(b"Pair-Setup")),
        salt,
        pairing._min_bytes(A),
        pairing._min_bytes(server.public),
        K,
    )
    assert server.verify(M1)
    M2 = pairing._h(pairing._min_bytes(A), M1, K)
    assert server.proof == M2


if __name__ == "__main__":
    functions = [
        (name, value)
        for name, value in sorted(globals().items())
        if name.startswith("test_") and callable(value)
    ]
    for name, function in functions:
        function()
        print("ok  %s" % name)
    print("%d tests passed" % len(functions))
