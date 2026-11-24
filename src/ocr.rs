//! Opt-in OCR re-typesetting of label text, with a safety net.
//!
//! A line of text is only replaced with freshly typeset text when *all* of these hold:
//!  1. Two independent reads agree exactly: one from the high-resolution working image, one
//!     from the final 1-bit label. (Different inputs make different mistakes.)
//!  2. Both reads are confident (average and weakest-character confidence).
//!  3. Any long number in the line (tracking numbers, ZIP+4) matches a barcode's data, if the
//!     line looks like a tracking number at all.
//!  4. After typesetting, reading the *new* pixels gives back exactly the same text.
//!
//! Anything else keeps its original (cleaned-up) pixels. The GUI then asks for a review
//! before printing.
//!
//! Engine: PaddleOCR PP-OCRv6 tiny (detection + recognition, Apache-2.0) run with rten.
//! Typeface: Liberation Sans (metric-compatible with Arial/Helvetica, OFL).

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use image::{GrayImage, Luma, imageops};
use rten::{Model, Value};
use rten_tensor::prelude::*;
use rten_tensor::{NdTensor, Tensor};
use std::sync::OnceLock;

use crate::barcodes::BarcodeReport;

type Rect = (u32, u32, u32, u32);

#[derive(Clone, Debug, PartialEq)]
pub enum LineStatus {
    /// Replaced with typeset text. `barcode_match`: its digits also appear in a barcode.
    Retypeset { barcode_match: bool },
    /// Original pixels kept.
    Kept(String),
}

#[derive(Clone, Debug)]
pub struct OcrLine {
    pub text: String,
    /// Label pixels.
    pub region: Rect,
    pub confidence: f32,
    pub status: LineStatus,
}

#[derive(Clone, Copy, Debug)]
pub struct OcrSettings {
    /// Minimum average character confidence (0–1) for both reads.
    pub min_confidence: f32,
}

impl Default for OcrSettings {
    fn default() -> Self {
        Self { min_confidence: 0.9 }
    }
}

struct Engine {
    det: Model,
    rec: Model,
    /// Index 0 = CTC blank, 1..=n = dictionary, n+1 = space.
    chars: Vec<String>,
    regular: FontRef<'static>,
    bold: FontRef<'static>,
}

fn engine() -> Option<&'static Engine> {
    static E: OnceLock<Option<Engine>> = OnceLock::new();
    E.get_or_init(|| {
        let det = Model::load_static_slice(include_bytes!("../assets/ocr/det.onnx")).ok()?;
        let rec = Model::load_static_slice(include_bytes!("../assets/ocr/rec.onnx")).ok()?;
        let mut chars = vec![String::new()];
        chars.extend(include_str!("../assets/ocr/dict.txt").lines().map(String::from));
        chars.push(" ".into());
        let regular = FontRef::try_from_slice(include_bytes!("../assets/fonts/LiberationSans-Regular.ttf")).ok()?;
        let bold = FontRef::try_from_slice(include_bytes!("../assets/fonts/LiberationSans-Bold.ttf")).ok()?;
        Some(Engine { det, rec, chars, regular, bold })
    })
    .as_ref()
}

/// One read of a line: text plus per-character confidences.
#[derive(Debug, Clone)]
struct Read {
    text: String,
    confs: Vec<f32>,
}

impl Read {
    fn mean(&self) -> f32 {
        if self.confs.is_empty() { 0.0 } else { self.confs.iter().sum::<f32>() / self.confs.len() as f32 }
    }
    fn min(&self) -> f32 {
        self.confs.iter().cloned().fold(1.0, f32::min)
    }
}

