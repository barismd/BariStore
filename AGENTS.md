# AGENTS.md: BariStore

## Goal

An app store for iPhone/iPad that runs in **Open Minis** (1.16+), one device,
no computer. One command: `baristore` opens the store page (minis-open sheet).
iOS/iPadOS 27+ only. Free Apple ID accounts only.

## Decisions (owner)

- Everything in **Rust**, one static aarch64-musl binary. Our own code: read
  behaviour and protocols from the reference projects, write original code; MIT
  sources (idevice, isideload) may be read as code; AGPL/GPL/non-commercial
  projects only for behaviour and docs. Record sources in THIRD_PARTY_NOTICES.md.
- Platform is Open Minis, not a-Shell: a-Shell cannot publish the
  `_remotepairing-pairable-host._tcp` Bonjour service (device-proven: no
  NSBonjourServices entry, no multicast entitlement, raw announcements never
  showed an entry on iOS 27.2.1), Minis does (`minis-bonjour advertise`).
- Store spec: Featured (search, category chips, a two-column grid of tiles
  with short descriptions and screenshots, Add IPA, Sources with "add" on top;
  owner, 1.0.1: a tile shows its screenshots whole and upright, side by side
  (two in an iPhone's tile, three in an iPad's), landscape ones one above the
  other, a single one on a blurred copy of itself, never a cropped top;
  BariStore picks AltStore, SideStore, LiveContainer+SideStore, LiveContainer,
  stable + nightly, only plain LiveContainer under Recommended (owner,
  2026-11-22); the three stores under a "Stores" category, LiveContainer
  under Utilities; Leviidev's source (Husk…) among the defaults, a new
  default joins an existing list once (`defaults-seen`)), My Apps (an
  Updates button left of Refresh all opens a sheet: newer versions from
  each app's own source and track, Update all, a newer BariStore build, "No
  updates found." and Check for updates; installed, "x days, y hours left",
  refresh, delete, per-app options with Beta updates: Off or the source's
  tracks; memory only in the app's About; App IDs: a reused App ID shows its
  own name "used by" the app, its extensions' IDs the app's name "created
  under the X App ID"), Options (Apple ID with Certificates: list, revoke,
  export .p12, and Anisette server: Automatic, or Select Anisette Server:
  one of BariStore's servers or one the user adds (plain HTTP only after a
  "not secure" warning), also behind a small gear on setup's Apple ID step;
  pairing export through the share
  sheet; info, theme, reset, install defaults: Increased Memory
  Limit, remove extensions). Install sheet: "Extensions too" (reuse free App
  IDs under a reused one) is on offer only when the chosen App ID has as many
  free App IDs under it as the app has extensions (counted from the IPA's
  zip directory). Extensions cannot take another app's App ID: iOS requires
  an extension's identifier to start with its app's. JIT (owner,
  2026-11-21): built in `src/device/jit.rs`.
- Install, refresh, JIT and update run under the same silent-speech
  keep-alive as pairing (owner: a precaution, not a feature).
- Versions: 1.0.0 is the first stable release (history squashed). CI builds
  every push to `main` that touches the code into `v<version>`: the latest
  release when stable (install.sh and `baristore update` download it), a
  prerelease for x.y.z-alpha.N. No nightly (owner, 1.0.0).
- Website: `docs/` on GitHub Pages (main, /docs):
  https://barismd.github.io/BariStore/, with `docs/install.sh` (the one-line
  install) and the screenshots README shows.
- Never sign the user out on our own. Never store the password.
- No `baristore doctor` command: Options › Log (quick) and Run checks (network,
  tunnel, sign-in probe) in the page.
