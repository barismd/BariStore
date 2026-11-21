//! From an IPA to an app on this iPhone: App IDs (one per app/extension),
//! app groups for the apps that need one, optional Increased Memory Limit,
//! profiles, signing, the loopback install, the pairing file written into
//! the apps that refresh themselves, and our record of what is installed.

use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::apple::gsa::{Account, GrandSlam};
use crate::apple::portal::{Portal, Team};
use crate::device::{services, session::Session};
use crate::sign::bundle::{self, Ipa, Kind, Plan, Signing};
use crate::sign::codesign::Identity;
use crate::{der, logf, pairing, state};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Options {
    /// Leave app extensions out (saves App IDs: a free team gets 10 a week).
    pub remove_extensions: bool,
    /// Ask Apple for Increased Memory Limit on the main App ID.
    pub increased_memory: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installed {
    pub bundle_id: String,       // as installed (with .TEAMID)
    pub original_id: String,
    pub name: String,
    pub version: String,
    pub source_url: String,      // where the IPA came from (for refresh)
    pub ipa_path: String,        // cached original IPA
    pub team: String,
    pub expires: u64,            // unix seconds (profile expiry)
    pub installed_at: u64,
    pub options: Options,
    pub icon_url: String,
}

pub fn registry_path() -> std::path::PathBuf {
    state::path("installed.json")
}

pub fn installed() -> Vec<Installed> {
    std::fs::read(registry_path()).ok()
        .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_installed(list: &[Installed]) {
    if let Ok(b) = serde_json::to_vec_pretty(list) {
        let _ = state::write_private(&registry_path(), &b);
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Apps whose own code needs an app group (and reads ALTAppGroups), with the
/// group id each looks up (behaviour as documented by nab138/isideload).
fn group_for(original_main: &str, team: &str, ipa: &Ipa) -> Option<String> {
    let base = original_main.trim_end_matches(".Beta");
    let has_sidestore_framework = ipa.files.keys().any(|p| p.contains("/Frameworks/SideStoreApp.framework/")
        || p.contains("/Frameworks/SideStore.framework/"));
    if base == "com.kdt.livecontainer" && has_sidestore_framework {
        return Some(format!("group.com.SideStore.SideStore.{team}"));
    }
    if base == "com.SideStore.SideStore" || base == "com.rileytestut.AltStore" || base == "com.kdt.livecontainer" {
        return Some(format!("group.{original_main}.{team}"));
    }
    None
}

pub fn new_identifier(original: &str, team: &str) -> String {
    if original.ends_with(&format!(".{team}")) { original.to_string() } else { format!("{original}.{team}") }
}

pub struct Context<'a> {
    pub gs: &'a GrandSlam,
    pub account: &'a Account,
    pub team: &'a Team,
    pub udid: &'a str,
    pub device_name: &'a str,
}

/// Prepares App IDs/profiles and signs. Returns the signed IPA and the main
/// app's new bundle id and profile expiry.
pub async fn prepare_and_sign(ctx: &Context<'_>, ipa_bytes: &[u8], opts: &Options,
                              status: &(dyn Fn(String) + Send + Sync)) -> Result<(Vec<u8>, String, u64, String), String> {
    let portal = Portal { gs: ctx.gs, account: ctx.account };
    let team = ctx.team.id.as_str();
    let mut ipa = Ipa::read(ipa_bytes)?;
    if opts.remove_extensions {
        let n = ipa.drop_extensions()?;
        logf!("install: removed {n} extension files");
    }
    let bundles = ipa.bundles()?;
    let main = ipa.main_app()?;
    let main_new = new_identifier(&main.id, team);

    status("Registering this iPhone".into());
    portal.ensure_device(team, ctx.udid, ctx.device_name).await.map_err(|e| e.to_string())?;
    status("Getting the signing certificate".into());
    let (leaf, key) = portal.certificate(team).await.map_err(|e| e.to_string())?;
    let leaf_info = der::cert_info(&leaf)?;
    let common_name = der::name_value(leaf_info.subject, "2.5.4.3").unwrap_or_default();

    let group = group_for(&main.id, team, &ipa);
    let mut plan_bundles = HashMap::new();
    let mut info_edits: HashMap<String, Dictionary> = HashMap::new();
    let mut expires = u64::MAX;
    for b in bundles.iter().filter(|b| b.kind != Kind::Framework) {
        let new_id = if b.dir == main.dir {
            main_new.clone()
        } else if let Some(rest) = b.id.strip_prefix(&format!("{}.", main.id)) {
            format!("{main_new}.{rest}")
        } else {
            format!("{main_new}.{}", b.id.rsplit('.').next().unwrap_or("extension"))
        };
        status(format!("App ID for {}", b.name));
        let mut app_id = portal.ensure_app_id(team, &new_id, &b.name).await
            .map_err(|e| if e.code == 9401 || e.message.contains("maximum") {
                format!("{e}. A free Apple ID can create 10 App IDs per 7 days; try again later or turn on \
                         \"Remove extensions\" for this app.")
            } else { e.to_string() })?;
        if let Some(group) = &group {
            if portal.assign_group(team, &app_id, group).await.map_err(|e| e.to_string())? {
                let mut d = Dictionary::new();
                d.insert("ALTAppGroups".into(), Value::Array(vec![Value::String(group.clone())]));
                info_edits.insert(b.dir.clone(), d);
                app_id = portal.app_ids(team).await.map_err(|e| e.to_string())?
                    .into_iter().find(|a| a.id == app_id.id).unwrap_or(app_id);
            }
        }
        if opts.increased_memory && b.dir == main.dir {
            status("Adding Increased Memory Limit".into());
            if let Err(e) = portal.add_increased_memory(team, &app_id).await {
                logf!("install: increased memory refused: {e}");
                status(format!("Increased Memory Limit was refused: {e}"));
            }
        }
        status(format!("Profile for {}", b.name));
        let profile = portal.profile(team, &app_id).await.map_err(|e| e.to_string())?;
        let p = bundle::profile_plist(&profile)?;
        if let Some(Value::Date(d)) = p.get("ExpirationDate") {
            let t: std::time::SystemTime = (*d).into();
            let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            expires = expires.min(secs);
        }
        let mut entitlements = p.get("Entitlements").and_then(Value::as_dictionary).cloned()
            .ok_or("the profile has no entitlements")?;
        entitlements.insert("application-identifier".into(), Value::String(format!("{team}.{new_id}")));
        entitlements.insert("com.apple.developer.team-identifier".into(), Value::String(team.into()));
        plan_bundles.insert(b.dir.clone(), Signing { new_id, profile, entitlements });
    }

    let mut chain = Vec::new();
    for cert in [&include_bytes!("../assets/certs/AppleWWDRCAG3.cer")[..],
                 &include_bytes!("../assets/certs/AppleWWDRCAG4.cer")[..],
                 &include_bytes!("../assets/certs/AppleWWDRCAG5.cer")[..],
                 &include_bytes!("../assets/certs/AppleWWDRCAG6.cer")[..],
                 &include_bytes!("../assets/certs/AppleWWDRCAG2.cer")[..]] {
        if der::cert_info(cert).map(|c| c.subject == leaf_info.issuer).unwrap_or(false) {
            chain.push(cert.to_vec());
            chain.push(include_bytes!("../assets/certs/AppleRootCA.cer").to_vec());
            break;
        }
    }
    if chain.is_empty() {
        return Err("the certificate Apple issued chains to an Apple CA BariStore does not know".into());
    }
    let plan = Plan {
        identity: Identity { leaf, chain, key, team: team.into(), common_name },
        bundles: plan_bundles,
        info_edits,
        add_files: vec![],
    };
    status("Signing".into());
    bundle::sign(&mut ipa, &plan, &mut |n, t| status(format!("Signing {n}/{t}")))?;
    let out = ipa.write()?;
    Ok((out, main_new, if expires == u64::MAX { 0 } else { expires }, main.version))
}

/// Writes the pairing record into apps that refresh themselves over the
/// loopback. Destinations as SideInstaller and StikDebug document them:
/// SideStore nightlies (>= 0.7.0-20260920) read Documents/PairingFile_RemoteRP.plist
/// once their preferences say isPairingReset = false and
/// activePairingProtocol = rppairing; older SideStore and LiveContainer's
/// built-in one read ALTPairingFile.mobiledevicepairing; AltStore Classic
/// 2.3.1+ reads Documents/ALTPairingFile.plist (not the Beta).
pub async fn place_pairing_file(session: &Session, installed_id: &str, original_id: &str,
                                record: &pairing::record::PairingRecord) -> Result<bool, String> {
    let (files, prefs): (&[&str], Option<String>) = if original_id.contains(".Beta") {
        return Ok(false);
    } else if original_id.starts_with("com.SideStore.SideStore") {
        (&["/Documents/ALTPairingFile.mobiledevicepairing", "/Documents/PairingFile_RemoteRP.plist"],
         Some(format!("/Library/Preferences/{installed_id}.plist")))
    } else if original_id.starts_with("com.kdt.livecontainer") {
        (&["/Documents/SideStore/Documents/ALTPairingFile.mobiledevicepairing",
           "/Documents/SideStore/Documents/PairingFile_RemoteRP.plist"],
         Some("/Documents/SideStore/Library/Preferences/com.SideStore.SideStore.plist".into()))
    } else if original_id.starts_with("com.rileytestut.AltStore") {
        (&["/Documents/ALTPairingFile.plist"], None)
    } else {
        return Ok(false);
    };
    let mut afc = services::app_container(session, installed_id, false).await?;
    let mkdirs = |path: &str| {
        let mut dirs = Vec::new();
        let mut current = String::new();
        let parent = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        for part in parent.split('/').filter(|p| !p.is_empty()) {
            current.push('/');
            current.push_str(part);
            dirs.push(current.clone());
        }
        dirs
    };
    let xml = record.to_remote_only_xml();
    for path in files {
        for dir in mkdirs(path) {
            let _ = afc.mkdir(&dir).await;
        }
        afc.write_file(path, &xml, &mut |_, _| {}).await?;
    }
    if let Some(prefs_path) = prefs {
        for dir in mkdirs(&prefs_path) {
            let _ = afc.mkdir(&dir).await;
        }
        let existing = afc.read_file(&prefs_path, 1 << 20).await.unwrap_or_default();
        let mut prefs: Dictionary = if existing.is_empty() { Dictionary::new() } else {
            plist::from_bytes::<Value>(&existing).ok().and_then(Value::into_dictionary).unwrap_or_default()
        };
        prefs.insert("isPairingReset".into(), Value::Boolean(false));
        prefs.insert("activePairingProtocol".into(), Value::String("rppairing".into()));
        afc.write_file(&prefs_path, &crate::pairing::record::plist_xml(&prefs), &mut |_, _| {}).await?;
    }
    logf!("install: pairing file written into {installed_id}");
    Ok(true)
}

/// Install signed bytes and remember the app.
#[allow(clippy::too_many_arguments)]
pub async fn install_signed(session: &Session, signed: &[u8], entry: Installed,
                            status: &(dyn Fn(String) + Send + Sync)) -> Result<(), String> {
    services::install(session, signed, &mut |stage, pct| status(format!("{stage} {pct}%"))).await?;
    let mut list = installed();
    list.retain(|i| i.bundle_id != entry.bundle_id);
    list.push(Installed { installed_at: now(), ..entry });
    save_installed(&list);
    Ok(())
}
