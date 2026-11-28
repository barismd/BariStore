# AGENTS.md: BariStore

## Goal

An app store for iPhone/iPad that runs in **rootshell** (kitknox/rootshell,
App Store, 1.0.16-177+; 178+ for the Shortcut), one device, no computer. The
**BariStore** Shortcut shows the store page over rootshell (rootshell's
BariStore profile runs `baristore page`); without it, `baristore` (a function `baristore install` puts in
~/.rootshellrc) opens it in Safari. Installed with one line,
`sh -c "$(curl -fsSL https://raw.githubusercontent.com/barismd/BariStore/test/docs/install.sh)"`
while it is a test build (the website's address,
https://barismd.github.io/BariStore/install.sh, once 1.2.0 is released on
main). iOS/iPadOS 27+ only. Made for free Apple IDs; a paid team is told
apart (listTeams memberships, as AltSign reads them; `team` file's third line
"free"/"paid") and the free limits (10 App IDs, 3 apps, 7 days) are neither
applied nor shown for it (owner, 1.1.1).
## Decisions (owner)

- Everything in **Rust**, one WebAssembly program (wasm32-wasip1,
  `baristore.wasm`, ~4 MB) for rootshell's WASI runtime. Our own code: read
  behaviour and protocols from the reference projects, write original code; MIT
  sources (idevice, isideload, rootshell, siboot) may be read as code;
  AGPL/GPL/non-commercial projects only for behaviour and docs. Record sources
  in THIRD_PARTY_NOTICES.md.
- Platform is rootshell (owner, 2026-11-27: Apple removed Open Minis from the
  App Store; ported on branch `test`). Its builds are **1.2.0** (owner,
  2026-11-28: "2.0.0 değil 1.2.0"; 1.2.0-alpha.N while tested). The `test`
  branch's history was squashed into one commit by the owner on
  2026-11-28, its earlier test releases deleted, and numbering started again
  at 1.2.0-alpha.1. Commits are the owner's (barismd), signed in a
  Codespace (Settings › Codespaces › GPG verification) so GitHub shows them
  Verified; none says it is Claude's (owner, 2026-11-28). Open Minis
  installs keep working on 1.1.x meanwhile; nothing published for rootshell
  touches them.
- Open Minis's end (owner, 2026-11-28), once rootshell is stable and 1.2.0
  is released on main: a last Open Minis build, **1.1.3**, with the fixes
  below, that makes an encrypted file holding everything BariStore keeps
  (session, anisette identities, signing key and certificate, pairing,
  installed apps and their App IDs, sources, settings; never the password,
  which BariStore never has; IPAs and pictures stay behind), sealed with a
  random code it shows (a passphrase through a slow KDF, AES-GCM), which
  1.2.0 on rootshell imports with that code and goes on from, losing
  nothing. Safe as long as the code never travels with the file: together
  they are what the device already holds. Then Open Minis is discontinued:
  1.1.3 gets no update; its users are told they may keep using it, that new
  versions come for rootshell for as long as Open Minis is not on the App
  Store, since running on two platforms is not easy; the Open Minis install
  line stays, pinned to 1.1.3. Open Minis builds update from
  releases/latest/download/baristore-aarch64 (stable only, so the `test`
  prereleases never reach them): once 1.2.0 is the latest release, it and
  every later one also carry 1.1.3's baristore-aarch64 and its .sha256, so
  1.1.x keeps finding 1.1.3 there.
