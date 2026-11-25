//! Smaller conveniences around the main pipeline: calibration page, shareable redacted
//! copies, folder watching, update checks, archive clean-up, clipboard paste and
//! "Open with" registration.

use image::{GrayImage, Luma};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

type Rect = (u32, u32, u32, u32);

// ───────────────────────────── calibration page ─────────────────────────────

/// A test page at the label's exact size: a 1-dot border (shows clipping), millimetre
/// rulers on every edge, a centre cross, and line/checker patches that show how evenly
/// the head prints.
pub fn calibration_page(width_mm: f32, height_mm: f32, dpi: f32, offset: (f32, f32)) -> GrayImage {
    let mm = dpi / 25.4;
    let (w, h) = ((width_mm * mm).round() as u32, (height_mm * mm).round() as u32);
    let mut g = GrayImage::from_pixel(w, h, Luma([255]));
    let fill = |g: &mut GrayImage, x0: i64, y0: i64, x1: i64, y1: i64| {
        for y in y0.max(0)..y1.min(h as i64) {
            for x in x0.max(0)..x1.min(w as i64) {
                g.put_pixel(x as u32, y as u32, Luma([0]));
            }
        }
    };
    // Border, exactly on the outermost dots.
    fill(&mut g, 0, 0, w as i64, 1);
    fill(&mut g, 0, h as i64 - 1, w as i64, h as i64);
    fill(&mut g, 0, 0, 1, h as i64);
    fill(&mut g, w as i64 - 1, 0, w as i64, h as i64);
    // Rulers: 1 mm ticks, longer every 5, labelled every 10.
    let t = (mm * 0.25).max(1.0) as i64;
    let mut labels = Vec::new();
    for i in 0..=(width_mm as i64) {
        let x = (i as f32 * mm).round() as i64;
        let len = if i % 10 == 0 { 4.0 } else if i % 5 == 0 { 2.5 } else { 1.5 } * mm;
        fill(&mut g, x, 0, x + t, len as i64);
        fill(&mut g, x, h as i64 - len as i64, x + t, h as i64);
        if i % 10 == 0 && i > 0 && i < width_mm as i64 {
            labels.push((x as f32 - mm * 1.2, 4.6 * mm, i.to_string()));
        }
    }
    for i in 0..=(height_mm as i64) {
        let y = (i as f32 * mm).round() as i64;
        let len = if i % 10 == 0 { 4.0 } else if i % 5 == 0 { 2.5 } else { 1.5 } * mm;
        fill(&mut g, 0, y, len as i64, y + t);
        fill(&mut g, w as i64 - len as i64, y, w as i64, y + t);
        if i % 10 == 0 && i > 0 && i < height_mm as i64 {
            labels.push((4.8 * mm, y as f32 - mm * 1.0, i.to_string()));
        }
    }
    for (x, y, s) in labels {
        crate::ocr::draw_text(&mut g, x, y, 2.2 * mm, &s, false);
    }
    // Centre cross.
    let (cx, cy) = ((w / 2) as i64, (h / 2) as i64);
    fill(&mut g, cx - (10.0 * mm) as i64, cy - t / 2, cx + (10.0 * mm) as i64, cy + t / 2 + 1);
    fill(&mut g, cx - t / 2, cy - (10.0 * mm) as i64, cx + t / 2 + 1, cy + (10.0 * mm) as i64);

    // Patches: 1-dot lines, 2-dot lines, checkerboards, solid.
    let px = 14.0 * mm;
    let top = 22.0 * mm;
    let left = (w as f32 - 4.0 * px - 3.0 * 2.0 * mm) / 2.0;
    for (k, kind) in ["1-dot lines", "2-dot lines", "checker", "solid"].iter().enumerate() {
        let x0 = (left + k as f32 * (px + 2.0 * mm)) as u32;
        let y0 = top as u32;
        for y in y0..(y0 + px as u32).min(h) {
            for x in x0..(x0 + px as u32).min(w) {
                let (dx, dy) = (x - x0, y - y0);
                let on = match k {
                    0 => dx % 2 == 0,
                    1 => (dx / 2) % 2 == 0,
                    2 => (dx + dy) % 2 == 0,
                    _ => true,
                };
                if on {
                    g.put_pixel(x, y, Luma([0]));
                }
            }
        }
        crate::ocr::draw_text(&mut g, x0 as f32, top + px + mm, 2.0 * mm, kind, false);
    }
    let mut y = h as f32 / 2.0 + 14.0 * mm;
    let lines = [
        ("ShippingLabelCraft calibration".to_string(), true),
        (format!("{width_mm} × {height_mm} mm · {dpi} dpi · {w} × {h} dots"), false),
        (format!("Offset now: {:+.1} mm across, {:+.1} mm down", offset.0, offset.1), false),
        ("Every edge should show its thin border line.".to_string(), false),
        ("A missing edge: nudge the offset away from it".to_string(), false),
        ("(Settings → Label → Alignment).".to_string(), false),
        ("Grey patches should look even and not merge.".to_string(), false),
    ];
    for (text, bold) in lines {
        let size = if bold { 4.0 } else { 2.8 } * mm;
        crate::ocr::draw_text(&mut g, 15.0 * mm, y, size, &text, bold);
        y += size * 1.45;
    }
    // Shift by the current alignment offset, exactly as a real label would be.
    let (dx, dy) = ((offset.0 * mm).round() as i64, (offset.1 * mm).round() as i64);
    if dx != 0 || dy != 0 { crate::pipeline::shift(&g, dx, dy) } else { g }
}

