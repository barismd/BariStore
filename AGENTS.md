# AGENTS.md: BariStore

## Goal

An app store for iPhone/iPad that runs in **Open Minis** (1.16+), one device,
no computer. One command: `baristore` opens the store page (minis-open sheet).
iOS/iPadOS 27+ only. Made for free Apple IDs; a paid team is told apart
(listTeams memberships, as AltSign reads them; `team` file's third line
"free"/"paid") and the free limits (10 App IDs, 3 apps, 7 days) are neither
applied nor shown for it (owner, 1.1.1).

## Decisions (owner)

- Everything in **Rust**, one static aarch64-musl binary. Our own code: read
  behaviour and protocols from the reference projects, write original code; MIT
  sources (idevice, isideload) may be read as code; AGPL/GPL/non-commercial
  projects only for behaviour and docs. Record sources in THIRD_PARTY_NOTICES.md.
- Platform is Open Minis, not a-Shell: a-Shell cannot publish the
  `_remotepairing-pairable-host._tcp` Bonjour service (device-proven: no
  NSBonjourServices entry, no multicast entitlement, raw announcements never
  showed an entry on iOS 27.2.1), Minis does (`minis-bonjour advertise`).
- Store spec: Featured (search, category chips, a two-column grid of short
  tiles: icon, name, two lines of text, GET; Add IPA, Sources with "add" on
  top; owner, 1.1.0: no screenshots in tiles (they loaded slowly; short
  tiles show more apps), screenshots only on the app's page, installed apps
  not listed (a search still finds them);
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
  under the X App ID"), Options (Apple ID with Certificates: list, revoke, import,
  export .p12, and Anisette server: Automatic, or Select Anisette Server:
  one of BariStore's servers or one the user adds (plain HTTP only after a
  "not secure" warning), also behind a small gear on setup's Apple ID step;
  pairing export through the share
  sheet; info, theme, Clear cache and Clear stored IPAs, reset, install
  defaults: Increased Memory Limit, remove extensions). Install sheet: "Extensions too" (reuse free App
  IDs under a reused one) is on offer only when the chosen App ID has as many
  free App IDs under it as the app has extensions (counted from the IPA's
  zip directory). Extensions cannot take another app's App ID: iOS requires
  an extension's identifier to start with its app's. JIT (owner,
  2026-11-21): built in `src/device/jit.rs`.
- Owner, 1.1.0: App icon (Choose a picture, Reset) and Custom App ID on the
  install sheet; Install to LiveContainer on it (store app and added IPA,
  not on LiveContainer's own page), shown only when LiveContainer is on the
  iPhone: BariStore installed it, or the iPhone's app list (asked when
  BariStore connects and with the App IDs) has a com.kdt.livecontainer… or
  an app called LiveContainer; My Apps › ••• › Turn on Increased Memory
  Limit, Change icon, Use its own icon again.
- Custom App ID: what is typed (letters, digits, '-', dots, one dot at
  least) with ".TEAM" appended as BariStore's own are: an App ID belongs to
  one developer everywhere. Apple refusing it says so.