- Users' reports to fix in rootshell's build now and in 1.1.3 (owner,
  2026-11-28): Developer Mode must be on (a Reddit user turned it off,
  reading "Developer Mode does not need to have been on", and got no "Pair
  with BariStore"): README, site and the page say it must be on, where it
  is, and that pairing makes it appear when it is not listed. An Apple ID
  made without a phone number (Apple Music for Android) was still asked for
  a code, with few ways to get one: every way Apple offers is on offer
  (each trusted number texted or called, the Apple devices, again), and an
  account with nowhere to send one is no code at once (the other client is
  tried), then told to add a number at account.apple.com or sign in once on
  an Apple device.
- Instructions (owner, 2026-11-28: "basit ve anlaşılır, kurulumdan önce"):
  README, site and the setup page start with Before you install / Before
  you start: every setting with its exact place (Developer Mode; LocalDevVPN
  Connect; rootshell › Settings › General › Location Diary Mode, Mode Auto
  During Active Sessions, and Terminal Notifications; Local Network
  access), and the extra steps; the install command prints the same.
  rootshell may show a Live Activity (its General › Live Activity) while
  BariStore runs: said to be rootshell's own. rootshell runs WASI
  Preview 1 programs in a hidden web view and lends them sockets and TLS; it
  has Apple's multicast entitlement, so the pairable host can be announced as
  raw mDNS (`src/mdns.rs`; device-proven by siboot's rootshell pairing). Before:
  Open Minis (1.x), which published the service with `minis-bonjour`; a-Shell
  could not (no NSBonjourServices entry, no multicast entitlement).
- The page shows over rootshell (owner, 2026-11-28: "Rootshell üzerinde
  çalışsın"), as atelier-install 0.4.0 does it: rootshell has no browser
  for a local page (its page tab opens only from an SSH connection's
  listening ports; OSC links and OpenURL go to Safari), but a Shortcut's
  Show Web Page shows one over the app in front. Owner, 2026-11-28: no
  random numbers for people to type; a rootshell profile the Shortcut runs;
  adding it to the Home Screen optional; tabs must not pile up. So: a Local
  Shell profile "BariStore" with the Startup Command `baristore page`, and a
  Shortcut of three actions (Open Connection Profile: BariStore, Wait 3 s,
  Show Web Page http://127.0.0.1:46510/; the page's Options › BariStore
  Shortcut and the README say how). `page` arms the store (web.rs `arm`):
  the first request for the bare address within a minute is sent on to a
  key made for it. rootshell stays in front while the page is up. Tabs:
  rootshell opens a new one for every run of a profile (or Open Local
  Shell) and has no way to reuse one or for a program to close its own
  (read 2026-11-28: `createLocalProfile` always opens a tab; `exec` is no
  command of its local shell; a script's `exit` ends only the script), so a
  run while BariStore already runs leaves one finished tab that says it can
  be closed. Only rootshell can change that (an option to focus a profile's
  open tab). Safari stays as the way without it (opened by itself until a
  Shortcut was used once). One install line for testing too (owner: "tek
  curl | sh linki"): rootshell's `sh` runs no script piped to it, so the
  line is `sh -c "$(curl …)"` (docs/install.sh; owner's device, 2026-11-28:
  the script ran). siboot-pair's own was three lines (curl -O, `ifconfig
  en0 > ifconfig.txt`, `./siboot-pair.wasm`), since rootshell ran no .wasm
  after `&&`.
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
  web.rs `Share`), while Location Diary Mode keeps rootshell running, until one request got it all (2
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
  oldest beyond 80 MB; baristore.log is cut to its last
  512 KB past 2 MB (it only ever grew).
- rootshell keeps a program running in the background only with its Location
  Diary Mode on (Settings, Auto During Active Sessions counts a WASM program
  from 1.0.16-177; without it ~25 s, `BackgroundExecutionCoordinator`): with
  the Shortcut, rootshell is in front while the page is up and in the
  background only in Settings or another app; in Safari, always while it is
  used. Setup and pairing say so; there is no keep-alive of BariStore's own
  (Minis's silent speech is gone).
- Versions: 1.0.0 is the first stable release (history squashed). CI builds
  every push to `main` (and `test`) that touches the code into
  `v<version>` with `baristore.wasm` and its `.sha256`: the latest release when
  stable, a prerelease for x.y.z-alpha.N; `test` publishes prereleases only.
  `baristore update` takes the latest published release that has a
  baristore.wasm (prereleases too when the running build is one; GitHub's
  releases API, which lists releases by version, not by date: a test
  build numbered 2.0.0 sorted above 1.2.0's, which then "updated" to it). No
  nightly (owner, 1.0.0). A version built again replaces its release and tag
  (owner, 1.1.1 was published again: "the old one must not stay").
- Website: `docs/` on GitHub Pages (main, /docs):
  https://barismd.github.io/BariStore/, with the one install line, the
  Shortcut's steps and the screenshots README shows. Until 1.2.0 is released
  on main, the site there is Open Minis's and every install line (README,
  site, the script's comment) is the `test` branch's raw address.
  `docs/install.sh` downloads the newest release's baristore.wasm (the
  newest prerelease while no stable one has it: GitHub's releases list saved
  to a file and searched with `grep -o -m 1`, as rootshell's shell runs no
  pipeline of programs; GitHub lists by version, so no release may carry a
  higher version than the newest build), runs `baristore.wasm install` (the command and the
  checklist; it checks the file against the release) and `baristore`.
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

- rootshell's WASM runtime (wasm/README.md, Resources/Wasm/wasm-worker.js,
  rootshell/Features/Wasm/*.swift, MIT, read 2026-11-27; src/rootshell):
  one thread; every call is a Worker → web view → Swift round trip that
  blocks the program until Network.framework answers; no poll, no threads
  (`thread::spawn` fails), no processes (BariStore runs no command, `std::env::
  temp_dir` and `process::id` panic). Socket calls `rootshell_socket_*`;
  `set_recv_timeout` and `bonjour_lookup` (1.0.16-177) are imported from `env`
  so an older rootshell still starts the program and is told to update
  (`has_recv_timeouts`). Answers come back through one 64 KB slot as base64 in
  JSON: a receive or file read that brings more fails with EINVAL and its bytes
  are gone, so receives ask for 32 KB and libc's read/readv/pread/write/writev/
  pwrite are BariStore's own at link time (std on wasip1 goes through them,
  checked with Rust 1.97), 32 KB in, 256 KB out (`rootshell/files.rs`).
  `accept` waits until a connection comes; a TCP `bind` takes only the port
  and the listener takes every address the device has; `getsockname` says
  0.0.0.0. TLS is rootshell's (`tls_connect_host`, Apple's trust store).
  `connect` has no time limit (a SYN nobody answers holds the program until
  TCP gives up). The file broker (FileManager) answers EIO for a folder that
  exists, makes no intermediate folders and refuses a rename onto a file that
  exists: `files::make_dirs` and `files::replace` (std's `create_dir_all`
  fails on that EIO). fds ≤ 4 are never closed; HOME=/ is rootshell's
  Documents (`~/.baristore` hidden from Files). (atelier-install, GPL, found
  and documented most of this; behaviour only.)
- The runtime (`src/rt`): tasks, timers and socket waits on the one thread,
  tokio's API shape (tokio itself only for its I/O traits, channels, locks).
  While all tasks wait it waits in rootshell: on the one socket a task reads
  for as long as no timer is due (400 ms at most), a few ms each when several
  wait, else on a UDP socket nothing is sent to; never in WASI's sleep, which
  asks the app nothing (WebKit can stop a hidden web view's work while
  nothing passes: atelier-install saw a hash stop until a key was pressed).
  Listeners are knocked on (`rt/io.rs` `Door`): connect to the port itself,
  send a word only this run knows, then accept returns at once with what came
  before the knock and the knock; every 150 ms for 3 s after a connection,
  400 ms when quiet (each knock is eight calls); a knock that cannot connect reopens the listener (iOS takes a
  suspended app's sockets). A connection's first bytes are read with the knock
  (300 ms limit) and kept. Long computations call `rt::check_in` (signing,
  hashing, packing, reading the IPA): due timers, a short look at the
  sockets and up to 32 other tasks, so the page keeps answering; the allocator
  nudges rootshell (a call that changes nothing) every 64th allocation after
  40 ms (RSA keys, pictures).
- Sends block until Network.framework took the bytes: a program must never
  send much to a connection of its own it has not read yet (tests use
  threads for such peers).
- Pairing in rootshell: the announcement (PTR, SRV, TXT with idevice's
  PairableHostInfo keys, an A record per address of this device, TTL 75 min,
  three times, again every 20 s, a goodbye when done) needs the device's
  Wi-Fi address, which rootshell does not tell a program: `baristore` saves
  `ifconfig`'s output in ~/.baristore/ifconfig.txt before it starts the
  program; failing that, its own query echoed by the mDNS group, or the only
  address Bonjour lists for `_remotepairing._tcp`. Untested on device for
  BariStore (siboot's rootshell pairing is device-proven).
- The code goes out as rootshell's notification (OSC 777 `notify`, posted in
  the background with Settings › Notifications › Terminal Notifications on);
  the page is opened with OSC 1337 `OpenURL` (http(s) only, Settings ›
  Terminal › Open Links from Programs, rootshell in front) and printed as an
  OSC 8 link. Untested on device.
- The page is in the Shortcut's web view (Show Web Page; it closes when
  rootshell goes to the background, and ✕ closes it) or in Safari: their
  own dialogs and share sheet; app URLs (livecontainer://) cannot be opened
  by a WebAssembly program, so a task asks the page (question kind "open": a
  link the user taps, iOS asks before opening the app). The store serves
  the page under its own key (`running`, Safari) and keys made for the
  Shortcut's runs (`page-keys`, the last 8, so an open page survives a
  restart); the state
  says which (`shortcut`) and the page words its hints so. It prefers port
  46510 (the Shortcut's); when another program has it, it takes another and
  a later Shortcut run asks it to serve 46510 too (`/api/arm`). Untested on
  device.
- rootshell's Open Connection Profile and Open Local Shell (Shortcuts;
  Open Local Shell's Command since 1.0.16-178, 467ab02b) open a new tab and
  type the command into it once the shell started, so ~/.rootshellrc's
  functions are there.
- rootshell holds a new tab's curl, ifconfig and mkdir (its ios_system
  commands) until a program running in another tab ends; a second WASM
  program runs (atelier-install, Alex's tests, 22 Nov 2026). So `baristore`
  (the ~/.rootshellrc function, one line: rootshell's line-by-line fallback
  reads it too) runs only the shell's own `cd` and `[` before
  `baristore.wasm --join "$@"` while ~/.baristore/live is there (written as
  a store starts, removed by Quit and an update; a store stopped by Ctrl-C
  leaves it, and --join then removes it): --join arms the running store
  for `page`, tells a typed `baristore` where it is, runs log/update/
  version itself, and ends with 1 when no store answers. Then the function
  saves ifconfig.txt, runs `baristore.wasm` from ~/bin and starts it again
  while it ends with 75 (an update). In a script or function a `.wasm` runs
  in rootshell's WASM runtime (`routeAppCommand`), not ios_system; a
  top-level line with `&&` goes to ios_system unless it starts with one of
  the shell's own commands (`true &&`). `sh -c "$(…)"` runs its script in
  rootshell's interpreter, in the tab's shell (functions stay; Homebrew's
  install.sh was made to work so); a bare `sh` does nothing and ios_system
  has no `sh`, so `curl … | sh` runs nothing. In a script, a pipeline of
  programs fails ("sh: not supported: pipelines with external commands",
  owner's device, 2026-11-28, inside `$(…)`): one program per command. No stdin is read (a read would
  block the program): Ctrl-C or Options › Quit end it. A crash logs its
  panic ("crash: …") before the program ends. Untested on device.
- LocalDevVPN (seomin0610/LocalDevVPN, read 2026-11-27) swaps source and
  destination of every IPv4 packet for 10.7.0.1, so the VPN check is a
  datagram to 10.7.0.1:49152: refused (ICMP) at once through LocalDevVPN,
  nothing without it (`net::udp_probe`, three tries of 1.5 s); a TCP connect
  would hold the program. Untested on device.
- The "Pair with …" entry asks for the passcode, then sends pair-setup M1.
  The code is chosen as pairing starts and shown on the page at once (the
  Shortcut's page closes as the user goes to Settings; atelier-install
  chooses its code ahead the same way), and notified at M1 (owner: an
  earlier notification is gone by then); a wrong code gets a new one.
- rootshell's `ifconfig` leaves out the line break after each `hwassist=…<…>`
  line, so an interface's `inet` or the next interface's header follows on
  it (atelier-install, on an iPhone, 14 Nov 2026): `mdns::ifconfig_lines`
  splits them again.
- Fixed in rootshell 1.0.16-177, BariStore's oldest: folder listings that
  stopped after ~100 entries (3ad9c828), and a stopped program's parked
  accept taking the next program's first connection (78c9a1da: a finished
  program's sockets and parked calls are dropped); atelier-install works
  around both for older ones.
- Developer Mode may be missing from Settings on a phone that never had it:
  offering a pairing (the pairable-host advert) makes iOS show it (Apple's
  docs, and the owner's a-Shell build revealed it this way).
- Tunnel TCP (as jktcp, idevice's device-proven stack): MSS = min(MTU from the
  CDTunnel handshake, 8192) - 60, window scale option sent (shift 0) and the
  peer's honoured, at most 1 MiB in flight; AFC writes 1 MiB per request.
- Upload speed (owner, 1.0.1: 160 MB took about a minute): the tunnel's
  TLS (AES-256-CBC + HMAC-SHA384) is CPU-bound, and software AES computes
  several blocks per call, so CBC one record at a time wasted most of it:
  records are encrypted eight side by side (`cbc_lockstep`). The log says
  "install: sent N MB in X s". Untested on device in rootshell.
- Memory: IPAs are streamed to disk, read from their file while signing
  (`bundle::Source`: a handle and a 32 KB buffer), only Mach-O/plist entries
  kept in memory, the signed IPA written to `~/.baristore/signed.ipa` and sent
  from it 1 MiB at a time (removed once sent; a leftover goes with
  `prune_files`), a cached IPA hashed 1 MiB at a time. 1.1.1 held the whole
  IPA, then the whole signed one, in memory (Reddit report, 2026-11-27: a
  663 MB synthetic Unity-like IPA peaked at 1656 MB, 1.1.2 322 MB). One
  picture is decoded at a time (bounded); a picture being made is not made
  again for a second request; one shared HTTP client keeps connections; every
  app's icon is made ready after each catalog refresh, one after another.
  The page shows Featured 30 tiles at a time (the next 30 as the end comes
  near, the first 30 again on a new category or search; 12 before the
  tiles got short) and loads pictures a few at a time, those near the
  screen first, each with a spinner until it is there (owner, 1.0.0:
  pictures came slowly with every one asked at once): icons 8 at a time,
  screenshots 3.
  Pictures it showed once skip that queue when the page is drawn again
  (owner, 1.0.1: the new tiles loaded slower; each category switch sent
  every picture through the queue again). Every answer is one send with
  `Connection: close`; a request's head must come within 15 s (Safari opens
  connections ahead of need).
- A task's title and retry are in `~/.baristore/task-running` while it runs:
  the next start shows a task the last one stopped during as failed, with
  Try again. The page reloads when the store's boot id changes and shows
  "Reconnecting" while the store does not answer (8 s state, 120 s actions).
- Wi-Fi cannot be read in rootshell either. No answer to the VPN check means
  no route (mobile data or no VPN): say so, never knock 16 000 ports then
  (siboot's preflight); a refused port does mean "look on other ports". The
  check takes three tries (back in the foreground the first one went nowhere
  and a retry seconds later connected, owner's log, 1.1.0).
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
- The page's origin is stable while the port and token are kept, but nothing
  it must remember lives in localStorage: settings.json holds theme and
  setup_done. The page uses its own sheets (they looked so in Minis, where
  confirm()/alert() did not show).
- Files reach the user through the Web Share sheet (`navigator.share` with a
  File, from a tap), else a download.
- The pairing service is usually on 49152 but can be another dynamic port
  (siboot 0.19.1): the saved port, 49152, the ports Bonjour lists for
  `_remotepairing._tcp` (rootshell may browse it; those at this device's own
  addresses first, as atelier-install does), then 49153–65535 except 62078,
  one connect at a time.
- iOS 27.2+: every RemoteXPC handshake on a tunnel must present the same host
  UUID (idevice); we keep one per install (`~/.baristore/host-uuid`).
- SideStore nightlies read `Documents/PairingFile_RemoteRP.plist` only with
  prefs `isPairingReset=false`, `activePairingProtocol=rppairing`; LiveContainer's
  SideStore reads `SideStore/Documents/ALTPairingFile.mobiledevicepairing`;
  AltStore 2.3.1+ reads `Documents/ALTPairingFile.plist` (SideInstaller docs).
- Second factor (`gsa::second_factor`, as SideStore and siboot offer it):
  GET /auth lists the trusted numbers (pushMode "voice": calls only) and may
  say noTrustedDevices; the code goes to the devices or a number, by text
  ("sms") or a call ("voice", the same PUT /auth/verify/phone), and the page
  offers every other way and the same again. `repair` means no second
  factor is set up: signed in. The log names what Apple asked for (au, hsc
  409) and how many numbers the account has.
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
  "Reading the app…", with Cancel. The server writes the body to the file
  4 MiB at a time (rootshell brings 32 KB a receive; 30 MB in 6 s through
  rootshell's worker on Linux).
- Install order (owner, 1.1.1: optimise): Apple is asked for
  `install::Shared` first, then the IPA is read (one thread: 1.1.1 read it
  meanwhile on a thread of its own); the app group is looked up (made) once;
  the app's and its extensions' App Groups, Increased Memory Limit and
  profiles are asked for at the same time (AltStore does extensions side by
  side); a cached tunnel's liveness check also gives Developer Mode's state.
- Signing speed (owner: a 140 MB app took very long): one pass per
  signature (its size is known first: the CMS has the same size whatever it
  signs), so pages are hashed once and one RSA signature made; the final
  check compares only the Info.plist/CodeResources slots; binaries of one
  bundle depth are hashed for their parent's seal as they are signed; new
  entries are compressed one small zip each and copied in raw; zip
  directories are read through a buffer. The task shows "Reading the app:
  N%". In WebAssembly the hashes are software (sha1/sha2 crates; 1.1.x used
  the ARMv8 SHA instructions Minis's emulator ran), one thread with
  check-ins; a synthetic 30 MB IPA signs in ~1.5 s in a debug build through
  rootshell's worker on Linux, the same code directories as the native build.
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

Device (owner, 2026-11-28, an earlier test build's install line): `sh -c
"$(curl …)"` ran the script in rootshell; its `$(curl … | grep … | head)`
failed as above (since fixed).
Linux-verified for 1.2.0-alpha.1 (after the squash): the unit tests
natively and in rootshell's worker; in that worker, `baristore page`
arming the bare address once (302 to a new key, then 404), `--join page`
arming the running store (and serving 46510 too when another program had
it at the start), `--join` ending with 1 with nothing running, a typed
`baristore` told where the store is, Quit removing `live`; the second
factor's ways (numbers from /auth texted or called, devices, again);
install.sh's download in a temp home; the page in Playwright (Before you
start open, then one line once pairing starts, Show/Hide; the code sheet's
other ways; the profile and Shortcut steps). Untested anywhere but a
device: the profile and Shortcut, install.sh in rootshell's interpreter,
the function in rootshell.

Linux-verified for the first rootshell build: the unit tests natively (75)
and as WebAssembly through rootshell's own worker (`scripts/rootshell.mjs`,
61; the rest start openssl or threads); the store in that worker: knocked
listener answering the page (state in tens of ms while a catalog refresh and
a VPN check run, 1.2 s at worst), the catalog over rootshell-style TLS,
icons shrunk, the page in Playwright (setup view, the "open" sheet), Add IPA
(30 MB byte-identical), `baristore install` (.rootshellrc once), pairing up
to the device's handshake (VPN check by datagram, address from
ifconfig.txt, three announcements, the handshake answered in 80 ms), `dev
sign` on a 30 MB IPA (code directories as the native build's). Nothing of
the rootshell builds is verified on a device yet: rootshell's real web view, Safari, the
mDNS announcement in Settings, OSC links and notifications, Location Diary
Mode in the background.

Before 1.2.0 (Open Minis; the logic is the same, the transport is new):
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

`src/rootshell` rootshell's calls (`native.rs`: the same rules off the device,
for tests), files in its pieces, terminal sequences · `src/rt` the runtime ·
`src/net.rs` TCP/UDP · `src/http.rs` HTTP/1.1 and WebSocket · `src/mdns.rs`
the pairable host's announcement · `src/pairing` pairable host · `src/device`
tunnel, TCP, XPC, services · `src/apple` anisette, GrandSlam, portal ·
`src/sign` code signing · `src/install.rs` App IDs → profiles → sign → install
→ pairing file, and `baristore install` · `src/store.rs` catalog ·
`src/icons.rs` icon cache · `src/app.rs` state/tasks · `src/web.rs` +
`ui/index.html` the page · `src/doctor.rs` report · `src/update.rs`
self-update · `scripts/rootshell.mjs` a program run as rootshell runs it ·
`docs/` the website, `docs/install.sh` the install line's script. CI: `.github/workflows/release.yml` tests (both ways)
and publishes the releases.

## Rules

- No dead code, no unused options, no extra files; `cargo build --release
  --target wasm32-wasip1` and `cargo build --release` must have zero warnings;
  `cargo test` must pass, and the WebAssembly tests in rootshell's worker (the
  command in release.yml).
- Never commit secrets, pairing records, keys, profiles or IPAs.
- Bump `version` in Cargo.toml on every functional change.
