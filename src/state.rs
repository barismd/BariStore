//! Where BariStore keeps its files, its settings, and how secrets are
//! written.

use std::io::Write;
use std::path::{Path, PathBuf};

/// `$BARISTORE_HOME`, else `~/.baristore` (rootshell gives a program
/// `HOME=/`, its Documents folder).
pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("BARISTORE_HOME") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join(".baristore")
}

pub fn path(name: &str) -> PathBuf {
    home().join(name)
}

/// There while a store runs (and after one was stopped by Ctrl-C): the
/// `baristore` command then asks it first (main.rs `join`).
pub fn live_path() -> PathBuf {
    path("live")
}

/// Ends the store (its page's ⏻, three idle minutes, an update): no
/// longer running.
pub fn end(code: i32) -> ! {
    let _ = std::fs::remove_file(live_path());
    std::process::exit(code)
}

/// Writes `data` whole or not at all (pairing records, sessions, keys):
/// to a file beside it, then in its place. They stay in rootshell's own
/// folder (~ is rootshell's Documents), in .baristore, which the Files app
/// does not show.
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        crate::rootshell::files::make_dirs(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    crate::rootshell::files::replace(&tmp, path)
}

/// What settings.json may hold, with the defaults: the page's theme, the
/// install defaults, whether setup is done, the anisette server chosen in
/// Options ("" = automatic) and the anisette servers the user added.
fn defaults() -> serde_json::Value {
    serde_json::json!({"theme": "system", "increased_memory": false, "remove_extensions": false,
                       "setup_done": false, "anisette": "", "anisette_servers": []})
}

pub fn settings() -> serde_json::Value {
    let mut s = defaults();
    let saved: serde_json::Value = std::fs::read(path("settings.json")).ok()
        .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    merge(&mut s, &saved);
    s
}

/// Known keys of `patch` replace the saved ones.
pub fn save_settings(patch: &serde_json::Value) {
    let mut s = settings();
    merge(&mut s, patch);
    let _ = std::fs::write(path("settings.json"), s.to_string());
}

fn merge(into: &mut serde_json::Value, from: &serde_json::Value) {
    if let (Some(obj), Some(from)) = (into.as_object_mut(), from.as_object()) {
        for (k, v) in from {
            if obj.get(k).is_some_and(|d| std::mem::discriminant(d) == std::mem::discriminant(v)) {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
}

/// A new path for a test's files: the system's temporary folder, or
/// rootshell's /tmp (WASI has no `temp_dir` and no process id).
#[cfg(test)]
pub fn scratch(name: &str) -> PathBuf {
    let base = if cfg!(target_os = "wasi") { PathBuf::from("/tmp") } else { std::env::temp_dir() };
    base.join(format!("baristore-{:08x}-{name}", rand::random::<u32>()))
}