- App icons the user picks: the page makes the picture square (its
  middle), 512 px, on white (iOS shows see-through parts black) and sends
  it to /api/upload-icon; BariStore checks it decodes and keeps it in
  app-icons/ (as `options.icon`, so updates and re-signs keep it; files no
  installed app names go after an hour). Signing writes
  BariStoreIcon60x60@2x/@3x and 76/83.5@2x~ipad PNGs into the app, names
  them in CFBundleIcons(~ipad) CFBundlePrimaryIcon and drops
  CFBundleIconName (the asset catalog's icon) and CFBundleIconFile(s);
  alternate icons stay. Untested on device. `custom_icon` records the one
  the app on the iPhone has (My Apps shows it, "custom:<file>").
- Increased Memory Limit (and an icon) after the install (owner, 1.1.0:
  "no re-download, no re-picking the IPA, no cache"): iOS takes the
  entitlement from the app's code signature, never from its profile
  (GetMoreRam's README: enable it, then reinstall), and no service gives
  the installed bundle back, so a refresh cannot add it. Kept in the app's
  options either way; with its IPA still in ipas/ (added IPAs while
  installed, others for an hour) the app is signed again at once over
  itself; without it, it comes with the next update (or Install again),
  nothing is downloaded for it. An update and a full refresh keep
  Increased Memory Limit once the app has it (`signing_options`); Replace
  on the install sheet starts from the copy's options (memory, icon,
  extensions; owner, 1.1.0: an update turned memory off).
- Install to LiveContainer: `livecontainer://install?url=` (LiveContainer
  LCAppListView.handleURL; its background URLSession downloads, ATS allows
  any load) with the source's own download address for store apps. An app
  from one of the user's AltSources goes through LiveContainer's own Sources
  (owner, 1.1.0): when LiveContainer does not list that source
  (LCAltStoreSourceURLs in its preferences, read through house_arrest), it
  gets `livecontainer://source?url=` instead and shows the source's apps,
  the app installed from there with a tap: iOS lets a background app open no
  other app, so source and install cannot both be sent (and a source it has
  already makes it show "duplicate"). Unreadable preferences: installed
  directly. An
  added IPA is served once from BariStore at
  http://127.0.0.1:<port>/share/<128-bit random>/<file> (no page token,
  web.rs `Share`), under the keep-alive, until one request got it all (2
  min to start, 1 min without progress). Untested on device.
- Install sheet App IDs (owner, 1.0.2: a deleted app's own App ID showed
  "New App ID"; a newer build of an added IPA became a second app), best
  first: Replace each copy BariStore installed of the same app (original
  identifier; goes over it as an update, its App IDs and data stay; button
  "Update"/"Install again"); its own App ID (identifier.TEAM) when it
  exists: free, "Its own App ID" and no "New App ID"; installed under it
  another way, Replace; another app reused it, "New App ID" first and its
  own one with "installing replaces X"; then App IDs of apps no longer on
  the iPhone. "New App ID" never takes an app's place on the iPhone: when
  an app is installed under identifier.TEAM, the app gets identifier2.TEAM
  (3…). Unknown case: a BariStore record whose app was deleted outside
  BariStore and whose App ID another tool reused still offers Replace.
- Recommended lists only apps not installed (an install's store key, or
  the original identifier for added IPAs).
- An added IPA's icon comes from the IPA ("ipa:<file in ipas/>" icon
  address): the biggest PNG in the app that Info.plist names, Xcode's CgBI
  PNGs (BGRA, premultiplied, raw deflate) turned into plain ones. Apps with
  their icon only in Assets.car show the letter.
- Cache (owner, 1.0.2: it piled up): after every task, at start and on a
  catalog refresh with no task running, ipas/ keeps only IPAs installed
  apps need and files from the last hour (an open install sheet, a retry);
  Options has two buttons, each with its size and a warning (owner, 1.1.1):
  Clear cache (pictures, the catalog copy, IPAs no installed app is signed
  from) and Clear stored IPAs (the ones installed apps are signed from,
  added ones too; owner, 1.1.0: the one just installed stayed with its
  size); files of the last hour stay while a task runs;
  pictures of apps no source lists go after each catalog refresh, and the
  oldest beyond 80 MB; baristore.log and server.out are cut to their last
  512 KB past 2 MB (they only ever grew).
- Install, refresh, JIT and update run under the same silent-speech
  keep-alive as pairing (owner: a precaution, not a feature).
- Versions: 1.0.0 is the first stable release (history squashed). CI builds
  every push to `main` that touches the code into `v<version>`: the latest
  release when stable (install.sh and `baristore update` download it), a
  prerelease for x.y.z-alpha.N. No nightly (owner, 1.0.0). A version built
  again replaces its release and tag (owner, 1.1.1 was published again:
  "the old one must not stay").