/// Text boxes (label pixels) from the detector run on the 1× gray label.
fn detect(e: &Engine, gray: &GrayImage) -> Vec<(Rect, Rect)> {
    let (w, h) = (gray.width().div_ceil(32) * 32, gray.height().div_ceil(32) * 32);
    let mut padded = GrayImage::from_pixel(w, h, Luma([255]));
    imageops::overlay(&mut padded, gray, 0, 0);
    let mean = [0.485f32, 0.456, 0.406];
    let std = [0.229f32, 0.224, 0.225];
    let n = (w * h) as usize;
    let mut data = vec![0f32; 3 * n];
    for c in 0..3 {
        for (i, p) in padded.as_raw().iter().enumerate() {
            data[c * n + i] = (*p as f32 / 255.0 - mean[c]) / std[c];
        }
    }
    let input = Tensor::from_data(&[1, 3, h as usize, w as usize], data);
    let Ok(out) = e.det.run_one(Value::from(input).into(), None) else { return Vec::new() };
    let Some(prob) = out.into_tensor::<f32>() else { return Vec::new() };
    let prob: Vec<f32> = prob.iter().copied().collect();
    if prob.len() != n {
        return Vec::new();
    }
    // DB post-processing: threshold, components, box score, unclip.
    let on: Vec<bool> = prob.iter().map(|&v| v > 0.3).collect();
    let mut seen = vec![false; n];
    let mut boxes = Vec::new();
    let (wi, hi) = (w as usize, h as usize);
    for start in 0..n {
        if !on[start] || seen[start] {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1, mut cnt, mut score) = (usize::MAX, usize::MAX, 0, 0, 0usize, 0f32);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % wi, i / wi);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            cnt += 1;
            score += prob[i];
            for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < 0 || ny < 0 || nx >= wi as i64 || ny >= hi as i64 {
                    continue;
                }
                let j = ny as usize * wi + nx as usize;
                if on[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        if cnt < 12 || score / (cnt as f32) < 0.5 {
            continue;
        }
        let (bw, bh) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32);
        let d = (bw * bh * 1.6 / (2.0 * (bw + bh))).round() as i64;
        let ex0 = (x0 as i64 - d).max(0) as u32;
        let ey0 = (y0 as i64 - d).max(0) as u32;
        let ex1 = ((x1 as i64 + d) as u32).min(gray.width() - 1);
        let ey1 = ((y1 as i64 + d) as u32).min(gray.height() - 1);
        let core = (x0 as u32, y0 as u32, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32);
        boxes.push((core, (ex0, ey0, ex1 - ex0 + 1, ey1 - ey0 + 1)));
    }
    boxes
}

/// Recognise one cropped line (any resolution).
fn recognise(e: &Engine, crop: &GrayImage) -> Option<Read> {
    if crop.width() < 4 || crop.height() < 4 {
        return None;
    }
    let nh = 48u32;
    let nw = ((crop.width() as f32 * nh as f32 / crop.height() as f32).round() as u32).clamp(16, 3200);
    let r = imageops::resize(crop, nw, nh, imageops::FilterType::Triangle);
    let n = (nw * nh) as usize;
    let mut d = vec![0f32; 3 * n];
    for c in 0..3 {
        for (i, p) in r.as_raw().iter().enumerate() {
            d[c * n + i] = (*p as f32 / 255.0 - 0.5) / 0.5;
        }
    }
    let out = e.rec.run_one(Value::from(Tensor::from_data(&[1, 3, nh as usize, nw as usize], d)).into(), None).ok()?;
    let p: NdTensor<f32, 3> = out.into_tensor::<f32>()?.try_into().ok()?;
    let [_, steps, classes] = p.shape();
    if classes != e.chars.len() {
        return None;
    }
    // Greedy CTC over printable ASCII only (shipping labels are Latin text).
    let allowed: Vec<bool> = e.chars.iter().enumerate().map(|(k, s)| k == 0 || s.chars().all(|c| c.is_ascii() && !c.is_control())).collect();
    let (mut text, mut confs, mut last) = (String::new(), Vec::new(), 0usize);
    for t in 0..steps {
        let mut best = (0usize, f32::MIN);
        for (k, &ok) in allowed.iter().enumerate() {
            let v = p[[0, t, k]];
            if ok && v > best.1 {
                best = (k, v);
            }
        }
        if best.0 != 0 && best.0 != last {
            text.push_str(&e.chars[best.0]);
            confs.push(best.1);
        }
        last = best.0;
    }
    Some(Read { text: text.trim().to_string(), confs })
}