// ───────────────────────────── shareable copy ─────────────────────────────

/// Copy of the label with every text line and barcode turned into a coarse mosaic, so the
/// layout can be shared (bug reports, screenshots) without names, addresses or tracking data.
pub fn redacted(bitmap: &GrayImage, text: &[Rect], codes: &[Rect]) -> GrayImage {
    let mut g = bitmap.clone();
    let block = (g.width() / 60).max(6);
    for &(x, y, w, h) in text.iter().chain(codes) {
        let (x1, y1) = ((x + w).min(g.width()), (y + h).min(g.height()));
        let mut by = y;
        while by < y1 {
            let mut bx = x;
            while bx < x1 {
                let (ex, ey) = ((bx + block).min(x1), (by + block).min(y1));
                let n = ((ex - bx) * (ey - by)).max(1);
                let dark: u32 = (by..ey).flat_map(|yy| (bx..ex).map(move |xx| (xx, yy))).filter(|&(xx, yy)| bitmap.get_pixel(xx, yy)[0] < 128).count() as u32;
                // Light grey wherever there was ink, so nothing can be read back.
                let v = if dark * 8 > n { 170 } else { 235 };
                for yy in by..ey {
                    for xx in bx..ex {
                        g.put_pixel(xx, yy, Luma([v]));
                    }
                }
                bx += block;
            }
            by += block;
        }
    }
    g
}

// ───────────────────────────── folder watching ─────────────────────────────

/// Files ShippingLabelCraft writes itself; never pick these up as new labels.
fn ours(name: &str) -> bool {
    let n = name.to_lowercase();
    // "labelcraft-" is the name used before the project was renamed.
    n.starts_with("shippinglabelcraft-") || n.starts_with("labelcraft-") || n.contains("-print.") || n.starts_with("driver-capture") || n.contains("-shareable.")
}

fn is_label_file(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    matches!(ext.as_str(), "pdf" | "png" | "jpg" | "jpeg") && !ours(&name) && !name.starts_with('.')
}

pub fn default_watch_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
    home.join("Downloads")
}

