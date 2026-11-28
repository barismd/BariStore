mod app;
mod apple;
mod der;
mod device;
mod doctor;
mod http;
mod icons;
mod install;
mod log;
mod mdns;
mod net;
mod pairing;
mod rootshell;
mod rt;
mod sign;
mod siphash;
mod srp;
mod state;
mod store;
mod update;
mod web;

use std::process::ExitCode;

use rootshell::osc::{say, style, BOLD, CYAN, DIM, GREEN};

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOCATOR: rootshell::Nudging = rootshell::Nudging;

const USAGE: &str = "baristore           open the store
baristore update    update it
baristore log       print the log
baristore --version";

/// The exit code after the store installed a new build: the `baristore`
/// command (install.rs `ROOTSHELLRC`) starts the new file at once.
pub const UPDATED: i32 = 75;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let _ = crate::rootshell::files::make_dirs(&state::home());
    // A panic ends the program (WebAssembly cannot unwind): its words go to
    // the log first, for Options › Log after the next start.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        logf!("crash: {info}");
        default_hook(info);
    }));
    if args.first().map(String::as_str) == Some("--join") {
        return rt::block_on(join(args[1..].to_vec()));
    }
    match rt::block_on(run(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("baristore: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None => serve().await,
        // The install line's step (docs/install.sh), not for typing.
        Some("install") => {
            install::set_up_command().await;
            Ok(())
        }
        Some("update") => {
            let outcome = update::download_and_replace(&|step: String| say(&step)).await?;
            match outcome {
                update::Outcome::UpToDate => say(&format!("BariStore {} is the newest.", env!("CARGO_PKG_VERSION"))),
                update::Outcome::Updated(version) => {
                    say(&format!("Updated to {version}."));
                    say("Type baristore to open it.");
                }
            }
            Ok(())
        }
        Some("log") => {
            say(&doctor::report(false).await);
            Ok(())
        }
        Some("--version") => {
            say(update::MARK.trim_matches('\0'));
            Ok(())
        }
        #[cfg(debug_assertions)]
        Some("dev") => dev(&args[1..]).await,
        _ => {
            say(USAGE);
            Ok(())
        }
    }
}

/// `baristore.wasm --join …`, which the `baristore` command (and the
/// install line) runs first while a store may be running in another tab
/// (`live`): rootshell holds a new tab's ifconfig, curl and mkdir until the
/// program in another tab ends (atelier-install saw it on a device), so
/// the command runs none of them before this. Opens the running store's
/// page (the install line also sets up the command), or runs a command
/// that needs no store, and ends with success; ends with 1 when no store
/// answers, and the command then starts one.
async fn join(args: Vec<String>) -> ExitCode {
    let first = args.first().map(String::as_str);
    if !matches!(first, None | Some("install")) {
        if let Err(message) = run(args).await {
            eprintln!("baristore: {message}");
        }
        return ExitCode::SUCCESS;
    }
    match Running::load() {
        Some(r) if answers(&r).await => {
            if first == Some("install") {
                install::set_up_command().await;
            }
            say(&style(BOLD, "BariStore is already running."));
            open_page(&r.url());
            ExitCode::SUCCESS
        }
        _ => {
            let _ = std::fs::remove_file(state::live_path());
            ExitCode::from(1)
        }
    }
}

/// Where the store answers: "<port> <token>" in ~/.baristore/running. A new
/// run keeps both while the port is free, so a page left open finds the
/// store again after a restart or an update.
struct Running {
    port: u16,
    token: String,
}

impl Running {
    fn load() -> Option<Running> {
        let text = std::fs::read_to_string(state::path("running")).ok()?;
        let mut parts = text.split_whitespace();
        Some(Running { port: parts.next()?.parse().ok()?, token: parts.next()?.to_string() })
    }

    fn save(&self) {
        let _ = state::write_private(&state::path("running"), format!("{} {}", self.port, self.token).as_bytes());
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/{}/", self.port, self.token)
    }
}

/// Whether a store answers at `r` (one running in another tab).
async fn answers(r: &Running) -> bool {
    let ask = async {
        let http = http::Client::builder().timeout(std::time::Duration::from_secs(4)).fresh_connections().build();
        let reply = http.get(format!("{}api/ping", r.url())).send().await.ok()?;
        reply.status().is_success().then_some(())
    };
    ask.await.is_some()
}

/// Opens the page in Safari: rootshell does it with its Settings ›
/// Terminal › Open Links from Programs on; else the link below opens with
/// a touch and hold (the address itself is too long for a line).
fn open_page(url: &str) {
    rootshell::osc::open_url(url);
    say(&format!("{} {}", style(GREEN, "▶"), style(BOLD, &rootshell::osc::link(url, "Open BariStore"))));
    say(&style(DIM, "(no Safari? touch and hold it)"));
}

/// What to set once, before the first start: lines of 34 columns at most
/// (osc.rs). The page and README say the same in more words.
const CHECKLIST: &[&str] = &[
    "Before you start:",
    "",
    "1. Developer Mode: On",
    "   Settings › Privacy & Security",
    "2. LocalDevVPN: Connect, on Wi-Fi",
    "3. rootshell › Settings › General",
    "   Location Diary Mode: On",
    "   Terminal Notifications: On",
    "4. rootshell › Settings › Terminal",
    "   Open Links from Programs: On",
    "5. Local Network: Allow",
    "   (when rootshell asks)",
];

/// The checklist, the first time BariStore starts, until Return.
fn first_start() {
    let seen = state::path("welcomed");
    if seen.exists() {
        return;
    }
    for (i, line) in CHECKLIST.iter().enumerate() {
        let shown = if i == 0 { style(&format!("{BOLD};{CYAN}"), line) } else { line.to_string() };
        say(&shown);
    }
    say("");
    say(&style(BOLD, "Press Return to open the store."));
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    let _ = state::write_private(&seen, b"");
    say("");
}

/// A port no program on the device listens on: the last one, so a page
/// left open in Safari finds the store again.
fn free_port(saved: Option<u16>) -> u16 {
    if let Some(p) = saved.filter(|p| !net::in_use(*p)) {
        return p;
    }
    loop {
        let p = 49200 + rand::random::<u16>() % 12000;
        if p != 62078 && !net::in_use(p) {
            return p;
        }
    }
}

/// The store: serves the page until it is quit (its page's power button,
/// Ctrl-C, three idle minutes) or updated.
async fn serve() -> Result<(), String> {
    if !rootshell::has_recv_timeouts() {
        return Err("update rootshell first".into());
    }
    let saved = Running::load();
    if let Some(r) = saved.as_ref() {
        if answers(r).await {
            say(&style(BOLD, "BariStore is already running."));
            open_page(&r.url());
            return Ok(());
        }
    }
    say(&style(BOLD, &format!("BariStore {}", env!("CARGO_PKG_VERSION"))));
    say("");
    first_start();
    let running = Running {
        port: free_port(saved.as_ref().map(|r| r.port)),
        token: saved.map(|r| r.token).filter(|t| t.len() == 32).unwrap_or_else(|| hex::encode(rand::random::<[u8; 16]>())),
    };
    let listener = net::TcpListener::bind(running.port).map_err(|e| format!("cannot serve the page on port {}: {e}", running.port))?;
    running.save();
    let _ = state::write_private(&state::live_path(), app::boot_id().as_bytes());
    web::set_key(&running.token);
    device::session::note_wifi();
    logf!("baristore {} starting on port {}", env!("CARGO_PKG_VERSION"), running.port);
    open_page(&running.url());
    say("");
    say("BariStore runs in this tab.");
    say(&style(DIM, "It quits after 3 idle minutes,"));
    say(&style(DIM, "with ⏻ on its page or Ctrl-C."));
    let store = app::App::new();
    web::serve(store, listener).await;
    Ok(())
}

/// Developer commands for interop tests (debug builds only).
#[cfg(debug_assertions)]
async fn dev(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("pairhost") => {
            let port: u16 = args.get(1).and_then(|p| p.parse().ok()).ok_or("dev pairhost <port> <code>")?;
            let code = args.get(2).ok_or("dev pairhost <port> <code>")?;
            let listener = net::TcpListener::bind(port).map_err(|e| e.to_string())?;
            let stream = listener.accept().await.map_err(|e| e.to_string())?;
            let identity = pairing::record::HostIdentity::generate();
            let log = |line: String| say(&format!("host: {line}"));
            let peer = pairing::host::PairableHost::new(stream, &identity, "BariStore", &log)
                .accept(|| async { code.clone() }).await.map_err(|e| e.to_string())?;
            say(&format!("paired: udid={} model={} name={}", peer.udid, peer.model, peer.name));
            Ok(())
        }
        Some("tunnel") => {
            // Through CDTunnel against BARISTORE_DEVICE, then one raw packet.
            let record = pairing::record::PairingRecord {
                host: pairing::record::HostIdentity::generate(),
                peer: Default::default(),
            };
            let tunnel = device::session::Tunnel::open(&record).await.map_err(|e| e.to_string())?;
            say(&format!("tunnel: rsd {}", tunnel.rsd_port));
            match rt::time::timeout(std::time::Duration::from_secs(5), tunnel.stack.connect(tunnel.rsd_port)).await {
                Ok(Err(e)) => say(&format!("tcp over the echo link: {e}")),
                Ok(Ok(_)) => say("tcp over the echo link: connected"),
                Err(_) => say("tcp over the echo link: no answer (expected from an echo)"),
            }
            Ok(())
        }
        Some("sign") => {
            // dev sign <in.ipa> <out.ipa> <cert.der> <key.pk8.der> <team>: every
            // app/appex gets a dummy profile and minimal entitlements.
            use plist::Value;
            let a = |i: usize| args.get(i).cloned().ok_or("dev sign <in> <out> <cert.der> <key.der> <team>");
            let leaf = std::fs::read(a(3)?).map_err(|e| e.to_string())?;
            let key = {
                use rsa::pkcs8::DecodePrivateKey;
                rsa::RsaPrivateKey::from_pkcs8_der(&std::fs::read(a(4)?).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?
            };
            let team = a(5)?;
            let mut ipa = sign::bundle::Ipa::read(std::path::Path::new(&a(1)?), &|_, _| {})?;
            let mut plan = sign::bundle::Plan {
                identity: sign::codesign::Identity { leaf, chain: vec![], key, team: team.clone(), common_name: "Test".into() },
                bundles: Default::default(), info_edits: Default::default(), add_files: vec![],
            };
            for b in ipa.bundles()? {
                if b.kind == sign::bundle::Kind::Framework { continue; }
                let new_id = format!("{}.{team}", b.id);
                let mut ent = plist::Dictionary::new();
                ent.insert("application-identifier".into(), Value::String(format!("{team}.{new_id}")));
                ent.insert("com.apple.developer.team-identifier".into(), Value::String(team.clone()));
                ent.insert("get-task-allow".into(), Value::Boolean(true));
                say(&format!("bundle {} {} -> {}", b.dir, b.id, new_id));
                plan.bundles.insert(b.dir.clone(), sign::bundle::Signing { new_id, profile: b"profile".to_vec(), entitlements: ent });
            }
            let started = std::time::Instant::now();
            sign::bundle::sign(&mut ipa, &plan, &mut |n, t| say(&format!("signed {n}/{t}")))?;
            ipa.write(std::path::Path::new(&a(2)?))?;
            say(&format!("ok in {:?}", started.elapsed()));
            Ok(())
        }
        _ => Err("dev: unknown command".into()),
    }
}