fn crop_pad(img: &GrayImage, r: Rect, pad: u32) -> GrayImage {
    let inner = imageops::crop_imm(img, r.0, r.1, r.2, r.3).to_image();
    let mut c = GrayImage::from_pixel(r.2 + 2 * pad, r.3 + 2 * pad, Luma([255]));
    imageops::overlay(&mut c, &inner, pad as i64, pad as i64);
    c
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

/// Bounding box of the glyphs belonging to this line: ink components inside the expanded box
/// whose centre falls inside the detector's core box (so neighbouring lines and rules that
/// poke into the box are ignored).
fn line_ink(img: &GrayImage, core: Rect, b: Rect) -> Option<Rect> {
    let (x0, y0, w, h) = b;
    let idx = |x: u32, y: u32| ((y - y0) * w + (x - x0)) as usize;
    let mut seen = vec![false; (w * h) as usize];
    let (mut bx0, mut by0, mut bx1, mut by1) = (u32::MAX, u32::MAX, 0, 0);
    for sy in y0..y0 + h {
        for sx in x0..x0 + w {
            if seen[idx(sx, sy)] || img.get_pixel(sx, sy)[0] >= 128 {
                continue;
            }
            let (mut cx0, mut cy0, mut cx1, mut cy1) = (sx, sy, sx, sy);
            let mut touches_edge = false;
            let mut stack = vec![(sx, sy)];
            seen[idx(sx, sy)] = true;
            while let Some((x, y)) = stack.pop() {
                cx0 = cx0.min(x);
                cy0 = cy0.min(y);
                cx1 = cx1.max(x);
                cy1 = cy1.max(y);
                touches_edge |= x == x0 || y == y0 || x == x0 + w - 1 || y == y0 + h - 1;
                for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < x0 as i64 || ny < y0 as i64 || nx >= (x0 + w) as i64 || ny >= (y0 + h) as i64 {
                        continue;
                    }
                    let (nx, ny) = (nx as u32, ny as u32);
                    if !seen[idx(nx, ny)] && img.get_pixel(nx, ny)[0] < 128 {
                        seen[idx(nx, ny)] = true;
                        stack.push((nx, ny));
                    }
                }
            }
            let (mx, my) = ((cx0 + cx1) / 2, (cy0 + cy1) / 2);
            let inside = mx >= core.0 && mx < core.0 + core.2 && my >= core.1 && my < core.1 + core.3;
            if inside {
                if touches_edge {
                    return None; // part of something bigger than this line
                }
                bx0 = bx0.min(cx0);
                by0 = by0.min(cy0);
                bx1 = bx1.max(cx1);
                by1 = by1.max(cy1);
            }
        }
    }
    (bx1 >= bx0 && by1 >= by0).then(|| (bx0, by0, bx1 - bx0 + 1, by1 - by0 + 1))
}

#[cfg(any())]
fn ink_bbox(img: &GrayImage, r: Rect) -> Option<Rect> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in r.1..r.1 + r.3 {
        for x in r.0..r.0 + r.2 {
            if img.get_pixel(x, y)[0] < 128 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x1 >= x0 && y1 >= y0).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

fn coverage(img: &GrayImage, r: Rect) -> f32 {
    let mut dark = 0u32;
    for y in r.1..r.1 + r.3 {
        for x in r.0..r.0 + r.2 {
            dark += (img.get_pixel(x, y)[0] < 128) as u32;
        }
    }
    dark as f32 / (r.2 * r.3).max(1) as f32
}

/// Typeset `text` so its ink exactly fills a `w`×`h` box. Returns a coverage mask.
fn typeset(font: &FontRef<'static>, text: &str, w: u32, h: u32) -> Option<GrayImage> {
    // Measure the ink bounds at a reference size, then scale x and y independently.
    let measure = |scale: PxScale| -> Option<(f32, f32, f32, f32)> {
        let sf = font.as_scaled(scale);
        let (mut caret, mut prev) = (0.0f32, None);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for ch in text.chars() {
            let id = sf.glyph_id(ch);
            if let Some(p) = prev {
                caret += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(scale, point(caret, sf.ascent()));
            if let Some(o) = font.outline_glyph(g) {
                let b = o.px_bounds();
                x0 = x0.min(b.min.x);
                y0 = y0.min(b.min.y);
                x1 = x1.max(b.max.x);
                y1 = y1.max(b.max.y);
            }
            caret += sf.h_advance(id);
            prev = Some(id);
        }
        (x1 > x0).then_some((x0, y0, x1, y1))
    };
    let base = PxScale::from(100.0);
    let (x0, y0, x1, y1) = measure(base)?;
    let scale = PxScale { x: 100.0 * w as f32 / (x1 - x0), y: 100.0 * h as f32 / (y1 - y0) };
    let (x0, y0, ..) = measure(scale)?;
    let sf = font.as_scaled(scale);
    let mut out = GrayImage::from_pixel(w, h, Luma([0]));
    let (mut caret, mut prev) = (0.0f32, None);
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            caret += sf.kern(p, id);
        }
        let g = id.with_scale_and_position(scale, point(caret - x0, sf.ascent() - y0));
        if let Some(o) = font.outline_glyph(g) {
            let b = o.px_bounds();
            o.draw(|gx, gy, c| {
                let (x, y) = (b.min.x as i64 + gx as i64, b.min.y as i64 + gy as i64);
                if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                    let v = out.get_pixel(x as u32, y as u32)[0].max((c * 255.0) as u8);
                    out.put_pixel(x as u32, y as u32, Luma([v]));
                }
            });
        }
        caret += sf.h_advance(id);
        prev = Some(id);
    }
    Some(out)
}

