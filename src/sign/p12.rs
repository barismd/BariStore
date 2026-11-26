//! PKCS#12 (RFC 7292) for the certificate AltStore-family apps sign with:
//! SideStore and AltStore read `ALTCertificate.p12` from their bundle (with
//! `ALTCertificateID` in Info.plist naming the certificate) and open it with
//! the certificate's portal machine identifier as the password; without it
//! they ask Apple for a new certificate or to revoke one. The key bag uses
//! PBES2 (PBKDF2-HMAC-SHA256, AES-256-CBC), the MAC HMAC-SHA256, as current
//! OpenSSL and Apple's Security framework write them. `open` reads such a
//! file back (Options › Apple ID › Certificates › Import).

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use rsa::pkcs1::EncodeRsaPublicKey;
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use rsa::RsaPrivateKey;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::der;

const ITERATIONS: u64 = 2048;
const DATA: &str = "1.2.840.113549.1.7.1";

fn bmp(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

/// RFC 7292 appendix B.2 key derivation with SHA-256 (u = 32, v = 64).
fn pkcs12_kdf(password: &str, salt: &[u8], id: u8, iterations: u64, n: usize) -> Vec<u8> {
    const U: usize = 32;
    const V: usize = 64;
    let fill = |src: &[u8]| -> Vec<u8> {
        if src.is_empty() {
            return Vec::new();
        }
        let len = V * src.len().div_ceil(V);
        src.iter().copied().cycle().take(len).collect()
    };
    let mut pw = bmp(password);
    pw.extend_from_slice(&[0, 0]);
    let mut i: Vec<u8> = [fill(salt), fill(&pw)].concat();
    let mut out = Vec::new();
    while out.len() < n {
        let mut h = Sha256::new();
        h.update([id; V]);
        h.update(&i);
        let mut a = h.finalize().to_vec();
        for _ in 1..iterations {
            a = Sha256::digest(&a).to_vec();
        }
        let b: Vec<u8> = a.iter().copied().cycle().take(V).collect();
        for block in i.chunks_mut(V) {
            let mut carry = 1u16;
            for k in (0..V).rev() {
                let sum = block[k] as u16 + b[k] as u16 + carry;
                block[k] = sum as u8;
                carry = sum >> 8;
            }
        }
        out.extend_from_slice(&a[..U.min(n - out.len())]);
    }
    out
}

fn content_info_data(content: &[u8]) -> Vec<u8> {
    der::seq(&[der::oid(DATA), der::ctx(0, &der::octets(content))])
}

fn attributes(local_key_id: &[u8], name: &str) -> Vec<u8> {
    der::set(&[
        der::seq(&[der::oid("1.2.840.113549.1.9.20"), der::set(&[der::tlv(0x1E, &bmp(name))])]),
        der::seq(&[der::oid("1.2.840.113549.1.9.21"), der::set(&[der::octets(local_key_id)])]),
    ])
}

/// The certificate and its key as a .p12 protected by `password`.
pub fn pkcs12(cert: &[u8], key: &RsaPrivateKey, password: &str, name: &str) -> Result<Vec<u8>, String> {
    let pkcs8 = key.to_pkcs8_der().map_err(|e| e.to_string())?;
    let local_key_id = Sha1::digest(cert).to_vec();
    let attrs = attributes(&local_key_id, name);

    let cert_bag = der::seq(&[
        der::oid("1.2.840.113549.1.12.10.1.3"),
        der::ctx(0, &der::seq(&[der::oid("1.2.840.113549.1.9.22.1"), der::ctx(0, &der::octets(cert))])),
        attrs.clone(),
    ]);

    let salt: [u8; 16] = rand::random();
    let iv: [u8; 16] = rand::random();
    let mut aes_key = [0u8; 32];
    pbkdf2::pbkdf2::<Hmac<Sha256>>(password.as_bytes(), &salt, ITERATIONS as u32, &mut aes_key)
        .map_err(|e| e.to_string())?;
    let sealed = cbc::Encryptor::<aes::Aes256>::new(&aes_key.into(), &iv.into())
        .encrypt_padded_vec_mut::<Pkcs7>(pkcs8.as_bytes());
    let pbes2 = der::seq(&[
        der::oid("1.2.840.113549.1.5.13"),
        der::seq(&[
            der::seq(&[
                der::oid("1.2.840.113549.1.5.12"),
                der::seq(&[der::octets(&salt), der::int(ITERATIONS),
                           der::seq(&[der::oid("1.2.840.113549.2.9"), der::null()])]),
            ]),
            der::seq(&[der::oid("2.16.840.1.101.3.4.1.42"), der::octets(&iv)]),
        ]),
    ]);
    let key_bag = der::seq(&[
        der::oid("1.2.840.113549.1.12.10.1.2"),
        der::ctx(0, &der::seq(&[pbes2, der::octets(&sealed)])),
        attrs,
    ]);

    let auth_safe = der::seq(&[
        content_info_data(&der::seq(&[cert_bag])),
        content_info_data(&der::seq(&[key_bag])),
    ]);
    let mac_salt: [u8; 16] = rand::random();
    let mac_key = pkcs12_kdf(password, &mac_salt, 3, ITERATIONS, 32);
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&mac_key).map_err(|e| e.to_string())?;
    mac.update(&auth_safe);
    let digest = mac.finalize().into_bytes();
    let mac_data = der::seq(&[
        der::seq(&[der::seq(&[der::oid("2.16.840.1.101.3.4.2.1"), der::null()]), der::octets(&digest)]),
        der::octets(&mac_salt),
        der::int(ITERATIONS),
    ]);
    Ok(der::seq(&[der::int(3), content_info_data(&auth_safe), mac_data]))
}

