//! App icons and screenshots, served from BariStore's own cache: each one is
//! fetched once, shrunk (PNG/JPEG; other formats are kept as they are) and
//! kept in ~/.baristore/icons. Sources link images of up to several
//! megabytes on slow hosts; the page then loads small copies from the
//! iPhone itself. Screenshots must be shrunk: a 1290×2796 one takes 14 MB
//! once decoded, and a page of them would take the web view's memory.

use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;

use crate::{logf, state};

/// Icons fit 120 px; screenshots 640 px (Featured's tiles) or 1100 px (an
/// app's page). The page asks only for these.
pub const ICON: u32 = 120;
pub const TILE: u32 = 640;
pub const SIZES: [u32; 3] = [ICON, TILE, 1100];
const MAX_BYTES: usize = 8 << 20;

fn path(url: &str, size: u32) -> PathBuf {
    let key = if size == ICON { url.to_string() } else { format!("{url}@{size}") };
    state::path("icons").join(hex::encode(&Sha256::digest(key.as_bytes())[..12]))
}

/// One icon and one screenshot are decoded at a time (Minis's shell has
/// little memory); icons never wait behind a screenshot.
static ICON_TURN: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
static SHOT_TURN: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// The MIME type of image bytes, from their magic number.
pub fn kind(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, ..] => "image/jpeg",
        [b'G', b'I', b'F', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        _ if bytes.windows(4).take(256).any(|w| w == b"<svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// What shrinking found: a smaller copy, an image already small enough, or
/// one that could not be decoded within the limits.
#[derive(Debug)]
enum Shrunk {
    Smaller(Vec<u8>),
    Fits,
    Unreadable,
}

/// Fits the image into `size`×`size`: PNG for icons (transparency), JPEG
/// for screenshots (a tenth of the bytes).
fn shrink(bytes: &[u8], size: u32) -> Shrunk {
    // Bounded: Minis's shell has little memory, and iOS takes some back in
    // the background.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(3000);
    limits.max_image_height = Some(3000);
    limits.max_alloc = Some(40 << 20);
    let Ok(mut reader) = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format() else {
        return Shrunk::Unreadable;
    };
    reader.limits(limits);
    let Ok(img) = reader.decode() else { return Shrunk::Unreadable };
    if img.width() <= size && img.height() <= size {
        return Shrunk::Fits;
    }
    let small = img.thumbnail(size, size);
    drop(img);
    let mut out = std::io::Cursor::new(Vec::new());
    let written = if size == ICON {
        small.write_to(&mut out, image::ImageFormat::Png)
    } else {
        image::DynamicImage::ImageRgb8(small.into_rgb8()).write_to(&mut out, image::ImageFormat::Jpeg)
    };
    match written {
        Ok(()) => Shrunk::Smaller(out.into_inner()),
        Err(_) => Shrunk::Unreadable,
    }
}

pub fn cached(url: &str, size: u32) -> Option<Vec<u8>> {
    std::fs::read(path(url, size)).ok().filter(|b| !b.is_empty())
}

/// The image at `url` fitted into `size` (one of SIZES), from the cache or
/// fetched (and cached) now. An icon that cannot be decoded is kept as it
/// is (SVG, WebP); a screenshot is not, it would be too big.
pub async fn get(http: &reqwest::Client, url: &str, size: u32) -> Result<Vec<u8>, String> {
    if !SIZES.contains(&size) {
        return Err("no such size".into());
    }
    if let Some(bytes) = cached(url, size) {
        return Ok(bytes);
    }
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("not a web address".into());
    }
    let resp = http.get(url).timeout(Duration::from_secs(30)).send().await
        .and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES || kind(&bytes) == "application/octet-stream" {
        return Err("not an image".into());
    }
    let original = bytes.to_vec();
    let shrunk = {
        let _turn = if size == ICON { &ICON_TURN } else { &SHOT_TURN }.acquire().await.map_err(|e| e.to_string())?;
        tokio::task::spawn_blocking(move || shrink(&original, size)).await.map_err(|e| e.to_string())?
    };
    let stored = match shrunk {
        Shrunk::Smaller(small) => small,
        Shrunk::Fits => bytes.to_vec(),
        Shrunk::Unreadable if size == ICON => bytes.to_vec(),
        Shrunk::Unreadable => return Err("the screenshot is too big or not readable".into()),
    };
    let file = path(url, size);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&file, &stored);
    Ok(stored)
}

/// Fetches the images not cached yet at `size` (`at_once` at a time), so
/// opening the store later shows them at once.
pub async fn prefetch(urls: Vec<String>, size: u32, at_once: usize) {
    use futures_util::StreamExt;
    let http = crate::store::client();
    let missing: Vec<String> = urls.into_iter().filter(|u| !u.is_empty() && cached(u, size).is_none()).collect();
    if missing.is_empty() {
        return;
    }
    let started = std::time::Instant::now();
    let count = missing.len();
    futures_util::stream::iter(missing)
        .for_each_concurrent(at_once, |url| {
            let http = http.clone();
            async move {
                if let Err(e) = get(&http, &url, size).await {
                    logf!("icons: {url}: {e}");
                }
            }
        })
        .await;
    logf!("icons: {count} at {size} px fetched in {:.1} s", started.elapsed().as_secs_f64());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_png_is_shrunk_and_small_one_kept() {
        let mut big = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(512, 512, image::Rgba([20, 60, 200, 255]))
            .write_to(&mut big, image::ImageFormat::Png).unwrap();
        let Shrunk::Smaller(small) = shrink(big.get_ref(), ICON) else { panic!("not shrunk") };
        assert_eq!(kind(&small), "image/png");
        let img = image::load_from_memory(&small).unwrap();
        assert_eq!((img.width(), img.height()), (ICON, ICON));
        let mut tiny = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 255]))
            .write_to(&mut tiny, image::ImageFormat::Png).unwrap();
        assert!(matches!(shrink(tiny.get_ref(), ICON), Shrunk::Fits));
        assert!(matches!(shrink(b"not an image", ICON), Shrunk::Unreadable));
    }

    #[test]
    fn screenshot_becomes_a_small_jpeg() {
        let mut shot = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(1290, 2796, image::Rgb([240, 240, 245]))
            .write_to(&mut shot, image::ImageFormat::Png).unwrap();
        let Shrunk::Smaller(small) = shrink(shot.get_ref(), 640) else { panic!("not shrunk") };
        assert_eq!(kind(&small), "image/jpeg");
        let img = image::load_from_memory(&small).unwrap();
        assert_eq!((img.width(), img.height()), (295, 640));
    }
}
