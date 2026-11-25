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

/// What a line is, as far as editing goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineKind {
    Text,
    /// Its digits also appear in a barcode (ZIP, ZIP+4…): the digits can't be edited.
    BarcodeNumber,
    /// A tracking number (12+ digits). Editable, but always double-checked.
    Tracking,
}

#[derive(Clone, Debug)]
pub struct OcrLine {
    pub text: String,
    /// Label pixels (detector box, expanded).
    pub region: Rect,
    /// Detector core box: glyphs whose centre lies inside belong to this line.
    pub core: Rect,
    pub confidence: f32,
    pub status: LineStatus,
    pub kind: LineKind,
    /// For tracking numbers: digits of the barcode that best corresponds, if any.
    pub barcode_digits: Option<String>,
}

pub fn digits_of(s: &str) -> String {
    digits(s)
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

/// Where text lines are on a label (expanded boxes), for redacting a shareable copy.
pub fn text_regions(gray: &GrayImage) -> Vec<Rect> {
    engine().map(|e| detect(e, gray).into_iter().map(|(_, b)| b).collect()).unwrap_or_default()
}

/// Draw `text` in Liberation Sans with its top-left at (x, y), `px` tall, in black.
/// Returns the width drawn.
pub fn draw_text(img: &mut GrayImage, x: f32, y: f32, px: f32, text: &str, bold: bool) -> f32 {
    let Some(e) = engine() else { return 0.0 };
    let font = if bold { &e.bold } else { &e.regular };
    let scale = PxScale::from(px);
    let sf = font.as_scaled(scale);
    let (mut caret, mut prev) = (0.0f32, None);
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            caret += sf.kern(p, id);
        }
        let g = id.with_scale_and_position(scale, point(x + caret, y + sf.ascent()));
        if let Some(o) = font.outline_glyph(g) {
            let b = o.px_bounds();
            o.draw(|gx, gy, c| {
                let (px_, py_) = (b.min.x as i64 + gx as i64, b.min.y as i64 + gy as i64);
                if c >= 0.5 && px_ >= 0 && py_ >= 0 && (px_ as u32) < img.width() && (py_ as u32) < img.height() {
                    img.put_pixel(px_ as u32, py_ as u32, Luma([0]));
                }
            });
        }
        caret += sf.h_advance(id);
        prev = Some(id);
    }
    caret
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
fn line_ink(img: &GrayImage, core: Rect, b: Rect) -> Option<(Rect, Vec<Rect>)> {
    let (x0, y0, w, h) = b;
    let idx = |x: u32, y: u32| ((y - y0) * w + (x - x0)) as usize;
    let mut seen = vec![false; (w * h) as usize];
    let (mut bx0, mut by0, mut bx1, mut by1) = (u32::MAX, u32::MAX, 0, 0);
    let mut parts = Vec::new();
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
                parts.push((cx0, cy0, cx1 - cx0 + 1, cy1 - cy0 + 1));
            }
        }
    }
    (bx1 >= bx0 && by1 >= by0).then(|| ((bx0, by0, bx1 - bx0 + 1, by1 - by0 + 1), parts))
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

/// Ink bounds of `text` laid out from caret 0 on a baseline at the font's ascent.
fn measure(font: &FontRef<'static>, text: &str, scale: PxScale) -> Option<(f32, f32, f32, f32)> {
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
}

/// Result of a hand edit.
#[derive(Clone, Debug)]
pub struct EditResult {
    /// What OCR reads back from the redrawn line (None = nothing readable).
    pub read_back: Option<String>,
    /// Horizontal squeeze applied to fit the original line's width (1.0 = none).
    pub squeeze: f32,
}

impl EditResult {
    pub fn reads_back(&self, text: &str) -> bool {
        self.read_back.as_deref().map(as_read) == Some(as_read(text))
    }
}