const ENCRYPTED_DATA: &str = "1.2.840.113549.1.7.6";
const KEY_BAG: &str = "1.2.840.113549.1.12.10.1.1";
const SHROUDED_KEY_BAG: &str = "1.2.840.113549.1.12.10.1.2";
const CERT_BAG: &str = "1.2.840.113549.1.12.10.1.3";
const OLD_ENCRYPTION: &str = "This .p12 uses an older encryption BariStore cannot open. Export the certificate from \
    BariStore (Options › Apple ID › Certificates › Export) and import that file.";

fn is(t: &der::Tlv, dotted: &str) -> bool {
    t.raw == der::oid(dotted).as_slice()
}

/// The parts of a SEQUENCE (or of an explicit [n] wrapper).
fn parts<'a>(t: &der::Tlv<'a>) -> Result<Vec<der::Tlv<'a>>, String> {
    if t.tag & 0x20 == 0 {
        return Err("not a .p12 file BariStore can read".into());
    }
    der::children(t.content)
}

fn first<'a>(content: &'a [u8]) -> Result<der::Tlv<'a>, String> {
    der::read(content).map(|(t, _)| t)
}

/// PBES2: PBKDF2 (HMAC-SHA1 or -SHA256) and AES-128/192/256-CBC.
fn decrypt(alg: &der::Tlv, data: &[u8], password: &str) -> Result<Vec<u8>, String> {
    let alg = parts(alg)?;
    if alg.len() < 2 || !is(&alg[0], "1.2.840.113549.1.5.13") {
        return Err(OLD_ENCRYPTION.into());
    }
    let scheme = parts(&alg[1])?;
    let (kdf, enc) = (parts(scheme.first().ok_or(OLD_ENCRYPTION)?)?, parts(scheme.get(1).ok_or(OLD_ENCRYPTION)?)?);
    if kdf.len() < 2 || !is(&kdf[0], "1.2.840.113549.1.5.12") || enc.len() < 2 {
        return Err(OLD_ENCRYPTION.into());
    }
    let params = parts(&kdf[1])?;
    let salt = params.first().filter(|t| t.tag == 0x04).ok_or(OLD_ENCRYPTION)?.content;
    let iterations = params.get(1).filter(|t| t.tag == 0x02 && t.content.len() <= 4).ok_or(OLD_ENCRYPTION)?
        .content.iter().fold(0u32, |n, b| (n << 8) | u32::from(*b));
    if iterations == 0 || iterations > 1 << 24 {
        return Err(OLD_ENCRYPTION.into());
    }
    let sha256 = match params.iter().find(|t| t.tag == 0x30).map(parts).transpose()?.as_deref() {
        None => false,
        Some([prf, ..]) if is(prf, "1.2.840.113549.2.7") => false,
        Some([prf, ..]) if is(prf, "1.2.840.113549.2.9") => true,
        _ => return Err(OLD_ENCRYPTION.into()),
    };
    let size = [("2.16.840.1.101.3.4.1.2", 16), ("2.16.840.1.101.3.4.1.22", 24), ("2.16.840.1.101.3.4.1.42", 32)]
        .iter().find(|(oid, _)| is(&enc[0], oid)).map(|(_, n)| *n).ok_or(OLD_ENCRYPTION)?;
    let iv: [u8; 16] = enc[1].content.try_into().map_err(|_| OLD_ENCRYPTION)?;
    let mut key = vec![0u8; size];
    let derived = if sha256 {
        pbkdf2::pbkdf2::<Hmac<Sha256>>(password.as_bytes(), salt, iterations, &mut key)
    } else {
        pbkdf2::pbkdf2::<Hmac<Sha1>>(password.as_bytes(), salt, iterations, &mut key)
    };
    derived.map_err(|e| e.to_string())?;
    let opened = match size {
        16 => cbc::Decryptor::<aes::Aes128>::new_from_slices(&key, &iv).map_err(|e| e.to_string())?
            .decrypt_padded_vec_mut::<Pkcs7>(data),
        24 => cbc::Decryptor::<aes::Aes192>::new_from_slices(&key, &iv).map_err(|e| e.to_string())?
            .decrypt_padded_vec_mut::<Pkcs7>(data),
        _ => cbc::Decryptor::<aes::Aes256>::new_from_slices(&key, &iv).map_err(|e| e.to_string())?
            .decrypt_padded_vec_mut::<Pkcs7>(data),
    };
    opened.map_err(|_| "Wrong password.".to_string())
}