/// Poll `dir` and call `found` for each label file that appears after the watcher
/// started, once its size has stopped changing (browsers write downloads in pieces).
/// Stops when `keep_going` returns false.
pub fn watch(dir: PathBuf, mut keep_going: impl FnMut() -> bool, mut found: impl FnMut(PathBuf)) {
    use std::collections::{HashMap, HashSet};
    let start = SystemTime::now();
    // Whatever is already there when watching starts is not new.
    let mut done: HashSet<PathBuf> = std::fs::read_dir(&dir).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    let mut pending: HashMap<PathBuf, (u64, u32)> = HashMap::new();
    while keep_going() {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if done.contains(&p) || !is_label_file(&p) {
                    continue;
                }
                let Ok(m) = e.metadata() else { continue };
                // Downloads keep the server's date on some systems, so creation time counts too.
                let fresh = [m.modified().ok(), m.created().ok()].into_iter().flatten().any(|t| t >= start);
                if !m.is_file() || !fresh {
                    done.insert(p);
                    continue;
                }
                let size = m.len();
                let entry = pending.entry(p.clone()).or_insert((size, 0));
                if entry.0 == size && size > 0 {
                    entry.1 += 1;
                } else {
                    *entry = (size, 0);
                }
                if entry.1 >= 2 {
                    pending.remove(&p);
                    done.insert(p.clone());
                    found(p);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
}

// ───────────────────────────── update check ─────────────────────────────

/// The latest release on GitHub, if it's newer than this build: (version, page URL).
pub fn newer_release() -> Option<(String, String)> {
    let v: serde_json::Value = ureq::get("https://api.github.com/repos/civarson/shippinglabelcraft/releases/latest")
        .set("User-Agent", concat!("ShippingLabelCraft/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(8))
        .call()
        .ok()?
        .into_json()
        .ok()?;
    let tag = v.get("tag_name")?.as_str()?.trim_start_matches('v').to_string();
    let url = v.get("html_url")?.as_str()?.to_string();
    (version_key(&tag) > version_key(env!("CARGO_PKG_VERSION"))).then_some((tag, url))
}

fn version_key(v: &str) -> Vec<u64> {
    v.split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect()
}

// ───────────────────────────── archive clean-up ─────────────────────────────

/// Delete archived prints older than `days` from `dir`. Only touches files ShippingLabelCraft
/// wrote there (named `shippinglabelcraft-….png`, or `labelcraft-….png` from before the
/// rename). Returns how many were removed.
pub fn purge_archive(dir: &Path, days: u32) -> usize {
    if days == 0 {
        return 0;
    }
    let cutoff = SystemTime::now() - Duration::from_secs(days as u64 * 86_400);
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !((name.starts_with("shippinglabelcraft-") || name.starts_with("labelcraft-")) && name.ends_with(".png")) {
            continue;
        }
        let old = e.metadata().and_then(|m| m.modified()).map(|t| t < cutoff).unwrap_or(false);
        if old && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

// ───────────────────────────── clipboard ─────────────────────────────

/// An image on the clipboard (a screenshot of a label), saved to a temporary PNG; or a
/// file path that was copied as text.
pub fn clipboard_label() -> Result<PathBuf, String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    if let Ok(img) = cb.get_image() {
        let (w, h) = (img.width as u32, img.height as u32);
        let rgba = image::RgbaImage::from_raw(w, h, img.bytes.into_owned()).ok_or("clipboard image is malformed")?;
        let gray = image::DynamicImage::ImageRgba8(rgba).to_luma8();
        let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let path = std::env::temp_dir().join(format!("pasted-label-{secs}.png"));
        gray.save(&path).map_err(|e| e.to_string())?;
        return Ok(path);
    }
    if let Ok(text) = cb.get_text() {
        let p = PathBuf::from(text.trim().trim_matches('"'));
        if p.is_file() && is_label_file(&p) {
            return Ok(p);
        }
    }
    Err("The clipboard has no image or label file".into())
}

// ───────────────────────────── "Open with" (Windows) ─────────────────────────────

/// Register ShippingLabelCraft in Explorer's "Open with" list for PDF and images (current user
/// only, no admin rights). The registration points at wherever the app is now.
#[cfg(windows)]
pub fn register_open_with(enable: bool) -> Result<(), String> {
    use std::process::Command;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy();
    let reg = |args: &[&str]| -> Result<(), String> {
        let out = Command::new("reg").args(args).output().map_err(|e| e.to_string())?;
        if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
    };
    let progid = r"HKCU\Software\Classes\ShippingLabelCraft.Label";
    let exts = [".pdf", ".png", ".jpg", ".jpeg"];
    // Keys from before the project was renamed (LabelCraft).
    let _ = reg(&["delete", r"HKCU\Software\Classes\LabelCraft.Label", "/f"]);
    let _ = reg(&["delete", r"HKCU\Software\Classes\Applications\LabelCraft.exe", "/f"]);
    for ext in exts {
        let _ = reg(&["delete", &format!(r"HKCU\Software\Classes\{ext}\OpenWithProgids"), "/v", "LabelCraft.Label", "/f"]);
    }
    if enable {
        reg(&["add", progid, "/ve", "/d", "Shipping label (ShippingLabelCraft)", "/f"])?;
        reg(&["add", &format!(r"{progid}\DefaultIcon"), "/ve", "/d", &format!("\"{exe}\",0"), "/f"])?;
        reg(&["add", &format!(r"{progid}\shell\open\command"), "/ve", "/d", &format!("\"{exe}\" \"%1\""), "/f"])?;
        let app = r"HKCU\Software\Classes\Applications\ShippingLabelCraft.exe";
        reg(&["add", app, "/v", "FriendlyAppName", "/d", "ShippingLabelCraft", "/f"])?;
        reg(&["add", &format!(r"{app}\shell\open\command"), "/ve", "/d", &format!("\"{exe}\" \"%1\""), "/f"])?;
        for ext in exts {
            reg(&["add", &format!(r"HKCU\Software\Classes\{ext}\OpenWithProgids"), "/v", "ShippingLabelCraft.Label", "/t", "REG_NONE", "/f"])?;
            reg(&["add", &format!(r"{app}\SupportedTypes"), "/v", ext, "/d", "", "/f"])?;
        }
    } else {
        let _ = reg(&["delete", progid, "/f"]);
        let _ = reg(&["delete", r"HKCU\Software\Classes\Applications\ShippingLabelCraft.exe", "/f"]);
        for ext in exts {
            let _ = reg(&["delete", &format!(r"HKCU\Software\Classes\{ext}\OpenWithProgids"), "/v", "ShippingLabelCraft.Label", "/f"]);
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn register_open_with(_enable: bool) -> Result<(), String> {
    Err("Only needed on Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(version_key("0.10.0") > version_key("0.9.3"));
        assert!(version_key("0.6.0") > version_key("0.5.2"));
        assert!(version_key("0.5.2") == version_key("0.5.2"));
    }

    #[test]
    fn ignores_own_files() {
        assert!(!is_label_file(Path::new("/d/label-print.pdf")));
        assert!(!is_label_file(Path::new("/d/shippinglabelcraft-x-123.png")));
        assert!(!is_label_file(Path::new("/d/notes.txt")));
        assert!(is_label_file(Path::new("/d/ebay-label-02.pdf")));
    }

    #[test]
    fn watcher_waits_for_stable_files() {
        let dir = std::env::temp_dir().join(format!("lc-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.pdf"), b"x").unwrap();
        let d2 = dir.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2500));
            std::fs::write(d2.join("new-label.pdf"), b"%PDF").unwrap();
            std::fs::write(d2.join("new-label-print.pdf"), b"%PDF").unwrap();
        });
        let mut got = Vec::new();
        let begun = std::time::Instant::now();
        watch(dir.clone(), || begun.elapsed() < Duration::from_secs(8), |p| got.push(p));
        writer.join().unwrap();
        let names: Vec<String> = got.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["new-label.pdf"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