- Code, comments, commits, README in English. The owner talks Turkish.
- README and website (owner, 1.0.1): for iPhone and iPad; say what BariStore
  does, never "the way SideStore/StikDebug… does it". The page says "iPad"
  on an iPad (`DEVICE`, BariStore's own messages through `dev()`).
- Device tests come in **batches** the owner runs; keep their effort low.

## Facts we rely on (and where they come from)

- Minis keeps running in Settings while it plays audio. `apple-player` (siboot;
  proven the same way for a-Shell's `play`) covers the screen with a sheet the
  owner does not want; `apple-speak speak --volume 0` uses the same audio
  session as Minis's own keep-alive (playback, mix with others) with no screen,
  so pairing uses it, repeated (each call returns after the speech or 60 s).
  Untested on device; a failed pairing is retried with the player.
- The "Pair with …" entry asks for the passcode, then sends pair-setup M1; the
  code is made and notified at M1 (owner: an earlier notification is gone by then).
- Developer Mode may be missing from Settings on a phone that never had it:
  offering a pairing (the pairable-host advert) makes iOS show it (Apple's
  docs, and the owner's a-Shell build revealed it this way). After pairing,
  AMFI action 0/1 shows or turns it on.
- Tunnel TCP (as jktcp, idevice's device-proven stack): MSS = min(MTU from the
  CDTunnel handshake, 8192) - 60, window scale option sent (shift 0) and the
  peer's honoured, at most 1 MiB in flight; AFC writes 1 MiB per request.
- Minis's kernel (OpenMinis/ish-arm64, fs/sock.h) takes SO_RCVTIMEO/
  SO_SNDTIMEO only as 66/67; a 64-bit musl program sets 20/21 and gets EINVAL,
  so std's `set_read_timeout` fails there: no socket timeouts, wait on a
  channel instead. Its exec leaves the process's other threads running
  (device-proven 2026-11-22: after `baristore update` exec'd, the old tokio
  workers panicked "Bad file descriptor"): BariStore never execs.
- `baristore` is a supervisor: it runs the server as a child
  (BARISTORE_SERVE="port token") and restarts it when it dies; iOS takes
  memory from Minis in the background and Rust aborts on a failed allocation
  (seen on device). Exit 0 = quit, 75 = updated (the supervisor starts the
  new file at the same address). The page reloads when the server's boot id
  changes. The server runs in its own process group (whatever ends the
  terminal's group leaves it running), the supervisor ignores SIGHUP, never
  panics on a closed terminal, retries a failed spawn and sleeps when stdin is
  at its end (it spun at 100 % CPU before). It pings GET /api/ping on a
  thread of its own (5 s wait, one at a time) and restarts a server that
  answered once and then not for 60 s (gaps from a suspended Minis do not
  count). 0.10.0-alpha.9's ping set SO_RCVTIMEO, never got an answer on the
  device and killed a healthy server every ~45 s (owner: installs stopped
  mid-way, the idle store closed); hence "answered once" and the long wait.
  Typed in the terminal while it runs: Return quits, `log` prints the log,
  anything else gets a hint (the owner typed `baristore log` there and it
  quit). `baristore log` prints the same report from a shell.
  The server also asks its own address every 3 s: two misses rebuild the
  listener, four (and no task running) end the server so the supervisor
  starts a new one (owner, alpha.10: back from JIT the page stayed on
  "Reconnecting" until it said stopped).
  `~/.baristore/running` ("port token server supervisor") lets a new
  `baristore` show the page of a running one or take over a server that
  outlived its terminal. The server rebinds a listener
  that only fails (iOS takes a suspended app's sockets); the page's requests
  time out (8 s state, 120 s actions) so a hung server shows "Reconnecting".
  The server's own stdout/stderr go to `~/.baristore/server.out` (Options ›
  Log shows its tail). Minis's agent shell kills a command's process group
  after 300 s (ISHExecutionCoordinator), another reason for the own group.
  Memory: IPAs are streamed to disk and only Mach-O/plist entries are kept in
  memory while signing; signing, packing, RSA key generation and hashing a
  cached IPA run with `block_in_place`; icon and screenshot decoding is
  bounded, one icon and one screenshot at a time; one shared HTTP client
  (TLS handshakes are slow on the emulated CPU); the first 12 tiles' first
  two screenshots are made ready after each catalog refresh; two tokio
  workers.
  The page shows Featured 12 tiles at a time (the next 12 as the end comes
  near, the first 12 again on a new category or search) and loads pictures
  3 at a time, those near the screen first, each with a spinner until it is
  there (owner, 1.0.0: pictures came slowly with every one asked at once).
- Wi-Fi cannot be read in Minis. A connect to 10.7.0.1 that times out means no
  route (mobile data or no VPN): say so, never knock 16 000 ports then
  (siboot's preflight); a refused port does mean "look on other ports".
- The page's origin is stable per run, but nothing it must remember lives in
  localStorage: settings.json holds theme and setup_done.
  WKWebView in Minis shows no confirm()/alert(): the page uses its own sheets.
- Minis's browser cancels navigations that are not http(s)/about/blob/minis
  (BrowserUseManager), so app URLs go through `apple-open <url>` (OpenOffload:
  UIApplication.openURL). Files reach the user through the Web Share sheet
  (`navigator.share` with a File, from a tap), else a download: the terminal's
  `/var/minis/shared` is the Files app's Minis › shared only once Minis's
  agent bind-mounted it (owner saw a plain rootfs folder).
- The pairing service is usually on 49152 but can be another dynamic port
  (siboot 0.19.1): knock 49153–65535 except 62078.
- iOS 27.2+: every RemoteXPC handshake on a tunnel must present the same host
  UUID (idevice); we keep one per install (`~/.baristore/host-uuid`).
- SideStore nightlies read `Documents/PairingFile_RemoteRP.plist` only with
  prefs `isPairingReset=false`, `activePairingProtocol=rppairing`; LiveContainer's
  SideStore reads `SideStore/Documents/ALTPairingFile.mobiledevicepairing`;
  AltStore 2.3.1+ reads `Documents/ALTPairingFile.plist` (SideInstaller docs).
- Apple sign-in, checked live 2026-11-21 with a made-up account (`baristore
  doctor` repeats the check): through the macOS-library servers Apple takes
  the client they advertise (`<MacBookPro13,3> <Mac OS X;10.12.6;16G29>`, UA
  `akd/1.0 CFNetwork/811.5.4 Darwin/16.7.0`, iloader 2.4.2) and answers a
  current Mac with HTTP 503; a Windows claim fails provisioning there. Sign-in
  rotates servers × claims (advertised, then current Mac for -23782). No
  Android-library servers (-80046). Fresh HTTP/1.1 connection per GSA
  request (isideload: reused ones get 5xx). Windows-library servers need the
  hsc 434 resync. One anisette identity per (server, client info). Email is
  lowercased. Credential codes -20101/-20970/-22406 stop sign-in.
- GrandSlam SRP: M1 hashes H(N) xor H(PAD(g)) (srp 0.7.0-rc.3
  `g_no_pad = false`); unpadded g gives -22406 for a right password (seen on
  device 2026-11-21). The session data (spd) is keyed on S, falling back to
  H(S). Pair-setup keeps unpadded g (device-proven). Known-answer test from
  the old Python sign-in in `src/srp.rs`.
- Refresh, as AltStore and SideStore do it: an app still on the iPhone,
  signed with the certificate BariStore still signs with (`cert_serial` in
  installed.json; records older than it: the certificate's notBefore is
  before the install), gets fresh profiles for its App IDs through misagent
  (`com.apple.misagent.shim.remote`, Install/Provisioning, idevice) and is
  neither re-signed nor uploaded. Refresh all does every app in one pass:
  certificate, device, App IDs and the iPhone's apps asked for at once, all
  profiles at the same time; only the rest (a new certificate, a missing App
  ID) is signed again, one after another. Developer-services requests reuse
  one set of anisette headers for 30 s (AltStore uses one per operation).
  Owner, alpha.12: refresh still re-signed (records without `cert_serial`);
  the one-pass refresh is untested on device.
- Store app versions: an AltSource's `versions[0]` (channel "") and the beta
  tracks of SideStore's format (`releaseChannels`: track + releases, newest
  first; "stable" is the plain version). Picks: "stable"/"nightly". An
  installed app's `channel` is the track Updates follows.
- An IPA's extensions are counted before downloading it from its zip
  directory with range requests: first `bytes=0-0` for the size, because
  GitHub's release storage answers suffix ranges (`bytes=-N`) with 501.
- Anisette server choice: settings.json `anisette` ("" = automatic); a chosen
  server is the only one sign-in uses, checked (/v3/client_info) when picked
  or added (a pick is saved at once, no Save button); the user's own servers
  are `anisette_servers` (one chosen in 1.0.0 counts as added).
- Add IPA (owner, 1.0.1: on an iPad a 150 MB IPA showed nothing at all): the
  file input stays in the page until it answers (iOS can drop a detached one
  and its change event while it copies a big file out of Files); a sheet
  shows from the tap ("cancel" closes it), then MB sent, time left and
  "Reading the app…", with Cancel. The server reads the body in 4 MiB pieces
  and writes each with `block_in_place`.
- Signing speed (owner: a 140 MB app took very long): the IPA is unpacked
  and hashed on up to four threads, page hashes are reused across the
  signature-size rounds and computed on several threads, and the final check
  compares only the Info.plist/CodeResources slots (it hashed every page a
  second time). Output is byte-identical to before (CodeDirectory, code,
  CodeResources compared on a zig-built arm64 test app).
- JIT's first run asks before downloading the disk image (~17 MB, sizes from
  the server's Content-Length). Owner, alpha.11: Husk (universal.js) got
  "JIT is on"; the result texts are "JIT is on for X." / "X started with
  JIT."
- Free teams: 10 App IDs at a time (SideStore counts the same); listAppIds
  gives each one's expiry. The install sheet picks "new" or an App ID whose app
  is not on the iPhone (installation_proxy Browse); when the IDs run out
  BariStore asks which one to reuse. Extensions only take ones under the
  app's identifier. The profile is checked for Increased Memory Limit.
  Installed.ids keeps the identifiers so refreshes keep them.
- StikDebug (com.stik.*) reads Documents/pairingFile.plist.
- What an app is (SideStore, AltStore, LiveContainer, LiveContainer+SideStore,
  StikDebug) comes from its IPA (main identifier, SideStoreApp.framework),
  recorded as `kind` in installed.json, never from the identifier it was
  installed under: App IDs are reused for other apps (owner installed JITProbe
  under LiveContainer's and got LiveContainer's buttons).
- Plain LiveContainer reads no pairing file (its built-in SideStore is
  Frameworks/SideStoreApp.framework, only in LiveContainer+SideStore). Its
  JIT-less certificate is imported from
  `livecontainer://certificate?cert=<base64 p12>&password=<pw>`
  (LCSettingsView.handleURL, SideStore's export callback): stored in its app
  group's defaults, which house_arrest cannot reach. Its ZSign opens the
  .p12 with OpenSSL 3 PKCS12_parse (our PBES2/AES-256 + HMAC-SHA256 p12 is
  fine; checked with OpenSSL 3.0 here). Manual import asks for a .p12 and its
  password: Options › Apple ID › Certificates › Export gives both.
- Development certificates: when submitDevelopmentCSR answers 7460 and none
  is BariStore's, the install asks which one to revoke (no more dead end).
- JIT as StikDebug does it: iOS 26.4+ installs the DDI as a cryptex through
  cryptexd (read-personalization-id, get-nonce with the identity's
  Cryptex1,NonceDomain, a Cryptex1 ticket from TSS gs.apple.com, install with
  five XPC file transfers), files from doronz88/DeveloperDiskImage
  Xcode_iOS_DDI_Cryptex (manifest checked 2026-11-21). Then CoreDevice
  appservice launches the app stopped, debugserver attaches; apps StikDebug
  lists for universal.js / legacy.js get brk #0xf00d / #0x69 served (16 KiB
  pages each touched with M addr,1:69), others detach at once. universal.js's
  "new breakpoints" command (app-supplied JavaScript) is not supported.
  On TXM iPhones (iOS 26+) attach/detach only marks the app debugged:
  Whetstone's JITProbe (RWX, RW→RX, MAP_JIT tests) showed "JIT off" for the
  owner (2026-11-22), as expected for an app that sends no brk requests; the
  result text says so. Not yet known whether its CS_DEBUGGED read true.
  Owner, alpha.10: after JIT, JITProbe's "re-run safe test" did not respond:
  an app launched stopped stayed frozen after attach + detach, so apps that
  are only attached and detached are now launched running.
- SideStore/AltStore refresh with the certificate in their bundle:
  `ALTCertificate.p12` (password = portal machineId) + `ALTCertificateID` =
  serial (SideStore CertificateProvisioningFlow; isideload). LC+SS: in the
  `com.SideStore.SideStore` framework. AltStore also gets `ALTDeviceID`.
- Each app extension needs its own App ID/profile, identifier under the app's.

## Verified vs not

Linux-verified: SRP pair-setup against idevice-srp, GrandSlam SRP against the
device-proven Python sign-in (known answer), pairing host against a
mock iPhone, TLS-PSK against openssl s_server, TCP stack against smoltcp, tunnel
against a mock device, CSR and PKCS#12 with openssl (3.x and 1.1.1), signatures
with rcodesign. **Device-verified (owner):** LocalDevVPN check, pairing (0.1.0); Apple sign-in
with 2FA, signing and installing an app (0.10.0-alpha.3).
Everything else: say "untested on device" until the owner confirms.
Linux-verified for 1.0.1: the page in Playwright against a mock server (tile
strips at 375/390/820 px with portrait, landscape, single and mixed
screenshots, Add IPA picking/cancel/upload/cancel, the Anisette sheet), the
upload endpoint with a 150 MB IPA (byte-identical, 1.1 s on Linux).
Linux-verified for 1.0.0: the page in Playwright against a mock server (Updates
sheet, Check, Beta updates, Extensions too, anisette sheet and gear, App ID
labels), the zip-directory count on LiveContainer's real IPA (3 extensions).
Linux-verified for 0.10.0-alpha.10: the supervisor (crash, a server that
never answered is left alone, one that hangs after answering is replaced,
SIGHUP, takeover, Return, `log`, Ctrl-C, Options › Quit; strace: no socket
timeouts), the page in Playwright (all views, hung server), the LiveContainer
URL encoding. Owner on 0.10.0-alpha.9 (device): LiveContainer installed on a
later try; everything else failed by the watchdog bug above.

## Layout

`src/pairing` pairable host · `src/device` tunnel, TCP, XPC, services ·
`src/apple` anisette, GrandSlam, portal · `src/sign` code signing ·
`src/install.rs` App IDs → profiles → sign → install → pairing file ·
`src/store.rs` catalog · `src/icons.rs` icon cache · `src/app.rs` state/tasks ·
`src/web.rs` + `ui/index.html` the page · `src/doctor.rs` report ·
`src/update.rs` self-update · `docs/` the website and `install.sh`. CI:
`.github/workflows/release.yml` publishes the releases.

## Rules

- No dead code, no unused options, no extra files; `cargo build --release` must
  have zero warnings; `cargo test` must pass.
- Never commit secrets, pairing records, keys, profiles or IPAs.
- Bump `version` in Cargo.toml on every functional change.