/// Replace one line on `bitmap` with `text`, at the size and weight of what's there now.
/// Text longer than the line is squeezed down to 70% width; beyond that it's refused.
pub fn edit_line(bitmap: &mut GrayImage, line: &OcrLine, text: &str) -> Result<EditResult, String> {
    let e = engine().ok_or("OCR engine failed to load")?;
    let text = text.trim();
    if text.is_empty() {
        return Err("A line can't be empty".into());
    }
    let b = line.region;
    let (ink, _) = line_ink(bitmap, line.core, b).ok_or("Can't find this line's letters on the label")?;
    // Size reference: the line as OCR read it, fitted to the ink box now on the label.
    let reference = if line.text.is_empty() { text } else { line.text.as_str() };
    let orig_cov = coverage(bitmap, ink);
    let mut best: Option<(f32, &FontRef<'static>, PxScale)> = None;
    for font in [&e.regular, &e.bold] {
        let Some((x0, y0, x1, y1)) = measure(font, reference, PxScale::from(100.0)) else { continue };
        let sy = 100.0 * ink.3 as f32 / (y1 - y0);
        let sx = (100.0 * ink.2 as f32 / (x1 - x0)).clamp(sy * 0.7, sy * 1.3);
        let scale = PxScale { x: sx, y: sy };
        if let Some(m) = typeset(font, reference, ink.2, ink.3) {
            let cov = m.pixels().filter(|p| p[0] >= 128).count() as f32 / (ink.2 * ink.3) as f32;
            let diff = (cov - orig_cov).abs();
            if best.as_ref().is_none_or(|(d, ..)| diff < *d) {
                best = Some((diff, font, scale));
            }
        }
    }
    let (_, font, mut scale) = best.ok_or("The font has no letters for this line")?;
    let (_, ry0, ..) = measure(font, reference, scale).ok_or("The font has no letters for this line")?;
    // Clear the old ink first so the free space beside the line can be measured.
    for y in ink.1..ink.1 + ink.3 {
        for x in ink.0..ink.0 + ink.2 {
            bitmap.put_pixel(x, y, Luma([255]));
        }
    }
    // Room to grow: blank columns beside the line (keeping a gap of ~⅓ letter height),
    // symmetric for centred headings so they stay centred.
    let (bw, bh) = bitmap.dimensions();
    let (ry0b, ry1b) = (ink.1.saturating_sub(1), (ink.1 + ink.3 + 1).min(bh));
    let blank_col = |x: u32| (ry0b..ry1b).all(|y| bitmap.get_pixel(x, y)[0] >= 128);
    let gap = (ink.3 / 3).max(3);
    let edge = 4u32;
    let mut right = ink.0 + ink.2;
    while right < bw - edge && blank_col(right) {
        right += 1;
    }
    let right_free = right.saturating_sub(ink.0 + ink.2).saturating_sub(if right < bw - edge { gap } else { 0 });
    let mut left = ink.0;
    while left > edge && blank_col(left - 1) {
        left -= 1;
    }
    let left_free = ink.0.saturating_sub(left).saturating_sub(if left > edge { gap } else { 0 });
    let centred = ((ink.0 as f32 + ink.2 as f32 / 2.0) - bw as f32 / 2.0).abs() < bw as f32 * 0.03;
    let room = if centred { ink.2 + 2 * left_free.min(right_free) } else { ink.2 + right_free };

    let (nx0, _, nx1, _) = measure(font, text, scale).ok_or("The font has no letters for this text")?;
    let squeeze = (room as f32 / (nx1 - nx0)).min(1.0);
    if squeeze < 0.7 {
        return Err(format!("Too long to fit (would need {:.0}% width): shorten it", squeeze * 100.0));
    }
    scale.x *= squeeze;
    let (nx0, _, nx1, _) = measure(font, text, scale).ok_or("The font has no letters for this text")?;
    let new_w = nx1 - nx0;
    let start = if centred { ink.0 as f32 + ink.2 as f32 / 2.0 - new_w / 2.0 } else { ink.0 as f32 };

    // Draw from the same top, clipped to the line's band and the room found above.
    let clip = (
        (start.floor().max(0.0) as u32).min(ink.0),
        b.1,
        0u32,
        b.3,
    );
    let clip_x1 = ((start + new_w).ceil() as u32 + 1).max(ink.0 + ink.2).min(bw);
    let clip = (clip.0, clip.1, clip_x1 - clip.0, clip.3);
    let sf = font.as_scaled(scale);
    let (ox, oy) = (start - nx0, ink.1 as f32 + sf.ascent() - ry0);
    let (mut caret, mut prev) = (0.0f32, None);
    let mut cover = GrayImage::new(clip.2, clip.3);
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            caret += sf.kern(p, id);
        }
        let g = id.with_scale_and_position(scale, point(ox + caret, oy));
        if let Some(o) = font.outline_glyph(g) {
            let bb = o.px_bounds();
            o.draw(|gx, gy, c| {
                let (x, y) = (bb.min.x as i64 + gx as i64 - clip.0 as i64, bb.min.y as i64 + gy as i64 - clip.1 as i64);
                if x >= 0 && y >= 0 && (x as u32) < clip.2 && (y as u32) < clip.3 {
                    let v = cover.get_pixel(x as u32, y as u32)[0].max((c * 255.0) as u8);
                    cover.put_pixel(x as u32, y as u32, Luma([v]));
                }
            });
        }
        caret += sf.h_advance(id);
        prev = Some(id);
    }
    for (x, y, p) in cover.enumerate_pixels() {
        if p[0] >= 128 {
            bitmap.put_pixel(clip.0 + x, clip.1 + y, Luma([0]));
        }
    }
    let b = clip;
    let read_back = recognise(e, &crop_pad(bitmap, b, 4)).map(|r| r.text).filter(|t| !t.is_empty());
    Ok(EditResult { read_back, squeeze })
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
        let d = digits(&a.text);
        let barcode_match = d.len() >= 5 && barcode_digits.iter().any(|bd| bd.contains(&d));
        let run_in_barcode = a
            .text
            .split(|c: char| !c.is_ascii_digit())
            .any(|run| run.len() >= 5 && barcode_digits.iter().any(|bd| bd.contains(run)));
        // A tracking number is a long number that's also in a barcode. Other long IDs
        // (label or postage IDs) are just text.
        let kind = if d.len() >= 12 && barcode_match {
            LineKind::Tracking
        } else if barcode_match || run_in_barcode {
            LineKind::BarcodeNumber
        } else {
            LineKind::Text
        };
        let tracking_barcode = (kind == LineKind::Tracking).then(|| barcode_digits.iter().find(|bd| bd.contains(&d)).cloned());
        let mut line = OcrLine {
            text: a.text.clone(),
            region: b,
            core,
            confidence,
            status: LineStatus::Kept(String::new()),
            kind,
            barcode_digits: tracking_barcode.flatten(),
        };

        let refuse = |why: &str| LineStatus::Kept(why.to_string());
        line.status = if a.text != c.text {
            refuse("two reads disagreed")
        } else if confidence < s.min_confidence || a.min().min(c.min()) < 0.35 {
            refuse("not confident enough")
        } else if d.len() >= 12 && !barcode_match {
            refuse("long number doesn't match any barcode")
        } else {
            match draw_line(e, &a.text, core, b, bitmap) {
                Ok(shown) => {
                    line.text = shown;
                    LineStatus::Retypeset { barcode_match }
                }
                Err(why) => refuse(&why),
            }
        };
        lines.push(line);
    }
    Ok(lines)
}

