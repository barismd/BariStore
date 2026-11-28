# Third-party notices

BariStore's code is MIT and written for this project. No third-party source
code is copied into it.

## Libraries in the program

All are permissive. Generated with
`cargo tree -e normal --target wasm32-wasip1 --prefix none --format '{p} {l}'`
(111 crates on 2026-11-28):

| Licence | Crates |
|---|---|
| MIT and/or Apache-2.0 (incl. 0BSD/Zlib/BSD alternatives) | 104 |
| BSD-3-Clause (dalek's `curve25519-dalek`, `ed25519-dalek`, `x25519-dalek`; `subtle`) | 4 |
| Zlib (`zlib-rs`, flate2's deflate) | 1 |
| MIT OR Apache-2.0, AND Unicode-3.0 (`unicode-ident`, at build time) | 1 |
| Apache-2.0 WITH LLVM-exception alternatives (`wasi`) | 1 |

Main ones: `tokio` (only its I/O traits, channels and locks: the runtime is
BariStore's own), `plist`, `serde`, `zip`, `image` (icon and screenshot
thumbnails, app icons the user picks), `flate2` with `zlib-rs` (the IPA's
entries, gzip answers, the CgBI icons Xcode puts in apps), RustCrypto (`sha1`,
`sha2`, `hmac`, `hkdf`, `pbkdf2`, `aes`, `cbc`, `aes-gcm`, `chacha20poly1305`,
`rsa`), dalek (`ed25519-dalek`, `x25519-dalek`), `num-bigint`, `uuid`. Each
crate's licence text is in its source package on crates.io. TLS is rootshell's
(Network.framework); the program carries none.

Test-only (not in the program): `idevice-srp` (MIT OR Apache-2.0, an SRP oracle)
and `smoltcp` (0BSD, a TCP peer). `scripts/rootshell.mjs` downloads rootshell's
`Resources/Wasm/wasm-worker.js` (MIT) at a fixed commit when it runs, to run
the tests as rootshell does; it is not copied here.

## Apple certificates (`assets/certs/`)

Apple Root CA, Apple Root CA - G3 and the Apple Worldwide Developer Relations
intermediates G2-G6: Apple's public CA certificates. Each intermediate verifies
against one of the two roots; Apple Root CA matches the fingerprint Apple
publishes. Used to complete code-signing chains.

## Protocol references (read, not copied)

| Project | Licence | Used for |
|---|---|---|
| [kitknox/rootshell](https://github.com/kitknox/rootshell) | MIT | its WASM runtime (wasm/README.md, Resources/Wasm, rootshell/Features/Wasm, read 2026-11-27): the socket ABI, the 64 KB answer slot, accept without a time limit, receive time limits, its file broker's errors, the multicast entitlement and Bonjour types it declares, OSC 1337 `OpenURL` and OSC 777 notifications, Location Diary Mode keeping a program running in the background, `.rootshellrc` functions, how its shell runs typed lines, profiles and Ctrl-C (read 2026-11-28); its worker runs the tests (scripts/rootshell.mjs) |
| [atelierstore/atelier-install](https://github.com/atelierstore/atelier-install) | GPL-3.0 | behaviour only, from its README: knocking on a listener of one's own before accept, reads that fit the 64 KB slot, libc's read/write replaced at link time, check-ins against WebKit stopping a hidden web view, FileManager's EIO in place of EEXIST/ENOENT, `true &&` lines, Location Diary Mode and Terminal Notifications for pairing, a new tab's curl/ifconfig held while another tab's program runs, rootshell's `ifconfig` without a line break after `hwassist=` |
| [FrizzleM/siboot](https://github.com/FrizzleM/siboot) (rootshell pairing 0.4.0, as kept MIT in [Thanking157/SideNeko](https://github.com/Thanking157/SideNeko)'s SideNekoBootstrap) | MIT | the pairable host's raw mDNS announcement: PTR, SRV, TXT and an A record with the Wi-Fi address from `ifconfig`, a 75-minute TTL, sent more than once (device-proven there); the address from `ifconfig.txt` |
| [seomin0610/LocalDevVPN](https://github.com/seomin0610/LocalDevVPN) | StosVPN License (SideStore Team) | behaviour only: its PacketTunnelProvider swaps source and destination address of every IPv4 packet for 10.7.0.1, ports unchanged, so a datagram from a port to 10.7.0.1 at that same port comes back to its socket (the VPN check) |
| [jkcoxson/idevice](https://github.com/jkcoxson/idevice) | MIT | RemotePairing, TLS-PSK tunnel, RemoteXPC (file transfers, flow control), services, misagent, cryptexd, TSS, CoreDevice app service, debugserver; its TCP stack `jktcp` for MSS/window choices |
| [nab138/isideload](https://github.com/nab138/isideload) | MIT | anisette v3, GrandSlam (text-message codes: Apple's serviceErrors, -22979/-22981, a 412 for a code already sent), developer services, app groups, Increased Memory Limit, ALTCertificate.p12 placement |
| [nab138/iloader](https://github.com/nab138/iloader) | MIT | the advertised client info fallback (2.4.2); from its issues (#1203, #1205, #1304), that Apple refuses a client info spelt `<macOS;` with HTTP 503 and takes a current Mac spelt `<Mac OS X;`, and -22410 for an identity provisioned under another client info |
| [xtool-org/xtool](https://github.com/xtool-org/xtool) | MIT | behaviour only, from its issues: Apple refusing the text-message code with a bare 403 (#345), and a 412 that names the trusted phone number's id when it is not 1 (#424) |
| [SideStore/SideStore](https://github.com/SideStore/SideStore) | AGPL-3.0 | behaviour only: how it finds its signing certificate (ALTCertificateID, ALTCertificate.p12), that it asks before revoking a certificate and imports a .p12, its refresh (new profiles through misagent), how it registers App IDs, its source format's beta tracks (`releaseChannels`) |
| [altstoreio/AltStore](https://github.com/altstoreio/AltStore) | AGPL-3.0 | behaviour only: App IDs (the app's identifier + team, registered only when missing, extensions' profiles side by side), forgetting apps deleted on the device, the free team's limits, how it picks and replaces a certificate (AltServer keeps one per team) |
| [LiveContainer/LiveContainer](https://github.com/LiveContainer/LiveContainer) | AGPL-3.0 | behaviour only: its `livecontainer://certificate?cert=…&password=…` import, its `livecontainer://install?url=…` (downloads an IPA into LiveContainer; plain HTTP allowed by its ATS settings) and `livecontainer://source?url=…` (adds an AltSource to its Sources tab) links, where it keeps its sources (`LCAltStoreSourceURLs`), that plain LiveContainer reads no pairing file, and that its ZSign opens a .p12 with OpenSSL 3 |
| [nanofuxion/Whetstone](https://github.com/nanofuxion/Whetstone) | MIT | what its JITProbe tests (RWX, RW→RX, MAP_JIT), to explain its result after JIT |
| FrizzleM/SideInstaller | non-commercial | documented behaviour only: where SideStore / LiveContainer / AltStore read a pairing file; which client info Apple accepts (Oct–Nov 2026) |
| StikDebug/StikDebug | AGPL-3.0 | behaviour only: pairing-file destination, which DDI and where from, the JIT flow and what universal.js / legacy.js do (reimplemented, not copied) |
| [doronz88/DeveloperDiskImage](https://github.com/doronz88/DeveloperDiskImage) | Apple's images | downloaded at use time, not redistributed |

The apps BariStore offers (AltStore, SideStore, LiveContainer and every app in a
user's AltSources) are downloaded from their publishers at install time; none
is redistributed here.

## Logo and screenshots

`assets/baristore.svg`: BariStore's logo, provided by the project owner;
`docs/logo.png` is rendered from it.

`docs/screens/`: screenshots of BariStore's own page. The app icons and app
screenshots inside them come from the apps' public sources and belong to
their publishers.

Featured shows SideStore's and AltStore's app icons by linking to them in
their projects' GitHub repositories; they are not copied here.
