//! `baristore update` and Options › Update BariStore: the newest release
//! with a rootshell build (a stable one; a test build also takes newer test
//! builds), checked against its SHA-256 and put in place of
//! ~/bin/baristore.wasm (rootshell finds `baristore.wasm` there by name).
//! rootshell read the running program whole when it started it, so the file
//! can be replaced while it runs.

use sha2::{Digest, Sha256};
use std::path::PathBuf;

const RELEASES: &str = "https://api.github.com/repos/barismd/BariStore/releases?per_page=20";
const FILE: &str = "baristore.wasm";

/// What every build carries, so a download is known to be BariStore and
/// its version can be read without starting it (a WebAssembly program
/// cannot run another).
pub static MARK: &str = concat!("\0BariStore build ", env!("CARGO_PKG_VERSION"), "\0");

pub enum Outcome {
    UpToDate,
    /// The version the new build carries.
    Updated(String),
}

/// Where `baristore` starts BariStore from.
pub fn executable() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    home.join("bin").join(FILE)
}

/// The version a build carries (its `MARK`).
fn version_of(bytes: &[u8]) -> Option<String> {
    let mark = b"\0BariStore build ";
    let at = bytes.windows(mark.len()).position(|w| w == mark)? + mark.len();
    let end = bytes[at..].iter().position(|b| *b == 0)?;
    String::from_utf8(bytes[at..at + end].to_vec()).ok().filter(|v| !v.is_empty() && v.len() < 40)
}

async fn get(http: &crate::http::Client, url: String) -> Result<crate::http::Response, String> {
    http.get(&url).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("download failed: {e}"))
}

/// The newest published build this one updates to: its download address
/// and SHA-256. Releases from before the rootshell builds have no
/// baristore.wasm and are passed over; a test build (x.y.z-alpha.N) also
/// takes test builds. Newest by when it was published, not in the order
/// GitHub lists releases (by version).
async fn published(http: &crate::http::Client) -> Result<(String, String), String> {
    let testing = env!("CARGO_PKG_VERSION").contains('-');
    let list: Vec<serde_json::Value> = get(http, RELEASES.into()).await?.json().await.map_err(|e| e.to_string())?;
    let (url, sums) = newest(&list, testing).ok_or("no release has a rootshell build yet")?;
    let sums = get(http, sums).await?.text().await.map_err(|e| e.to_string())?;
    let want = sums.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if want.len() != 64 {
        return Err("the release has no valid checksum".into());
    }
    Ok((url, want))
}

/// Of GitHub's list of releases, the latest published one with a rootshell
/// build (`testing`: test builds too): its baristore.wasm and .sha256.
fn newest(list: &[serde_json::Value], testing: bool) -> Option<(String, String)> {
    let asset = |r: &serde_json::Value, name: &str| r["assets"].as_array()?.iter()
        .find(|a| a["name"] == name)?["browser_download_url"].as_str().map(str::to_string);
    list.iter()
        .filter(|r| r["draft"] != true && (testing || r["prerelease"] != true))
        .filter_map(|r| Some((r["published_at"].as_str()?, asset(r, FILE)?, asset(r, &format!("{FILE}.sha256"))?)))
        .max_by(|a, b| a.0.cmp(b.0))
        .map(|(_, url, sums)| (url, sums))
}

/// The SHA-256 of the build in ~/bin (None when it is not there).
fn installed_sha256() -> Option<String> {
    crate::rt::block_in_place(|| std::fs::read(executable()).ok().map(|b| hex::encode(Sha256::digest(&b))))
}

/// Whether a build other than this one is published.
pub async fn newer_available() -> Result<bool, String> {
    let (_, want) = published(&crate::store::client()).await?;
    Ok(installed_sha256().as_deref() != Some(want.as_str()))
}

pub async fn download_and_replace(progress: &dyn Fn(String)) -> Result<Outcome, String> {
    let http = crate::http::Client::builder().user_agent(concat!("BariStore/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(600)).build();
    progress("Checking for a newer build".into());
    let (url, want) = published(&http).await?;
    if installed_sha256().as_deref() == Some(want.as_str()) {
        return Ok(Outcome::UpToDate);
    }
    let mut resp = get(&http, url).await?;
    let total = resp.content_length().unwrap_or(0);
    let mut bytes = Vec::with_capacity(total as usize);
    let mut shown = None;
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("download failed: {e}"))? {
        bytes.extend_from_slice(&chunk);
        // Once per megabyte, not per chunk.
        let mb = bytes.len() / 1_000_000;
        if total > 0 && shown != Some(mb) {
            shown = Some(mb);
            progress(format!("Downloading {mb} of {} MB", total / 1_000_000));
        }
    }
    if hex::encode(Sha256::digest(&bytes)) != want {
        return Err("the download does not match its checksum; nothing was changed".into());
    }
    let version = version_of(&bytes).filter(|_| bytes.starts_with(b"\0asm"))
        .ok_or("the download is not a BariStore build; nothing was changed")?;
    let exe = executable();
    if let Some(dir) = exe.parent() {
        crate::rootshell::files::make_dirs(dir).map_err(|e| e.to_string())?;
    }
    let staged = exe.with_extension("new");
    std::fs::write(&staged, &bytes).map_err(|e| format!("cannot write {}: {e}", staged.display()))?;
    crate::rootshell::files::replace(&staged, &exe).map_err(|e| format!("cannot replace {}: {e}", exe.display()))?;
    Ok(Outcome::Updated(version))
}

#[cfg(test)]
mod tests {
    /// The latest published build, whatever order GitHub lists them in (by
    /// version: a higher one published earlier comes first).
    #[test]
    fn the_newest_build_by_date() {
        let release = |tag: &str, at: &str, pre: bool, wasm: bool| {
            let mut assets = vec![serde_json::json!({"name": "baristore-aarch64", "browser_download_url": format!("{tag}/a")})];
            if wasm {
                assets.push(serde_json::json!({"name": "baristore.wasm", "browser_download_url": format!("{tag}/w")}));
                assets.push(serde_json::json!({"name": "baristore.wasm.sha256", "browser_download_url": format!("{tag}/s")}));
            }
            serde_json::json!({"tag_name": tag, "published_at": at, "prerelease": pre, "draft": false, "assets": assets})
        };
        let list = [release("v2.0.0-alpha.2", "2026-11-28T06:02:01Z", true, true),
                    release("v1.2.0-alpha.3", "2026-11-28T11:56:22Z", true, true),
                    release("v1.1.2", "2026-11-27T13:52:45Z", false, false)];
        assert_eq!(super::newest(&list, true), Some(("v1.2.0-alpha.3/w".into(), "v1.2.0-alpha.3/s".into())));
        assert_eq!(super::newest(&list, false), None);
    }

    #[test]
    fn a_build_names_its_version() {
        let mut bytes = b"\0asm\x01\0\0\0 lots of code ".to_vec();
        bytes.extend_from_slice(super::MARK.as_bytes());
        bytes.extend_from_slice(b"more");
        assert_eq!(super::version_of(&bytes).as_deref(), Some(env!("CARGO_PKG_VERSION")));
        assert_eq!(super::version_of(b"\0asm nothing"), None);
    }
}