/// Replace the ink of one line with typeset text, then re-read it; restore on mismatch.
/// Glyph pieces sitting well above the line's baseline and smaller than its letters:
/// superscripts such as ™, ®, ¹. Returns their boxes.
fn raised_parts(parts: &[Rect]) -> Vec<Rect> {
    if parts.len() < 3 {
        return Vec::new();
    }
    let mut bottoms: Vec<u32> = parts.iter().map(|p| p.1 + p.3).collect();
    let mut heights: Vec<u32> = parts.iter().map(|p| p.3).collect();
    bottoms.sort_unstable();
    heights.sort_unstable();
    let baseline = bottoms[bottoms.len() / 2] as f32;
    let mut tops: Vec<u32> = parts.iter().map(|p| p.1).collect();
    tops.sort_unstable();
    let cap_top = tops[tops.len() / 2] as f32;
    let h = heights[heights.len() / 2] as f32;
    parts
        .iter()
        .copied()
        .filter(|p| ((p.1 + p.3) as f32) < baseline - 0.3 * h && (p.3 as f32) < 0.75 * h && (p.3 as f32) > 0.35 * h && (p.1 as f32) < cap_top + 0.2 * h)
        .collect()
}

/// OCR reads ™ as "TM"; compare typeset text with a read-back on those terms.
fn as_read(text: &str) -> String {
    text.replace('™', "TM")
}

/// Decide what to typeset for a line whose OCR text is `text`: superscript "TM" becomes ™;
/// any other superscript makes the line too risky to redraw.
fn with_superscripts(text: &str, ink: Rect, parts: &[Rect]) -> Result<String, String> {
    let raised = raised_parts(parts);
    if raised.is_empty() {
        return Ok(text.to_string());
    }
    let at_end = raised.iter().all(|r| r.0 as f32 >= ink.0 as f32 + ink.2 as f32 * 0.8);
    match text.strip_suffix("TM") {
        Some(rest) if at_end && raised.len() >= 2 => Ok(format!("{}™", rest.trim_end())),
        // Hyphens, apostrophes and the like also sit above the baseline. A real superscript
        // OCR flattened (®, ¹) is caught later: the redrawn line won't have it raised.
        _ => Ok(text.to_string()),
    }
}

fn draw_line(e: &Engine, text: &str, core: Rect, b: Rect, bitmap: &mut GrayImage) -> Result<String, String> {
    let (ink, parts) = line_ink(bitmap, core, b).ok_or("no ink")?;
    let shown = with_superscripts(text, ink, &parts)?;
    let text = shown.as_str();
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
    if check.as_deref() == Some(as_read(text).as_str()) && raised_parts(&line_ink(bitmap, core, b).map(|x| x.1).unwrap_or_default()).len() == raised_parts(&parts).len() {
        Ok(shown.clone())
    } else if check.as_deref() == Some(as_read(text).as_str()) {
        if std::env::var_os("SHIPPINGLABELCRAFT_DEBUG").is_some() {
            eprintln!("{text}: before {:?}\n  after {:?}\n  parts {:?}", raised_parts(&parts), raised_parts(&line_ink(bitmap, core, b).map(|x| x.1).unwrap_or_default()), parts);
        }
        imageops::replace(bitmap, &backup, b.0 as i64, b.1 as i64);
        Err("has superscript or raised characters".into())
    } else {
        imageops::replace(bitmap, &backup, b.0 as i64, b.1 as i64);
        Err("re-typeset text didn't read back identically".into())
    }
}
