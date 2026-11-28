# How BariStore works

BariStore is one WebAssembly program of about 4 MB. [rootshell](https://apps.apple.com/app/id6755794662), a terminal
app on the App Store, runs it; the store itself is a page BariStore serves on the device, which Safari shows.

## What it does

- **No computer, no second device.** Pairing, signing and installing all happen on the iPhone or iPad itself.
- **Developer Mode appears by itself.** If Settings does not list it, BariStore's pairing makes it appear.
- **Uses none of your 3 app slots.** It runs inside rootshell, an App Store app.
- **Refresh in seconds.** A refresh renews the apps' profiles: nothing is downloaded, signed or uploaded again (apps
  signed with an older certificate are signed again).
- **JIT with one tap** for emulators and other JIT apps.
- **Sources and updates.** App sources, any IPA from Files (a newer build of one updates it in place), one-tap
  updates, and per-app beta tracks (Off, nightly, alpha…).
- **Your App IDs under control.** See what uses each one, reuse the App IDs of deleted apps or type your own, keep or
  drop extensions, ask for Increased Memory Limit (later too: an update keeps it).
- **Your own app icons.** Pick any picture as an app's icon when you install it, or change it later.
- **Install into LiveContainer.** Send an app from Featured or an IPA from Files straight into LiveContainer.
- **One Apple ID on your iPhone and your iPad.** Export the signing certificate on one and import it on the other.
  BariStore never revokes a certificate without asking.
- **Works with the other tools.** SideStore, AltStore, LiveContainer + SideStore and StikDebug get the pairing file;
  the stores and LiveContainer get the signing certificate, so they can refresh themselves too.

## While it runs

- `baristore` starts BariStore in its rootshell tab and opens the store in Safari (rootshell › Settings › Terminal ›
  Open Links from Programs lets it do so by itself). Typed again while BariStore runs, it opens the store again.
- rootshell's Location Diary Mode keeps BariStore running while you are in Safari or Settings. So that it does not
  run for nothing, BariStore quits after 3 minutes without a tap on its page or a task, and the **⏻** button on its
  page quits it at once.
- Pairing: Settings › Privacy & Security › Developer Mode lists **Pair with BariStore**. After your passcode, the
  6-digit code comes as a notification and shows on the page.
- Setup's first step checks Wi-Fi (as BariStore started) and LocalDevVPN.

## Safe by design

- **Your password never leaves your device** and is never saved: Apple's sign-in turns it into a one-time proof on
  the device.
- **There is no BariStore server.** No accounts, no analytics. BariStore talks only to Apple, an anisette server and
  the sources you add.
- **The anisette server never sees your Apple ID.** It only supplies device-identity data from a random identifier.
  Options › Anisette server lets you pick one or add your own.
- **Your keys stay with you.** The pairing record, session and signing key stay in rootshell's own folder, in
  `.baristore`, which the Files app does not show.
- **The page answers only on the device**, under an address with a random key that other apps cannot guess.

## Free Apple ID limits (Apple's)

Apps last **7 days** (Refresh all renews them), **3** apps at a time (BariStore does not count; LiveContainer runs
many apps inside one), **10** App IDs per 7 days (each app extension needs one; "Remove extensions" saves them).
A paid developer account has none of these limits: BariStore notices it.

## Build

`cargo build --release --target wasm32-wasip1` makes `target/wasm32-wasip1/release/baristore.wasm`. Tests:
`cargo test`, and the same tests run by rootshell's own runtime with
`CARGO_TARGET_WASM32_WASIP1_RUNNER="node scripts/rootshell.mjs" cargo test --target wasm32-wasip1 -- --test-threads=1`
(see the skips in `.github/workflows/release.yml`). Design notes and what has been verified on a device:
[AGENTS.md](../AGENTS.md). Third-party notices: [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).
