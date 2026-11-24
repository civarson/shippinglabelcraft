//! Barcode detection, exact regeneration and verification (zxing-cpp + bundled zint).
//!
//! A barcode is only replaced when:
//! 1. it decodes from the source,
//! 2. it can be re-encoded from the exact decoded content (raw bytes, or GS1 element
//!    strings for GS1 symbols), and
//! 3. the regenerated symbol, as it sits in the final 1-bit label, decodes to the
//!    identical content: same symbology, same raw bytes, same symbology identifier,
//!    same text.
//!
//! Otherwise the cleaned-up original pixels are kept and the report says why.

use image::{GrayImage, Luma, imageops};
use zxingcpp::{Barcode, BarcodeFormat as F, ContentType, TextMode};

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Replaced with a freshly generated symbol that re-decoded identically.
    Regenerated { module_px: u32 },
    /// Original (cleaned) pixels kept.
    Kept(String),
    /// Seen in the final label (scan-only mode).
    Readable,
}

#[derive(Clone, Debug)]
pub struct BarcodeReport {
    pub format: String,
    pub text: String,
    pub status: Status,
    /// Region in label pixels (x, y, w, h).
    pub region: (u32, u32, u32, u32),
}

impl BarcodeReport {
    pub fn summary(&self) -> String {
        let t = &self.text;
        let t = if t.chars().count() > 40 { format!("{}…", t.chars().take(40).collect::<String>()) } else { t.clone() };
        match &self.status {
            Status::Regenerated { module_px: 0 } => format!("✔ {} regenerated & verified: {t}", self.format),
            Status::Regenerated { module_px } => {
                format!("✔ {} regenerated & verified ({module_px}-dot modules): {t}", self.format)
            }
            Status::Kept(why) => format!("• {} kept as original ({why}): {t}", self.format),
            Status::Readable => format!("✔ {} reads OK: {t}", self.format),
        }
    }
}

type Rect = (u32, u32, u32, u32);
type Quad = [(f32, f32); 4];

/// Symbologies found on shipping labels. EAN/UPC/ITF are left out on purpose: fragments of
/// other barcodes and text produce false positives for them.
const READ_FORMATS: [F; 8] =
    [F::Code128, F::Code39, F::Code93, F::PDF417, F::DataMatrix, F::QRCode, F::MaxiCode, F::Aztec];

fn is_linear(f: F) -> bool {
    matches!(f, F::Code128 | F::Code39 | F::Code93)
}

fn read(img: &GrayImage, formats: &[F]) -> Vec<Barcode> {
    zxingcpp::read()
        .formats(formats)
        .try_harder(true)
        .try_rotate(true)
        .try_invert(false)
        .text_mode(TextMode::HRI)
        .from(img)
        .unwrap_or_default()
}

/// Content fingerprint used for verification.
#[derive(PartialEq, Debug, Clone)]
struct Signature {
    symbology: F,
    bytes: Vec<u8>,
    symbology_id: String,
    content_type: ContentType,
    text: String,
}

impl Signature {
    /// Same payload as far as any scanner is concerned. Text vs. Binary is only the
    /// decoder's guess about identical bytes, so those two count as equal.
    fn same_payload(&self, o: &Signature) -> bool {
        let ct = |c: ContentType| match c {
            ContentType::Binary => ContentType::Text,
            c => c,
        };
        self.symbology == o.symbology
            && self.bytes == o.bytes
            && self.symbology_id == o.symbology_id
            && ct(self.content_type) == ct(o.content_type)
            && self.text == o.text
    }
}

fn signature(b: &Barcode) -> Signature {
    Signature {
        symbology: b.format().symbology(),
        bytes: b.bytes(),
        symbology_id: b.symbology_identifier(),
        content_type: b.content_type(),
        text: b.text(),
    }
}

/// A decoded symbol with its corner points in working-image coordinates.
struct Hit {
    b: Barcode,
    quad: Quad,
}

impl Hit {
    fn new(b: Barcode, off: (f32, f32)) -> Self {
        let p = b.position();
        let quad = [
            (p.top_left.x as f32 + off.0, p.top_left.y as f32 + off.1),
            (p.top_right.x as f32 + off.0, p.top_right.y as f32 + off.1),
            (p.bottom_right.x as f32 + off.0, p.bottom_right.y as f32 + off.1),
            (p.bottom_left.x as f32 + off.0, p.bottom_left.y as f32 + off.1),
        ];
        Self { b, quad }
    }
}

