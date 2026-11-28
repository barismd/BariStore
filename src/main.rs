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

use rootshell::osc::say;

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOCATOR: rootshell::Nudging = rootshell::Nudging;

const USAGE: &str = "usage: baristore            open the store in Safari
       baristore page       show the store over rootshell (the BariStore Shortcut runs it)
       baristore install    set up the baristore command
       baristore update     install the newest BariStore
       baristore log        print the log (as Options › Log)
       baristore --version";

/// The exit code after the store installed a new build: the `baristore`
/// command (install.rs `ROOTSHELLRC`) starts the new file at once.
pub const UPDATED: i32 = 75;

/// The port the BariStore Shortcut opens the page on
/// (http://127.0.0.1:46510/): the store takes it whenever it is free.
pub const PAGE_PORT: u16 = 46510;

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

/// Whether the store starts for the Shortcut (`page`): the page opens over
/// rootshell, at PAGE_PORT, without a key (web.rs `arm`).
fn for_shortcut(args: &[String]) -> bool {
    args.first().map(String::as_str) == Some("page")
}

async fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("page") => serve(for_shortcut(&args)).await,
        Some("install") => {
            install::set_up_command().await;
            Ok(())
        }
        Some("update") => {
            let outcome = update::download_and_replace(&|step: String| say(&step)).await?;
            match outcome {
                update::Outcome::UpToDate => say(&format!("BariStore {} is the newest build.", env!("CARGO_PKG_VERSION"))),
                update::Outcome::Updated(version) => say(&format!("Updated to {version}. Type baristore to open the store.")),
            }
            Ok(())
        }
        Some("log") => {
            say(&doctor::report(false).await);
            Ok(())
        }
        Some("--version") | Some("-V") => {
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
/// (`live`): rootshell holds a
/// new tab's ifconfig, curl and mkdir until the program in another tab
/// ends (atelier-install saw it on a device), so the command runs none of
/// them before this. Hands what was asked to the running store, or runs a
/// command that needs no store, and ends with success; ends with 1 when no
/// store answers, and the command then starts one.
async fn join(args: Vec<String>) -> ExitCode {
    if !matches!(args.first().map(String::as_str), None | Some("page") | Some("install")) {
        if let Err(message) = run(args).await {
            eprintln!("baristore: {message}");
        }
        return ExitCode::SUCCESS;
    }
    let shortcut = for_shortcut(&args);
    match Running::load() {
        // The install line again while the store runs: the command only.
        Some(r) if answers(&r).await && args.first().map(String::as_str) == Some("install") => {
            install::set_up_command().await;
            say("BariStore is running in another rootshell tab: Options › Update BariStore there installs the \
                 newest build.");
            ExitCode::SUCCESS
        }
        Some(r) if answers(&r).await => {
            hand_over(&r, shortcut).await;
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

/// To the store running in another tab: a page to open for the Shortcut
/// (and its port), or, typed, where the store is.
async fn hand_over(r: &Running, shortcut: bool) {
    if !shortcut {
        say("BariStore is already running in another rootshell tab.");
        tell_where(&r.url());
        return;
    }
    let http = http::Client::builder().timeout(std::time::Duration::from_secs(10)).fresh_connections().build();
    let body = serde_json::json!({"port": PAGE_PORT});
    let reply = http.post(&format!("{}api/arm", r.url())).json(&body).send().await;
    let answer: serde_json::Value = match reply {
        Ok(reply) => reply.json().await.unwrap_or_default(),
        Err(e) => serde_json::json!({"message": e.to_string()}),
    };
    if answer["ok"] == true {
        say("BariStore runs in another rootshell tab, and its page shows over rootshell now. rootshell opens a new \
             tab for each run of the Shortcut: this one can be closed.");
    } else {
        say(&format!("BariStore runs in another rootshell tab, but did not take the Shortcut's page ({}).",
                     answer["message"].as_str().unwrap_or("no answer")));
    }
}

/// Opens the page in Safari: rootshell does it with its Settings ›
/// Terminal › Open Links from Programs on; else the address it shows opens
/// with a touch and hold.
fn tell_where(url: &str) {
    say(&format!("The store opens in Safari: {}", rootshell::osc::link(url, url)));
    say("Nothing opened? Touch and hold the address and tap Open Link (with rootshell's Settings › Terminal › Open \
         Links from Programs on, it opens by itself).");
    rootshell::osc::open_url(url);
}

/// A port no program on the device listens on: the Shortcut's first.
fn free_port(saved: Option<u16>) -> u16 {
    for p in [Some(PAGE_PORT), saved].into_iter().flatten() {
        if !net::in_use(p) {
            return p;
        }
    }
    loop {
        let p = 49200 + rand::random::<u16>() % 12000;
        if p != 62078 && !net::in_use(p) {
            return p;
        }
    }
}

/// The store: serves the page until Options › Quit, Ctrl-C or an update.
/// `shortcut`: started for the Shortcut, which shows the page over rootshell.
async fn serve(shortcut: bool) -> Result<(), String> {
    if !rootshell::has_recv_timeouts() {
        return Err("this rootshell is too old for BariStore: update rootshell from the App Store (1.0.16-177 or \
                    newer is needed).".into());
    }
    let saved = Running::load();
    if let Some(r) = saved.as_ref() {
        if answers(r).await {
            hand_over(r, shortcut).await;
            return Ok(());
        }
    }
    let running = Running {
        port: free_port(saved.as_ref().map(|r| r.port)),
        token: saved.map(|r| r.token).filter(|t| t.len() == 32).unwrap_or_else(|| hex::encode(rand::random::<[u8; 16]>())),
    };
    let listener = net::TcpListener::bind(running.port).map_err(|e| format!("cannot serve the page on port {}: {e}", running.port))?;
    running.save();
    let _ = state::write_private(&state::live_path(), app::boot_id().as_bytes());
    web::set_keys(&running.token);
    if shortcut {
        web::arm();
    }
    logf!("baristore {} starting on port {}{}", env!("CARGO_PKG_VERSION"), running.port,
          if shortcut { ", for the Shortcut" } else { "" });
    say(&format!("BariStore {}", env!("CARGO_PKG_VERSION")));
    if shortcut && running.port != PAGE_PORT {
        say(&format!("Port {PAGE_PORT}, where the Shortcut looks, is taken by another program: open the store in \
                      Safari instead."));
        say(&rootshell::osc::link(&running.url(), &running.url()));
    } else if shortcut {
        say("The store shows over rootshell, in the Shortcut's page.");
    } else {
        tell_where(&running.url());
    }
    say("Keep this tab open: BariStore runs here. Ctrl-C quits it (Options › Quit too).");
    say("So that it keeps running while you are in Safari, Settings or another app, turn on rootshell's Location \
         Diary Mode (Settings › General, Mode Auto During Active Sessions).");
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
