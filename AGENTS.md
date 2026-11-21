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
- Store spec: Featured (search, add source, AltSources, BariStore picks:
  AltStore, SideStore, LiveContainer+SideStore, LiveContainer, stable + nightly),
  My Apps (installed, refresh, delete, per-app options), Options (Apple ID,
  pairing export, info, theme, reset, install defaults: Increased Memory Limit,
  remove extensions). JIT: not built (needs DDI mounting + debugserver; iOS 26+
  JIT works only in apps built for it); ask before starting it.
- Never sign the user out on our own. Never store the password.
- Code, comments, commits, README in English. The owner talks Turkish.
- Device tests come in **batches** the owner runs; keep their effort low.

## Facts we rely on (and where they come from)

- Minis keeps running in Settings only while `apple-player` plays (siboot;
  same mechanism proven for a-Shell's `play` on device).
- iOS 27.2+: every RemoteXPC handshake on a tunnel must present the same host
  UUID (idevice); we keep one per install (`~/.baristore/host-uuid`).
- SideStore nightlies read `Documents/PairingFile_RemoteRP.plist` only with
  prefs `isPairingReset=false`, `activePairingProtocol=rppairing`; LiveContainer's
  SideStore reads `SideStore/Documents/ALTPairingFile.mobiledevicepairing`;
  AltStore 2.3.1+ reads `Documents/ALTPairingFile.plist` (SideInstaller docs).
- Apple sign-in needs Mac-emulating anisette and the server's own client_info
  (-80045/-80046 since October 2026).
- Each app extension needs its own App ID/profile, identifier under the app's.

## Verified vs not

Linux-verified: SRP against idevice-srp (both roles), pairing host against a
mock iPhone, TLS-PSK against openssl s_server, TCP stack against smoltcp, tunnel
against a mock device, CSR with openssl, signatures with rcodesign. **Nothing is
device-verified yet.** Say "untested on device" until the owner confirms.

## Layout

`src/pairing` pairable host · `src/device` tunnel, TCP, XPC, services ·
`src/apple` anisette, GrandSlam, portal · `src/sign` code signing ·
`src/install.rs` App IDs → profiles → sign → install → pairing file ·
`src/store.rs` catalog · `src/app.rs` state/tasks · `src/web.rs` + `ui/index.html`
the page · `src/doctor.rs` report. CI: `.github/workflows/release.yml` publishes
the `nightly` release that `install.sh` downloads.

## Rules

- No dead code, no unused options, no extra files; `cargo build --release` must
  have zero warnings; `cargo test` must pass.
- Never commit secrets, pairing records, keys, profiles or IPAs.
- Bump `version` in Cargo.toml on every functional change.