fn digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Re-typeset what can be safely re-typeset. `work` is the 2× working image, `gray` the 1×
/// gray label, `bitmap` the 1-bit label (modified in place). Barcode regions are left alone.
pub fn retypeset(
    work: &GrayImage,
    ws: u32,
    gray: &GrayImage,
    bitmap: &mut GrayImage,
    barcodes: &[BarcodeReport],
    s: &OcrSettings,
) -> Result<Vec<OcrLine>, String> {
    let e = engine().ok_or("OCR engine failed to load")?;
    let (bw, bh) = bitmap.dimensions();
    let blocked: Vec<Rect> = barcodes
        .iter()
        .map(|b| {
            let r = b.region;
            (r.0.saturating_sub(6), r.1.saturating_sub(6), (r.2 + 12).min(bw), (r.3 + 12).min(bh))
        })
        .collect();
    let barcode_digits: Vec<String> = barcodes.iter().map(|b| digits(&b.text)).filter(|d| !d.is_empty()).collect();

    let mut boxes = detect(e, gray);
    boxes.sort_by_key(|(_, b)| (b.1, b.0));
    let mut lines = Vec::new();
    for (core, b) in boxes {
        if b.0 + b.2 > bw || b.1 + b.3 > bh || b.3 < 6 || b.2 < 6 {
            continue;
        }
        if blocked.iter().any(|k| overlaps(*k, b)) {
            continue;
        }
        let hi = crop_pad(work, (b.0 * ws, b.1 * ws, b.2 * ws, b.3 * ws), 4 * ws);
        let lo = crop_pad(bitmap, b, 4);
        let (Some(a), Some(c)) = (recognise(e, &hi), recognise(e, &lo)) else { continue };
        if a.text.is_empty() {
            continue;
        }
        let confidence = a.mean().min(c.mean());
        let mut line = OcrLine { text: a.text.clone(), region: b, confidence, status: LineStatus::Kept(String::new()) };
        let d = digits(&a.text);
        let barcode_match = d.len() >= 5 && barcode_digits.iter().any(|bd| bd.contains(&d));

        let refuse = |why: &str| LineStatus::Kept(why.to_string());
        line.status = if a.text != c.text {
            refuse("two reads disagreed")
        } else if confidence < s.min_confidence || a.min().min(c.min()) < 0.35 {
            refuse("not confident enough")
        } else if d.len() >= 12 && !barcode_match {
            refuse("long number doesn't match any barcode")
        } else {
            match draw_line(e, &a.text, core, b, bitmap) {
                Ok(()) => LineStatus::Retypeset { barcode_match },
                Err(why) => refuse(&why),
            }
        };
        lines.push(line);
    }
    Ok(lines)
}

/// Replace the ink of one line with typeset text, then re-read it; restore on mismatch.
fn draw_line(e: &Engine, text: &str, core: Rect, b: Rect, bitmap: &mut GrayImage) -> Result<(), String> {
    let ink = line_ink(bitmap, core, b).ok_or("no ink")?;
    if ink.3 as f32 > core.3 as f32 * 2.2 || ink.2 < 4 || ink.3 < 4 {
        return Err("text touches other graphics".into());
    }
    // Pick the weight whose ink density is closest to the original.
    let orig_cov = coverage(bitmap, ink);
    let mut best: Option<(f32, GrayImage)> = None;
    for font in [&e.regular, &e.bold] {
        if let Some(m) = typeset(font, text, ink.2, ink.3) {
            let cov = m.pixels().filter(|p| p[0] >= 128).count() as f32 / (ink.2 * ink.3) as f32;
            let diff = (cov - orig_cov).abs();
            if best.as_ref().is_none_or(|(d, _)| diff < *d) {
                best = Some((diff, m));
            }
        }
    }
    let (_, mask) = best.ok_or("font has no glyphs for this text")?;
    let backup = imageops::crop_imm(bitmap, b.0, b.1, b.2, b.3).to_image();
    for y in ink.1..ink.1 + ink.3 {
        for x in ink.0..ink.0 + ink.2 {
            let on = mask.get_pixel(x - ink.0, y - ink.1)[0] >= 128;
            bitmap.put_pixel(x, y, Luma([if on { 0 } else { 255 }]));
        }
    }
    let check = recognise(e, &crop_pad(bitmap, b, 4)).map(|r| r.text);
    if check.as_deref() == Some(text) {
        Ok(())
    } else {
        imageops::replace(bitmap, &backup, b.0 as i64, b.1 as i64);
        Err("re-typeset text didn't read back identically".into())
    }
}
