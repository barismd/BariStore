//! Options › Log: one block of plain text to paste when something fails, with
//! the recent log; "Run checks" adds network, tunnel and sign-in checks.
//! Never contains the password, tokens, keys or the pairing record; the email
//! and UDID are shortened.

use std::fmt::Write;
use std::time::Duration;

use crate::{apple, device::session, install, pairing::flow, rt, state, store};

fn short(s: &str, keep: usize) -> String {
    if s.len() <= keep { s.to_string() } else { format!("{}…", &s[..keep]) }
}

async fn reach(url: &str) -> String {
    let http = store::client();
    let started = std::time::Instant::now();
    match rt::time::timeout(Duration::from_secs(12), http.get(url).send()).await {
        Ok(Ok(r)) => format!("HTTP {} in {} ms", r.status().as_u16(), started.elapsed().as_millis()),
        Ok(Err(e)) => format!("failed: {e}"),
        Err(_) => "timed out".into(),
    }
}

pub async fn report(checks: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "BariStore {} {}", env!("CARGO_PKG_VERSION"), if checks { "checks" } else { "log" });
    let _ = writeln!(out, "rootshell   {}", if crate::rootshell::has_recv_timeouts() { "1.0.16-177 or newer" }
                                         else { "TOO OLD (no receive time limits)" });
    let _ = writeln!(out, "home        {}", state::home().display());
    // What `baristore` noted (pairing's announcement needs it).
    let noted = crate::mdns::ifconfig_addresses(&std::fs::read_to_string(state::path("ifconfig.txt")).unwrap_or_default());
    let shown: Vec<String> = noted.iter().map(|a| std::net::Ipv4Addr::from(*a).to_string()).collect();
    let _ = writeln!(out, "address     {}", if shown.is_empty() { "none noted (start with baristore)".to_string() } else { shown.join(", ") });
    let route = session::route().await;
    let _ = writeln!(out, "connection  {}", match route { Ok(()) => "Wi-Fi and LocalDevVPN", Err(e) => e });
    let vpn = route.is_ok();
    if checks && vpn {
        let _ = writeln!(out, "pairing svc {}", match session::endpoint().await {
            Ok((host, port)) => format!("{host}:{port}"),
            Err(e) => e.to_string(),
        });
    }
    match flow::load_record() {
        Some(r) => {
            let _ = writeln!(out, "pairing     yes, {} ({}), udid {}", r.peer.name, r.peer.model, short(&r.peer.udid, 8));
            if checks && vpn {
                match rt::time::timeout(Duration::from_secs(40), session::Session::open(&r)).await {
                    Ok(Ok(s)) => {
                        let dm = crate::device::services::developer_mode(&s).await;
                        let _ = writeln!(out, "tunnel      ok, {} services, Developer Mode {:?}", s.services.len(), dm);
                        let ios = s.properties.get("OSVersion").and_then(plist::Value::as_string).unwrap_or("?");
                        let _ = writeln!(out, "iOS         {ios}");
                    }
                    Ok(Err(e)) => { let _ = writeln!(out, "tunnel      FAILED: {e}"); }
                    Err(_) => { let _ = writeln!(out, "tunnel      timed out"); }
                }
            }
        }
        None => { let _ = writeln!(out, "pairing     none yet"); }
    }
    match apple::gsa::Account::load() {
        Some(a) => { let _ = writeln!(out, "Apple ID    {} (token fresh: {})", short(&a.email, 3), a.token_fresh()); }
        None => { let _ = writeln!(out, "Apple ID    not signed in"); }
    }
    let team = std::fs::read_to_string(state::path("team")).unwrap_or_default();
    let mut lines = team.lines();
    let _ = writeln!(out, "team        {}{}", lines.next().unwrap_or("-"),
                     lines.nth(1).map(|kind| format!(" ({kind})")).unwrap_or_default());
    if checks {
        let chosen = apple::anisette::chosen();
        let servers: Vec<&str> = match &chosen {
            Some(server) => vec![server.as_str()],
            None => apple::anisette::SERVERS.to_vec(),
        };
        for server in &servers {
            let _ = writeln!(out, "anisette    {server:<32} {}", reach(&format!("{server}/v3/client_info")).await);
        }
        // A made-up account through the first (or the chosen) server: tells
        // which client Apple takes from this network today.
        for claim in apple::anisette::Claim::ALL {
            let verdict = apple::gsa::probe(servers[0], claim).await;
            let _ = writeln!(out, "sign-in     as {:<12} {verdict}", claim.name());
        }
        let _ = writeln!(out, "github      {}", reach("https://api.github.com/repos/SideStore/SideStore/releases?per_page=1").await);
    }
    let apps = install::installed();
    let _ = writeln!(out, "installed   {} app(s)", apps.len());
    for a in apps {
        let left = a.expires.saturating_sub(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()) / 3600;
        let _ = writeln!(out, "  {} {} ({} h left)", a.name, a.version, left);
    }
    let _ = writeln!(out, "--- last log lines ---");
    let log = std::fs::read_to_string(state::path("baristore.log")).unwrap_or_default();
    let lines: Vec<&str> = log.lines().collect();
    for line in &lines[lines.len().saturating_sub(200)..] {
        let _ = writeln!(out, "{line}");
    }
    out
}