- Website: `docs/` on GitHub Pages (main, /docs):
  https://barismd.github.io/BariStore/, with `docs/install.sh` (the one-line
  install) and the screenshots README shows.
- Never sign the user out on our own. Never store the password.
- No `baristore doctor` command: Options › Log (quick) and Run checks (network,
  tunnel, sign-in probe) in the page.
- Code, comments, commits, README in English. The owner talks Turkish.
- README and website (owner, 1.0.1): for iPhone and iPad; say what BariStore
  does, never "the way SideStore/StikDebug… does it". The page and
  BariStore's messages say "device", never "iPhone" (owner, 1.1.1: an iPad
  read "iPhone" everywhere; "device is enough").
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
  docs, and the owner's a-Shell build revealed it this way).
- Tunnel TCP (as jktcp, idevice's device-proven stack): MSS = min(MTU from the
  CDTunnel handshake, 8192) - 60, window scale option sent (shift 0) and the
  peer's honoured, at most 1 MiB in flight; AFC writes 1 MiB per request.
- Upload speed (owner, 1.0.1: 160 MB took about a minute): the tunnel's
  TLS (AES-256-CBC + HMAC-SHA384) is CPU-bound on the emulated CPU, and
  the aes crate without AES instructions computes four blocks per call, so
  CBC one record at a time wasted three quarters: records are now
  encrypted eight side by side (`cbc_lockstep`, 3.2× in software), and
  aarch64 builds enable `--cfg aes_armv8` (AES instructions when AT_HWCAP
  reports them, as ring already does for HTTPS; else the same software).
  The log says "install: sent N MB in X s". Untested on device.
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
  A server that ends otherwise is logged with its status ("supervisor: the
  server stopped (signal: 6 …)"; 1.1.1 only told the terminal, so the log
  showed restarts without a cause). A task's title and retry are in
  `~/.baristore/task-running` while it runs: the next server shows a task
  the last one stopped during as failed, with Try again (1.1.1's page just
  reconnected to a server that knew of no task).
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
  Memory: IPAs are streamed to disk, read from their file while signing
  (`bundle::Source`: a handle and 256 KiB buffer per reading thread), only
  Mach-O/plist entries kept in memory, the signed IPA written to
  `~/.baristore/signed.ipa` and sent from it 1 MiB at a time (removed once
  sent; a leftover goes with `prune_files`), a cached IPA hashed 1 MiB at a
  time. 1.1.1 held the whole IPA, then the whole signed one, in memory
  (Reddit report, 2026-11-27: Critical Ops Revival on an iPhone 13 Pro Max
  never installed: "memory allocation of 20320 bytes failed" in server.out,
  the server hung in "Reading the app", every new server aborted for minutes;
  a 663 MB synthetic Unity-like IPA peaked at 1656 MB, now 322 MB, linear
  in its Mach-O files only). Minis's kernel (ish-arm64 kernel/mmap.c,
  "memory brake") refuses every guest allocation while Minis is within 10%
  of its iOS memory limit, until it is 15% below. Signing, packing, RSA key
  generation and hashing a cached IPA run with `block_in_place`; icon and screenshot decoding is
  bounded, one icon and two screenshots at a time (the prefetch takes one
  turn at most), and a picture being made is not made again for a second
  request (the page and the prefetch asked for the same ones); one shared HTTP client
  (TLS handshakes are slow on the emulated CPU); every app's icon is made
  ready after each catalog refresh; two tokio workers.
  The page shows Featured 30 tiles at a time (the next 30 as the end comes
  near, the first 30 again on a new category or search; 12 before the
  tiles got short) and loads pictures a few at a time, those near the
  screen first, each with a spinner until it is there (owner, 1.0.0:
  pictures came slowly with every one asked at once): icons 8 at a time,
  screenshots 3.
  Pictures it showed once skip that queue when the page is drawn again
  (owner, 1.0.1: the new tiles loaded slower; each category switch sent
  every picture through the queue again).