fn bbox(c: &Quad, scale: f32, w: u32, h: u32) -> Rect {
    let x0 = c.iter().map(|p| p.0).fold(f32::MAX, f32::min) / scale;
    let y0 = c.iter().map(|p| p.1).fold(f32::MAX, f32::min) / scale;
    let x1 = c.iter().map(|p| p.0).fold(f32::MIN, f32::max) / scale;
    let y1 = c.iter().map(|p| p.1).fold(f32::MIN, f32::max) / scale;
    let x0 = (x0.floor().max(0.0) as u32).min(w - 1);
    let y0 = (y0.floor().max(0.0) as u32).min(h - 1);
    let x1 = (x1.ceil().max(0.0) as u32).min(w - 1);
    let y1 = (y1.ceil().max(0.0) as u32).min(h - 1);
    (x0, y0, (x1 - x0 + 1).max(1), (y1 - y0 + 1).max(1))
}

fn overlap(a: Rect, b: Rect) -> f32 {
    let x0 = a.0.max(b.0);
    let y0 = a.1.max(b.1);
    let x1 = (a.0 + a.2).min(b.0 + b.2);
    let y1 = (a.1 + a.3).min(b.1 + b.3);
    if x1 <= x0 || y1 <= y0 {
        return 0.0;
    }
    ((x1 - x0) * (y1 - y0)) as f32 / ((a.2 * a.3).min(b.2 * b.3)) as f32
}

/// Angle (radians, image coords, y down) of the reading direction of the longest 1D barcode.
pub fn reading_angle(img: &GrayImage) -> Option<f32> {
    read(img, &[F::Code128, F::Code39, F::Code93])
        .into_iter()
        .map(|b| {
            let c = Hit::new(b, (0.0, 0.0)).quad;
            let (dx, dy) = (c[1].0 - c[0].0, c[1].1 - c[0].1);
            ((dx * dx + dy * dy).sqrt(), dy.atan2(dx))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, a)| a)
}

/// Find every symbol: one pass over the whole working image, then a second pass over
/// isolated blobs of ink. The second pass is what finds MaxiCodes (zxing only reads those
/// when they're on their own) and small 2D codes the full-page scan missed.
fn find_all(work: &GrayImage, ws: u32, bitmap: &GrayImage) -> Vec<Hit> {
    let (bw, bh) = bitmap.dimensions();
    let mut hits: Vec<Hit> = read(work, &READ_FORMATS).into_iter().map(|b| Hit::new(b, (0.0, 0.0))).collect();

    // Blobs on the 1-bit label: dilate ink slightly so a 2D symbol becomes one component.
    let dark: Vec<bool> = bitmap.as_raw().iter().map(|&p| p < 128).collect();
    let grown = crate::pipeline::dilate(&dark, bw, bh, 3);
    let dpi_guess = bw as f32 / 4.0; // label is ~4" wide
    let (lo, hi) = ((0.30 * dpi_guess) as u32, (2.6 * dpi_guess) as u32);
    let mut known: Vec<Rect> = hits.iter().map(|h| bbox(&h.quad, ws as f32, bw, bh)).collect();
    for c in crate::pipeline::components(&grown, bw, bh) {
        let r = (c.x0, c.y0, c.x1 - c.x0 + 1, c.y1 - c.y0 + 1);
        if r.2 < lo || r.3 < lo || r.2 > hi || r.3 > hi || r.2.max(r.3) > 3 * r.2.min(r.3) {
            continue;
        }
        if known.iter().any(|k| overlap(*k, r) > 0.5) {
            continue;
        }
        // Copy just this blob (everything else white) out of the working image.
        let pad = 16 * ws;
        let (x0, y0) = ((r.0 * ws).saturating_sub(pad), (r.1 * ws).saturating_sub(pad));
        let x1 = ((r.0 + r.2) * ws + pad).min(work.width());
        let y1 = ((r.1 + r.3) * ws + pad).min(work.height());
        let mut iso = GrayImage::from_pixel(x1 - x0, y1 - y0, Luma([255]));
        for y in y0..y1 {
            for x in x0..x1 {
                let (lx, ly) = (x / ws, y / ws);
                if lx >= c.x0 && lx <= c.x1 && ly >= c.y0 && ly <= c.y1 && grown[(ly * bw + lx) as usize] {
                    iso.put_pixel(x - x0, y - y0, *work.get_pixel(x, y));
                }
            }
        }
        for b in read(&iso, &READ_FORMATS) {
            let h = Hit::new(b, (x0 as f32, y0 as f32));
            let hr = bbox(&h.quad, ws as f32, bw, bh);
            if known.iter().any(|k| overlap(*k, hr) > 0.5) {
                continue;
            }
            known.push(hr);
            hits.push(h);
        }
    }
    hits
}