/// The content of a ContentInfo of type data: [0] { OCTET STRING }.
fn data_content<'a>(info: &[der::Tlv<'a>]) -> Result<&'a [u8], String> {
    match info {
        [kind, wrapped, ..] if is(kind, DATA) && wrapped.tag == 0xA0 => {
            let octets = first(wrapped.content)?;
            if octets.tag != 0x04 {
                return Err("not a .p12 file BariStore can read".into());
            }
            Ok(octets.content)
        }
        _ => Err("not a .p12 file BariStore can read".into()),
    }
}

/// The certificate and RSA key a .p12 holds (PBES2 encryption, as
/// BariStore, OpenSSL 3 and current Apple tools write it).
pub fn open(p12: &[u8], password: &str) -> Result<(Vec<u8>, RsaPrivateKey), String> {
    let pfx = parts(&first(p12)?)?;
    let auth_safe = data_content(&parts(pfx.get(1).ok_or("not a .p12 file BariStore can read")?)?)?;
    let (mut certs, mut keys) = (Vec::new(), Vec::new());
    for info in parts(&first(auth_safe)?)? {
        let info = parts(&info)?;
        let bags = if info.first().is_some_and(|k| is(k, ENCRYPTED_DATA)) {
            // [0] { EncryptedData { version, { type, algorithm, [0] IMPLICIT content } } }
            let encrypted = parts(&first(info.get(1).ok_or(OLD_ENCRYPTION)?.content)?)?;
            let content = parts(encrypted.get(1).ok_or(OLD_ENCRYPTION)?)?;
            let sealed = content.get(2).filter(|t| t.tag == 0x80).ok_or(OLD_ENCRYPTION)?;
            decrypt(content.get(1).ok_or(OLD_ENCRYPTION)?, sealed.content, password)?
        } else {
            data_content(&info)?.to_vec()
        };
        for bag in parts(&first(&bags)?)? {
            let bag = parts(&bag)?;
            let (Some(kind), Some(value)) = (bag.first(), bag.get(1)) else { continue };
            let value = first(value.content)?;
            if is(kind, CERT_BAG) {
                let cert = parts(&value)?;
                if let Some(wrapped) = cert.get(1) {
                    certs.push(first(wrapped.content)?.content.to_vec());
                }
            } else if is(kind, KEY_BAG) {
                keys.push(value.raw.to_vec());
            } else if is(kind, SHROUDED_KEY_BAG) {
                let shrouded = parts(&value)?;
                let sealed = shrouded.get(1).ok_or("not a .p12 file BariStore can read")?;
                keys.push(decrypt(&shrouded[0], sealed.content, password)?);
            }
        }
    }
    let key = keys.first().ok_or("The .p12 holds no private key.")?;
    let key = RsaPrivateKey::from_pkcs8_der(key).map_err(|_| "Wrong password, or not an RSA key.".to_string())?;
    let public = key.to_public_key().to_pkcs1_der().map_err(|e| e.to_string())?;
    let cert = certs.into_iter().find(|c| der::cert_info(c)
        .is_ok_and(|i| i.spki.windows(public.as_bytes().len()).any(|w| w == public.as_bytes())))
        .ok_or("The .p12 holds no certificate for its key.")?;
    Ok((cert, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openssl_opens_it() {
        let dir = std::env::temp_dir().join(format!("baristore-p12-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key_path = dir.join("k.pem");
        let cert_path = dir.join("c.der");
        let ok = std::process::Command::new("openssl")
            .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=Test", "-days", "1",
                   "-outform", "DER", "-keyout", key_path.to_str().unwrap(), "-out", cert_path.to_str().unwrap()])
            .output().unwrap().status.success();
        assert!(ok);
        use rsa::pkcs8::DecodePrivateKey;
        let key = RsaPrivateKey::from_pkcs8_pem(&std::fs::read_to_string(&key_path).unwrap()).unwrap();
        let cert = std::fs::read(&cert_path).unwrap();
        let p12 = pkcs12(&cert, &key, "A1B2C3D4-MACHINE", "BariStore").unwrap();
        let p12_path = dir.join("t.p12");
        std::fs::write(&p12_path, p12).unwrap();
        let out = std::process::Command::new("openssl")
            .args(["pkcs12", "-in", p12_path.to_str().unwrap(), "-passin", "pass:A1B2C3D4-MACHINE", "-nodes"])
            .output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(text.contains("BEGIN CERTIFICATE") && text.contains("BEGIN PRIVATE KEY"));
        assert!(text.contains("friendlyName: BariStore"));
        let wrong = std::process::Command::new("openssl")
            .args(["pkcs12", "-in", p12_path.to_str().unwrap(), "-passin", "pass:wrong", "-nodes"])
            .output().unwrap();
        assert!(!wrong.status.success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Our own .p12 and one OpenSSL 3 wrote (AES-256, its certificate bag
    /// encrypted too) open with their password, and only with it.
    #[test]
    fn opens_ours_and_openssls() {
        let dir = std::env::temp_dir().join(format!("baristore-p12-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (key_path, cert_path) = (dir.join("k.pem"), dir.join("c.pem"));
        assert!(std::process::Command::new("openssl")
            .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=Test", "-days", "1",
                   "-keyout", key_path.to_str().unwrap(), "-out", cert_path.to_str().unwrap()])
            .output().unwrap().status.success());
        let der_path = dir.join("c.der");
        assert!(std::process::Command::new("openssl")
            .args(["x509", "-in", cert_path.to_str().unwrap(), "-outform", "DER", "-out", der_path.to_str().unwrap()])
            .output().unwrap().status.success());
        let key = RsaPrivateKey::from_pkcs8_pem(&std::fs::read_to_string(&key_path).unwrap()).unwrap();
        let cert = std::fs::read(&der_path).unwrap();
        let ours = pkcs12(&cert, &key, "pw-1", "BariStore").unwrap();
        let (c, k) = open(&ours, "pw-1").unwrap();
        assert_eq!((c, k), (cert.clone(), key.clone()));
        assert_eq!(open(&ours, "pw-2").unwrap_err(), "Wrong password.");
        let theirs = dir.join("o.p12");
        let out = std::process::Command::new("openssl")
            .args(["pkcs12", "-export", "-inkey", key_path.to_str().unwrap(), "-in", cert_path.to_str().unwrap(),
                   "-passout", "pass:secret", "-out", theirs.to_str().unwrap()])
            .output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let theirs = std::fs::read(&theirs).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(open(&theirs, "secret").unwrap(), (cert, key));
        assert!(open(&theirs, "wrong").is_err());
        assert!(open(b"not a p12", "x").is_err());
    }
}