- Wi-Fi cannot be read in Minis. A connect to 10.7.0.1 that times out means no
  route (mobile data or no VPN): say so, never knock 16 000 ports then
  (siboot's preflight); a refused port does mean "look on other ports". The
  probe takes three tries on new sockets (2.5 s each): back in the
  foreground the first one went nowhere and a retry seconds later connected
  (owner's log, 1.1.0). Untested on device.
- Sources (owner, 1.1.0: "could not reach the source" within a second of
  opening, a restart fixed some): the error shown was the previous run's,
  saved in sources.json; errors are no longer read back, the page hides them
  while a fetch runs, a GET that got no answer (or a 5xx) is tried again
  after 1 s and 3 s (sources, the picks' GitHub API, icons), and the log
  names the cause (DNS, refused, reset, TLS), not only reqwest's "error
  sending request". Untested on device.
- Log lines are one write each under the log's lock (writeln! made two and
  two threads' lines ran into each other); "Reading the app" and "Signing n
  of m" are progress, not log lines.
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
- Apple sign-in, checked live 2026-11-27 with a made-up account (Options ›
  Run checks repeats the check): through the macOS-library servers Apple
  takes the client they advertise (`<MacBookPro13,3> <Mac OS X;10.12.6;16G29>`,
  UA `akd/1.0 CFNetwork/811.5.4 Darwin/16.7.0`, iloader 2.4.2) and a current
  Mac spelt `<MacBookPro18,3> <Mac OS X;26.6;25F84>`; spelt `<macOS;26.6;…>`
  (1.1.1's) it answers HTTP 503 since 2026-11-19 (iloader #1203/#1205), so
  1.1.1 only ever signed in as the old Mac. A Windows claim fails
  provisioning there. Sign-in rotates servers × claims, current Mac first:
  as the old Mac Apple asks many accounts for a text message (secondaryAuth,
  even accounts that get codes on their devices elsewhere) and refuses to
  send it with a bare 403 (issue #2, 2026-11-27: "text-message code requested
  (403 Forbidden)", no SMS; the same in iloader, xtool, Impactor since
  October). A refused text message (403, or Apple's serviceErrors such as
  -28248) is NoCode: the other claim is tried instead of waiting for a code;
  a 412 names the phone number's real id (xtool #424); -22979/-22981 keep
  the last code. No Android-library servers (-80046). Fresh HTTP/1.1 connection per GSA
  request (isideload: reused ones get 5xx). Windows-library servers need the
  hsc 434 resync. One anisette identity per (server, client info). Email is
  lowercased. Credential codes -20101/-20970/-22406 stop sign-in.
- GrandSlam SRP: M1 hashes H(N) xor H(PAD(g)) (srp 0.7.0-rc.3
  `g_no_pad = false`); unpadded g gives -22406 for a right password (seen on
  device 2026-11-21). The session data (spd) is keyed on S, falling back to
  H(S). Pair-setup keeps unpadded g (device-proven). Known-answer test from
  the old Python sign-in in `src/srp.rs`.
- Refresh, as AltStore and SideStore do it: an app on the device, signed
  with the certificate BariStore still signs with (`cert_serial` in
  installed.json; records older than it: the certificate's notBefore is
  before the install), gets fresh profiles for its App IDs through misagent
  (`com.apple.misagent.shim.remote`, Install/Provisioning, idevice) and is
  neither re-signed nor uploaded. A task asks Apple once (`install::Shared`:
  certificate, device registered, App IDs, the device's apps, at the same
  time); refresh, Update all and the slow re-signs reuse it. Only the rest
  (a new certificate, an expired App ID) is signed again, one after another.
  Developer-services requests reuse one set of anisette headers for 30 s
  (AltStore uses one per operation). The one-pass refresh is untested on
  device (owner's 1.1.0 log: a new certificate, both apps re-signed).
- App IDs (owner, 1.1.1: "renew them only when needed"), as AltStore and
  SideStore do (FetchProvisioningProfilesOperation, read 2026-11-26): an App
  ID is the app's own identifier + "." + team (extensions under it), kept
  per installed app; every install and refresh uses the team's App ID when
  it exists and registers one only when it is missing (expired: App IDs end
  7 days after they were made, a refresh renews profiles, never App IDs),
  only for apps on the device. Apps deleted on the device are forgotten
  (AltStore deletes those records at launch): a refresh, the App IDs list and
  the page's connection check drop records the device's app list lacks (not
  while a task runs, never on an empty list), so no refresh installs a
  deleted app again (1.1.0 did: it signed every record it could not refresh).
  Another device with the same Apple ID shares the App IDs: "Not on this
  device" in the list may be in use there.
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
- Install order (owner, 1.1.1: optimise): the IPA is read on a thread of its
  own while `install::Shared` is asked for; the app group is looked up (made)
  once; the app's and its extensions' App Groups, Increased Memory Limit and
  profiles are asked for at the same time (AltStore does extensions side by
  side); a cached tunnel's liveness check also gives Developer Mode's state.
  Untested on device.
- Signing speed (owner: a 140 MB app took very long): the IPA is unpacked
  and hashed on up to four threads, page hashes are computed on several
  threads, and the final check compares only the Info.plist/CodeResources
  slots (it hashed every page a second time).
- Speed, 1.1.0 (owner: "Reading" of an added IPA took long, signing too):
  Minis's emulator (OpenMinis/ish-arm64) runs the SHA-1/SHA-256/SHA-512 and
  CRC32 instructions natively (asbestos crypto.S) but reports HWCAP 0x11f
  (FP, ASIMD, AES, PMULL, atomics only), so every hash ran in emulated
  software. The aarch64 build enables `+sha2,+crc` (.cargo/config.toml:
  ring's SHA-256, crc32fast's zip CRC32) and src/hash.rs has SHA-1/256/384
  on the instructions (SHA-512 chosen at run time: HWCAP, or uname's
  "-ish"), same digests as RustCrypto (tests, qemu-aarch64): the IPA's
  files, page hashes, download checks, the tunnel's HMAC-SHA384. flate2
  inflates with zlib-rs (about 2× miniz_oxide under qemu). A signature is
  one pass: its size is known first (the CMS has the same size whatever it
  signs), so pages are hashed once and one RSA signature made (2–4 before).
  Binaries of one bundle depth are signed side by side and hashed for
  their parent's seal there; new entries are compressed side by side (one
  small zip each, copied in raw). The task shows "Reading the app: N%".
  zip directories are read through a BufReader (each read was a system
  call). On a synthetic 113 MB app under qemu: 8.6 s → 5.5 s; the archive
  matches 1.0.2's but for the re-signed binaries. Untested on device.
- JIT's first run asks before downloading the disk image (~17 MB, sizes from
  the server's Content-Length). Owner, alpha.11: Husk (universal.js) got
  "JIT is on"; the result texts are "JIT is on for X." / "X started with
  JIT."
- Free teams: 10 App IDs at a time (SideStore counts the same); listAppIds
  gives each one's expiry. The install sheet picks "new" or an App ID whose app
  is not on the device (installation_proxy Browse); when the IDs run out
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
  password: Options › Apple ID › Certificates › Export gives both. A task
  that signed LiveContainer with a certificate it was not given yet
  (`lc_cert` in installed.json) sends it at its end (owner, 1.1.1: after a
  new certificate LiveContainer kept the revoked one); on the first app (the
  Apple ID not trusted yet) a button offers it instead. The other kinds take
  the certificate inside their signed bundle (ALTCertificate.p12), StikDebug
  only the pairing file. Untested on device.
- Development certificates: a free Apple ID has room for one (7460 "You
  already have a current iOS Development certificate"; AltStore revokes one
  whenever it holds none of the team's keys). Owner, 1.1.0: BariStore on an
  iPhone and an iPad with one Apple ID revoked each other's certificate
  (1.1.0 revoked every "BariStore" certificate on 7460), so the other
  device's apps showed "Not Verified" until refreshed. Now nothing is revoked
  without asking (SideStore asks too); a BariStore certificate this device
  has no key for is named as another device's, with "Import that device's
  certificate instead". Options › Apple ID › Certificates › Import takes the
  .p12 Export made there (PBES2: PBKDF2-HMAC-SHA1/256, AES-128/192/256-CBC;
  ours and OpenSSL 3's; older 3DES/RC2 files are refused with a word), checks
  the key matches, that the team lists the certificate, and keeps its key as
  signing-key.der (AltServer keeps one certificate per team the same way).
  Owner, 1.1.1 asked for a certificate per device, else one shared without
  Import, "if possible": a team with room (a paid one) already gets a
  certificate per device without a question; a free one has room for one.
  Sharing it by itself is not possible: the key never leaves the device that
  made it (Apple keeps only the certificate) and BariStore has no server;
  deriving it from the password would make the key file a password oracle.
  So Import stays. One certificate does work on several devices: each one
  registers itself and gets a profile listing it (AltServer signs for every
  device with one).
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
Linux-verified for 1.1.2: a 663 MB synthetic IPA (150 MB framework, 512 MB
of assets) signed at 322 MB peak (1.1.1: 1656 MB), the archive the same as
1.1.1's but for the re-signed binaries; the file reader against the file's
bytes; Apple taking the current Mac spelt "Mac OS X" through five servers
and refusing it spelt "macOS" (HTTP 503, new.ani.sidestore.io), live with a
made-up account; the parsing of /auth's phone numbers and Apple's
serviceErrors. Untested on
device: the text-message code under the current Mac, the upload from the
file, the stopped-task sheet.
Linux-verified for 1.1.1: the .p12 reader on ours and OpenSSL 3's (wrong
password refused), a lost connection tried again, free/paid teams from
listTeams' memberships; the page in Playwright against a mock server
(no "iPhone" left, paid team without limits, free team's limits, source
errors hidden while fetching, Certificates › Import sends the file and its
password, LiveContainer's button after a task, Clear cache and Clear stored
IPAs with their sizes and warnings); the real server's two clearings on a
temp home (stored IPAs kept by the one, the only ones deleted by the other).
Linux-verified for 1.1.0: hashes against RustCrypto (x86_64; aarch64 under
qemu, instructions and software alike); a one-pass signature (load command,
page hashes, special slots, CMS verified by OpenSSL); an app with a
framework and a loose dylib signed, given a picture as icon, packed and read
back (modes, CodeResources hashes, icon sizes, Info.plist); a share served
whole and gone once dropped; the real server's icon upload (bad picture
refused) and custom: icon; the page in Playwright against a mock server
(Custom App ID with the team appended, icon chosen and sent, Replace
starting with memory on, Install to LiveContainer offered and sent, hidden
without LiveContainer and shown for one only in the iPhone's app list,
Turn on Increased Memory Limit and Change icon with and without the IPA
kept, Clear cache's words, short tiles without screenshots, installed apps
only in search, 30 then 60 tiles).
Linux-verified for 1.0.2: the page in Playwright against a mock server
(App ID choices: own free, own reused by another app, an installed copy,
New on request, an added IPA over its installed copy, its icon in the sheet
and My Apps, Recommended without installed apps, pictures back at once
after a category switch); the real server's upload answer and its ipa:
icon (largest named PNG, path checks); CgBI conversion; TLS records sealed
side by side against the cbc crate and OpenSSL s_server; log trimming.
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
`src/hash.rs` SHA-1/256/384 on the CPU's SHA instructions ·
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