/// Report what's readable in the final label, without changing it.
pub fn scan_only(bitmap: &GrayImage) -> Vec<BarcodeReport> {
    find_all(bitmap, 1, bitmap)
        .into_iter()
        .map(|h| BarcodeReport {
            format: h.b.format().to_string(),
            text: h.b.text(),
            status: Status::Readable,
            region: bbox(&h.quad, 1.0, bitmap.width(), bitmap.height()),
        })
        .collect()
}

/// Detect on the high-res working image, regenerate into the 1-bit label.
pub fn regenerate_all(work: &GrayImage, work_scale: u32, bitmap: &mut GrayImage) -> Vec<BarcodeReport> {
    let ws = work_scale as f32;
    let (bw, bh) = bitmap.dimensions();
    let mut hits = find_all(work, work_scale, bitmap);
    // Big symbols first, so a small one never gets drawn and then painted over.
    hits.sort_by_key(|h| {
        let r = bbox(&h.quad, ws, bw, bh);
        std::cmp::Reverse(r.2 * r.3)
    });
    let mut reports = Vec::new();
    for h in &hits {
        let sym = h.b.format().symbology();
        let mut rep = BarcodeReport {
            format: h.b.format().to_string(),
            text: h.b.text(),
            status: Status::Kept(String::new()),
            region: bbox(&h.quad, ws, bw, bh),
        };
        let outcome = if h.b.has_eci() {
            Err("uses ECI character sets".to_string())
        } else if is_linear(sym) {
            regen_linear(h, ws, bitmap)
        } else if sym == F::MaxiCode {
            regen_maxicode(h, ws, bitmap)
        } else {
            regen_matrix(h, ws, bitmap)
        };
        match outcome {
            Ok((region, module_px)) => {
                rep.region = region;
                rep.status = Status::Regenerated { module_px };
            }
            Err(why) => rep.status = Status::Kept(why),
        }
        reports.push(rep);
    }
    reports
}

/// Re-encode the decoded content of `b` with creator `options`.
fn create(b: &Barcode, options: &str) -> Result<Barcode, String> {
    let gs1 = b.content_type() == ContentType::GS1;
    let opts = match (gs1, options.is_empty()) {
        (true, true) => "gs1".to_string(),
        (true, false) => format!("gs1, {options}"),
        (false, _) => options.to_string(),
    };
    let creator = zxingcpp::create(b.format().symbology()).options(opts);
    let bytes = b.bytes();
    let res = if gs1 {
        // HRI text, e.g. "(420)78701(92)0019…", is the GS1 element string form.
        creator.from_str(b.text())
    } else if let (Ok(s), false) = (std::str::from_utf8(&bytes), bytes.contains(&0)) {
        creator.from_str(s)
    } else {
        creator.from_slice(&bytes)
    };
    res.map_err(|e| format!("encoder: {e}"))
}

/// Rasterise a created symbol at one pixel per module, no quiet zone.
fn module_grid(b: &Barcode) -> Result<GrayImage, String> {
    let img = b
        .to_image_with(&zxingcpp::write().scale(1).add_quiet_zones(false).add_hrt(false))
        .map_err(|e| format!("render: {e}"))?;
    Ok((&img).into())
}

/// The region on its own with a white margin (so verification judges this symbol alone).
fn isolate(img: &GrayImage, r: Rect, pad: u32) -> GrayImage {
    let inner = imageops::crop_imm(img, r.0, r.1, r.2, r.3).to_image();
    let mut canvas = GrayImage::from_pixel(r.2 + 2 * pad, r.3 + 2 * pad, Luma([255]));
    imageops::overlay(&mut canvas, &inner, pad as i64, pad as i64);
    canvas
}

