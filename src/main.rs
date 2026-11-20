mod device;
mod log;
mod minis;
mod pairing;
mod siphash;
mod srp;
mod state;

use std::process::ExitCode;

const USAGE: &str = "usage: baristore            open the store
       baristore doctor     print a diagnostic report to paste
       baristore --version";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let _ = std::fs::create_dir_all(state::home());
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    let result = runtime.block_on(run(args));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("baristore: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None => Err("the store UI is not built yet".into()),
        Some("--version") | Some("-V") => {
            println!("baristore {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("dev") => dev(&args[1..]).await,
        _ => {
            println!("{USAGE}");
            Ok(())
        }
    }
}

/// Developer commands for interop tests on Linux. Not part of the product.
async fn dev(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("pairhost") => {
            let port: u16 = args.get(1).and_then(|p| p.parse().ok()).ok_or("dev pairhost <port> <code>")?;
            let code = args.get(2).ok_or("dev pairhost <port> <code>")?;
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| e.to_string())?;
            let (stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
            let identity = pairing::record::HostIdentity::generate();
            let log = |line: String| println!("host: {line}");
            let peer = pairing::host::PairableHost::new(stream, &identity, "BariStore", &log)
                .accept(code).await.map_err(|e| e.to_string())?;
            println!("paired: udid={} model={} name={}", peer.udid, peer.model, peer.name);
            Ok(())
        }
        Some("tunnel") => {
            // Through CDTunnel against BARISTORE_DEVICE, then one raw packet.
            let record = pairing::record::PairingRecord {
                host: pairing::record::HostIdentity::generate(),
                peer: Default::default(),
            };
            let tunnel = device::session::Tunnel::open(&record).await.map_err(|e| e.to_string())?;
            println!("tunnel: {} -> {} rsd {}", tunnel.client, tunnel.server, tunnel.rsd_port);
            match tokio::time::timeout(std::time::Duration::from_secs(5), tunnel.stack.connect(tunnel.rsd_port)).await {
                Ok(Err(e)) => println!("tcp over the echo link: {e}"),
                Ok(Ok(_)) => println!("tcp over the echo link: connected"),
                Err(_) => println!("tcp over the echo link: no answer (expected from an echo)"),
            }
            Ok(())
        }
        _ => Err("dev: unknown command".into()),
    }
}