fn fill(img: &mut GrayImage, r: Rect, v: u8) {
    for y in r.1..(r.1 + r.3).min(img.height()) {
        for x in r.0..(r.0 + r.2).min(img.width()) {
            img.put_pixel(x, y, Luma([v]));
        }
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    let x0 = a.0.min(b.0);
    let y0 = a.1.min(b.1);
    (x0, y0, (a.0 + a.2).max(b.0 + b.2) - x0, (a.1 + a.3).max(b.1 + b.3) - y0)
}

/// Erase `erase`, let `draw` paint the new symbol inside it, then verify. On failure the
/// original pixels are restored.
fn replace_verified(
    bitmap: &mut GrayImage,
    erase: Rect,
    pad: u32,
    want: &Signature,
    draw: impl FnOnce(&mut GrayImage),
) -> Result<(), String> {
    let backup = imageops::crop_imm(bitmap, erase.0, erase.1, erase.2, erase.3).to_image();
    fill(bitmap, erase, 255);
    draw(bitmap);
    let crop = isolate(bitmap, erase, pad);
    let got = read(&crop, &[want.symbology]);
    if std::env::var_os("LABELCRAFT_DEBUG").is_some() {
        let _ = crop.save(format!("debug_verify_{:?}.png", want.symbology));
        eprintln!("want {want:?}");
        for g in &got {
            eprintln!(" got {:?}", signature(g));
        }
    }
    if got.iter().any(|g| want.same_payload(&signature(g))) {
        Ok(())
    } else {
        imageops::replace(bitmap, &backup, erase.0 as i64, erase.1 as i64);
        Err(if got.is_empty() { "regenerated symbol didn't re-scan".into() } else { "regenerated content differed".into() })
    }
}

fn regen_linear(h: &Hit, ws: f32, bitmap: &mut GrayImage) -> Result<(Rect, u32), String> {
    let (bw, bh) = bitmap.dimensions();
    let c = &h.quad;
    let (dx, dy) = (c[1].0 - c[0].0, c[1].1 - c[0].1);
    if dx <= 0.0 || dy.abs() > 0.01 * dx {
        return Err("barcode isn't straight".into());
    }
    let r = bbox(c, ws, bw, bh);
    let (x0, x1) = (r.0, r.0 + r.2 - 1);
    let orig_w = r.2 as f32;

    let new = create(&h.b, "")?;
    let grid = module_grid(&new)?;
    let modules = grid.width();

    // Whole-dot module width giving the closest overall width to the original.
    let mut x = (orig_w / modules as f32).round().max(1.0) as u32;
    while x > 1 && modules * x + 8 > bw {
        x -= 1;
    }
    let new_w = modules * x;
    let center = (x0 + x1) as f32 / 2.0;
    let nx0 = (center - new_w as f32 / 2.0).round();
    if nx0 < 0.0 || nx0 as u32 + new_w > bw {
        return Err("regenerated symbol wouldn't fit".into());
    }
    let nx0 = nx0 as u32;

    let (ys, ye) = bar_vertical_extent(bitmap, x0, x1, r.1, r.1 + r.3 - 1);
    let hgt = ye - ys + 1;
    if hgt < 8 {
        return Err("couldn't measure bar height".into());
    }
    let xs1 = x0.saturating_sub(1);
    let erase = union((xs1, ys, (x1 + 2).min(bw) - xs1, hgt), (nx0, ys, new_w, hgt));
    replace_verified(bitmap, erase, 12 * x + 6, &signature(&h.b), |bm| {
        for i in 0..modules {
            if grid.get_pixel(i, 0)[0] < 128 {
                fill(bm, (nx0 + i * x, ys, x, hgt), 0);
            }
        }
    })?;
    Ok(((nx0, ys, new_w, hgt), x))
}

/// Vertical extent of the bars: start from the rows the decoder used, then walk outward
/// while each new row still looks like the row next to it (follows slight skew) and still
/// has bar-like edge density.
fn bar_vertical_extent(img: &GrayImage, xs: u32, xe: u32, y0: u32, y1: u32) -> (u32, u32) {
    let row = |yy: u32| -> Vec<bool> { (xs..=xe).map(|x| img.get_pixel(x, yy)[0] < 128).collect() };
    let edges = |r: &[bool]| r.windows(2).filter(|w| w[0] != w[1]).count();
    let mid = (y0 + y1) / 2;
    let ref_edges = edges(&row(mid)).max(1);
    let accept = |prev: &[bool], cur: &[bool]| -> bool {
        let same = prev.iter().zip(cur).filter(|(a, b)| a == b).count();
        same as f32 / cur.len() as f32 >= 0.9 && edges(cur) * 2 >= ref_edges
    };
    let mut top = mid;
    let mut prev = row(mid);
    while top > 0 {
        let cur = row(top - 1);
        if !accept(&prev, &cur) {
            break;
        }
        top -= 1;
        prev = cur;
    }
    let mut bot = mid;
    let mut prev = row(mid);
    while bot + 1 < img.height() {
        let cur = row(bot + 1);
        if !accept(&prev, &cur) {
            break;
        }
        bot += 1;
        prev = cur;
    }
    (top, bot)
}

fn regen_matrix(h: &Hit, ws: f32, bitmap: &mut GrayImage) -> Result<(Rect, u32), String> {
    let (bw, bh) = bitmap.dimensions();
    let region = bbox(&h.quad, ws, bw, bh);
    // Original module sizes are rarely whole printer dots; allow the new symbol to be up to
    // ~8% larger than the old footprint so it can reach the next whole-dot size.
    let room = (region.2 * 108 / 100 + 1, region.3 * 108 / 100 + 1);

    let option_sets: Vec<String> = match h.b.format().symbology() {
        // Try column counts so the symbol's shape matches the space it came from.
        F::PDF417 => (1..=20).map(|c| format!("columns={c}")).collect(),
        // Highest error correction that still fits.
        F::QRCode => ["ecLevel=H", "ecLevel=Q", "ecLevel=M", "ecLevel=L"].iter().map(|s| s.to_string()).collect(),
        _ => vec![String::new()],
    };
    let mut best: Option<(GrayImage, u32)> = None;
    let mut last_err = String::from("couldn't fit a regenerated symbol in the original space");
    for opts in &option_sets {
        let grid = match create(&h.b, opts).and_then(|n| module_grid(&n)) {
            Ok(g) => g,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        let s = (room.0 / grid.width()).min(room.1 / grid.height());
        if s == 0 {
            continue;
        }
        let area = grid.width() * grid.height() * s * s;
        let better = match &best {
            None => true,
            Some((g, bs)) => (s, area) > (*bs, g.width() * g.height() * bs * bs),
        };
        if better {
            best = Some((grid, s));
        }
    }
    let (grid, s) = best.ok_or(last_err)?;
    let (nw, nh) = (grid.width() * s, grid.height() * s);
    let cx = region.0 as i64 + region.2 as i64 / 2;
    let cy = region.1 as i64 + region.3 as i64 / 2;
    let (nx0, ny0) = (cx - nw as i64 / 2, cy - nh as i64 / 2);
    if nx0 < 0 || ny0 < 0 || nx0 + nw as i64 > bw as i64 || ny0 + nh as i64 > bh as i64 {
        return Err("regenerated symbol wouldn't fit".into());
    }
    let (nx0, ny0) = (nx0 as u32, ny0 as u32);
    let erase = union(region, (nx0, ny0, nw, nh));
    replace_verified(bitmap, erase, (nw.max(nh) / 5).max(16), &signature(&h.b), |bm| {
        for (x, y, p) in grid.enumerate_pixels() {
            if p[0] < 128 {
                fill(bm, (nx0 + x * s, ny0 + y * s, s, s), 0);
            }
        }
    })?;
    Ok(((nx0, ny0, nw, nh), s))
}

/// MaxiCode is a fixed-size hexagonal symbol, so it can't be snapped to whole-dot modules.
/// Render it large and resample to the original footprint.
fn regen_maxicode(h: &Hit, ws: f32, bitmap: &mut GrayImage) -> Result<(Rect, u32), String> {
    let (bw, bh) = bitmap.dimensions();
    let region = bbox(&h.quad, ws, bw, bh);
    let new = create(&h.b, "")?;
    let big: GrayImage = (&new
        .to_image_with(&zxingcpp::write().scale(8).add_quiet_zones(false).add_hrt(false))
        .map_err(|e| format!("render: {e}"))?)
        .into();
    // Keep the symbol's own aspect ratio inside the original footprint.
    let f = (region.2 as f32 / big.width() as f32).min(region.3 as f32 / big.height() as f32);
    let (nw, nh) = ((big.width() as f32 * f).round() as u32, (big.height() as f32 * f).round() as u32);
    if nw < 60 {
        return Err("too small to redraw".into());
    }
    let small = imageops::resize(&big, nw, nh, imageops::FilterType::Triangle);
    let nx0 = region.0 + (region.2 - nw) / 2;
    let ny0 = region.1 + (region.3 - nh) / 2;
    replace_verified(bitmap, region, 24, &signature(&h.b), |bm| {
        for (x, y, p) in small.enumerate_pixels() {
            if p[0] < 128 {
                bm.put_pixel(nx0 + x, ny0 + y, Luma([0]));
            }
        }
    })?;
    Ok(((nx0, ny0, nw, nh), 0))
}
