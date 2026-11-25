//! Image pipeline: load → crop → orient → fit to label → threshold → barcode regeneration.

use anyhow::{Context, Result, anyhow};
use image::{GrayImage, Luma, imageops};
use std::path::Path;

use crate::barcodes::{self, BarcodeReport};

/// Physical label + printer settings.
#[derive(Clone, Debug)]
pub struct Settings {
    pub dpi: f32,
    pub width_mm: f32,
    pub height_mm: f32,
    /// Blank border kept around the content, in mm (printer edge tolerance).
    pub margin_mm: f32,
    /// Replace decoded barcodes with freshly generated, verified ones.
    pub regenerate_barcodes: bool,
    /// Shifts the black/white cut-off. Positive = darker output.
    pub darkness: i32,
    /// Extra quarter-turns (clockwise) applied after auto-orientation.
    pub extra_rotation: u8,
    /// Manual crop in source pixel coordinates (x, y, w, h). None = auto.
    pub manual_crop: Option<(u32, u32, u32, u32)>,
    /// PDF page index (0-based).
    pub page: usize,
    /// Redraw PDFs straight onto the printer's dot grid (vs. resampling a raster).
    pub vector_render: bool,
    /// Find the label on the page automatically (else use the whole page).
    pub auto_crop: bool,
    /// Correct small tilts.
    pub straighten: bool,
    /// Turn the label so its main barcode reads left-to-right.
    pub auto_rotate: bool,
    /// Thicken thin strokes by one dot (helps faint thermal prints).
    pub bolder: bool,
    /// Round barcode bar widths up rather than to nearest (easier scanning, slightly wider).
    pub thicker_bars: bool,
    /// Nudge the printed image (mm, + = right/down) to correct printer alignment.
    pub offset_x_mm: f32,
    pub offset_y_mm: f32,
    /// Opt-in OCR re-typesetting of text lines.
    pub ocr: bool,
    pub ocr_settings: crate::ocr::OcrSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dpi: 203.0,
            width_mm: 100.0,
            height_mm: 150.0,
            margin_mm: 0.5,
            regenerate_barcodes: true,
            darkness: 0,
            extra_rotation: 0,
            manual_crop: None,
            page: 0,
            vector_render: true,
            auto_crop: true,
            straighten: true,
            auto_rotate: true,
            bolder: false,
            thicker_bars: false,
            offset_x_mm: 0.0,
            offset_y_mm: 0.0,
            ocr: false,
            ocr_settings: Default::default(),
        }
    }
}

impl Settings {
    pub fn target_size(&self) -> (u32, u32) {
        (
            (self.width_mm / 25.4 * self.dpi).round() as u32,
            (self.height_mm / 25.4 * self.dpi).round() as u32,
        )
    }
}

/// A loaded input document.
pub struct Source {
    pub name: String,
    pub kind: SourceKind,
    pub page_count: usize,
}

pub enum SourceKind {
    Pdf(std::sync::Arc<Vec<u8>>),
    Raster(GrayImage),
}

/// Rendered source page plus whether it is vector (clean) content.
pub struct SourcePage {
    pub gray: GrayImage,
    pub is_vector: bool,
    /// For PDFs: enough to re-render the page straight onto the printer's dot grid.
    pub pdf: Option<PdfPage>,
}

#[derive(Clone)]
pub struct PdfPage {
    pub data: std::sync::Arc<Vec<u8>>,
    pub index: usize,
    /// Pixels per PDF point used for `gray`.
    pub scale: f32,
}

/// 2D affine transform, kurbo coefficient order: x' = a·x + c·y + e, y' = b·x + d·y + f.
#[derive(Clone, Copy, Debug)]
pub struct Xf([f64; 6]);

impl Xf {
    pub const IDENTITY: Xf = Xf([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    pub fn translate(x: f64, y: f64) -> Xf {
        Xf([1.0, 0.0, 0.0, 1.0, x, y])
    }
    pub fn scale(sx: f64, sy: f64) -> Xf {
        Xf([sx, 0.0, 0.0, sy, 0.0, 0.0])
    }
    /// `self` first, then `next`.
    pub fn then(self, next: Xf) -> Xf {
        let [a1, b1, c1, d1, e1, f1] = self.0;
        let [a2, b2, c2, d2, e2, f2] = next.0;
        Xf([
            a2 * a1 + c2 * b1,
            b2 * a1 + d2 * b1,
            a2 * c1 + c2 * d1,
            b2 * c1 + d2 * d1,
            a2 * e1 + c2 * f1 + e2,
            b2 * e1 + d2 * f1 + f2,
        ])
    }
    /// The pixel mapping of `rotate_quarter(img, q)` for an image of size w×h.
    fn quarter(q: u8, w: u32, h: u32) -> Xf {
        let (w, h) = (w as f64, h as f64);
        match q % 4 {
            1 => Xf([0.0, 1.0, -1.0, 0.0, h, 0.0]),
            2 => Xf([-1.0, 0.0, 0.0, -1.0, w, h]),
            3 => Xf([0.0, -1.0, 1.0, 0.0, 0.0, w]),
            _ => Xf::IDENTITY,
        }
    }
}

pub struct Output {
    /// Final 1-bit label (0 = black, 255 = white), exactly target_size().
    pub bitmap: GrayImage,
    /// The crop rectangle used, in source page pixels.
    pub crop: (u32, u32, u32, u32),
    pub barcodes: Vec<BarcodeReport>,
    pub notes: Vec<String>,
    /// OCR results (empty when OCR is off).
    pub ocr: Vec<crate::ocr::OcrLine>,
    /// The label before any text was re-typeset, for side-by-side review.
    pub before_ocr: Option<GrayImage>,
}

const PDF_RENDER_DPI: f32 = 300.0;

impl Source {
    pub fn open(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let is_pdf = bytes.starts_with(b"%PDF")
            || path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        if is_pdf {
            let data = std::sync::Arc::new(bytes);
            let pdf = hayro::hayro_syntax::Pdf::new(data.clone())
                .map_err(|e| anyhow!("could not open PDF: {e:?}"))?;
            let n = pdf.pages().len();
            if n == 0 {
                return Err(anyhow!("PDF has no pages"));
            }
            Ok(Self { name, kind: SourceKind::Pdf(data), page_count: n })
        } else {
            let img = load_raster(&bytes)?;
            Ok(Self { name, kind: SourceKind::Raster(img), page_count: 1 })
        }
    }

    pub fn render_page(&self, page: usize) -> Result<SourcePage> {
        match &self.kind {
            SourceKind::Raster(g) => Ok(SourcePage { gray: g.clone(), is_vector: false, pdf: None }),
            SourceKind::Pdf(data) => {
                let pdf = hayro::hayro_syntax::Pdf::new(data.clone())
                    .map_err(|e| anyhow!("could not open PDF: {e:?}"))?;
                let pages = pdf.pages();
                let page_index = page.min(pages.len() - 1);
                let page = pages
                    .get(page_index)
                    .ok_or_else(|| anyhow!("page out of range"))?;
                let (w, h) = page.render_dimensions();
                // Keep the render under ~6000 px on the long side.
                let mut scale = PDF_RENDER_DPI / 72.0;
                let long = w.max(h) * scale;
                if long > 6000.0 {
                    scale *= 6000.0 / long;
                }
                let cache = hayro::RenderCache::new();
                let pix = hayro::render(
                    page,
                    &cache,
                    &hayro::hayro_interpret::InterpreterSettings::default(),
                    &hayro::RenderSettings::default(),
                    &hayro::PixmapSettings {
                        x_scale: scale,
                        y_scale: scale,
                        bg_color: hayro::vello_cpu::color::palette::css::WHITE,
                    },
                );
                let (pw, ph) = (pix.width() as u32, pix.height() as u32);
                let mut gray = GrayImage::new(pw, ph);
                for (i, px) in pix.data().iter().enumerate() {
                    // Premultiplied over white background → composite onto white.
                    let a = px.a as f32 / 255.0;
                    let r = px.r as f32 + 255.0 * (1.0 - a);
                    let g = px.g as f32 + 255.0 * (1.0 - a);
                    let b = px.b as f32 + 255.0 * (1.0 - a);
                    let l = (0.299 * r + 0.587 * g + 0.114 * b).clamp(0.0, 255.0) as u8;
                    gray.as_mut()[i] = l;
                }
                // A PDF that is just one embedded scan is not "vector" in any useful sense,
                // but its render is still at least as good as the embedded image.
                Ok(SourcePage { gray, is_vector: true, pdf: Some(PdfPage { data: data.clone(), index: page_index, scale }) })
            }
        }
    }
}

fn load_raster(bytes: &[u8]) -> Result<GrayImage> {
    use image::ImageDecoder;
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .context("unrecognised image format")?;
    let mut decoder = reader.into_decoder().context("decoding image")?;
    let orientation = decoder.orientation().ok();
    let mut img = image::DynamicImage::from_decoder(decoder).context("decoding image")?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    // Flatten transparency onto white (screenshots of labels are often RGBA).
    let rgba = img.to_rgba8();
    let mut gray = GrayImage::new(rgba.width(), rgba.height());
    for (d, s) in gray.pixels_mut().zip(rgba.pixels()) {
        let a = s[3] as f32 / 255.0;
        let l = 0.299 * s[0] as f32 + 0.587 * s[1] as f32 + 0.114 * s[2] as f32;
        d[0] = (l * a + 255.0 * (1.0 - a)).clamp(0.0, 255.0) as u8;
    }
    Ok(gray)
}

/// Run the whole pipeline on one rendered page.
pub fn process(page: &SourcePage, s: &Settings) -> Result<Output> {
    let mut notes = Vec::new();
    let src = &page.gray;
    let (tw, th) = s.target_size();

    // 1. Crop to the label.
    let crop = match s.manual_crop {
        Some(c) => clamp_rect(c, src.width(), src.height()),
        None if !s.auto_crop => (0, 0, src.width(), src.height()),
        None => {
            let (c, how) = auto_crop(src, page.is_vector);
            notes.push(how);
            c
        }
    };
    let mut img = imageops::crop_imm(src, crop.0, crop.1, crop.2, crop.3).to_image();
    // Source-page pixels → current image pixels; extended with every geometric step so a PDF
    // can later be redrawn directly at the printer's resolution.
    let mut m = Xf::translate(-(crop.0 as f64), -(crop.1 as f64));

    // 2. Straighten small skews (scans, photos, screenshots of rotated PDFs).
    let skew = if s.straighten { estimate_skew(&img) } else { 0.0 };
    if skew.abs() > 0.15 {
        let (r, x) = rotate_fine_xf(&img, skew.to_radians());
        img = r;
        m = m.then(x);
        notes.push(format!("Straightened by {skew:.1}°"));
    }

    if std::env::var_os("SHIPPINGLABELCRAFT_DEBUG").is_some() {
        let _ = img.save("debug_deskewed.png");
    }
    // 3. Orientation from the main 1D barcode's reading direction. Try the image as-is,
    //    then an upscaled copy (helps the decoder with low-res sources).
    let mut angle = if s.auto_rotate { barcodes::reading_angle(&img) } else { None };
    let long = img.width().max(img.height());
    if s.auto_rotate && angle.is_none() && long < 2400 {
        let f = 2400.0 / long as f32;
        let up = imageops::resize(
            &img,
            (img.width() as f32 * f) as u32,
            (img.height() as f32 * f) as u32,
            imageops::FilterType::Lanczos3,
        );
        angle = barcodes::reading_angle(&up);
    }
    match angle {
        Some(angle) => {
            let deg = angle.to_degrees();
            let quarter = ((-deg / 90.0).round() as i32).rem_euclid(4) as u8;
            m = m.then(Xf::quarter(quarter, img.width(), img.height()));
            img = rotate_quarter(&img, quarter);
            let residual = -deg - quarter as f32 * 90.0;
            let residual = ((residual + 180.0).rem_euclid(360.0)) - 180.0;
            if residual.abs() > 0.15 {
                let (r, x) = rotate_fine_xf(&img, residual.to_radians());
                img = r;
                m = m.then(x);
            }
            if quarter != 0 {
                notes.push(format!("Rotated {}° so the barcode reads left-to-right", quarter as u32 * 90));
            }
        }
        None if !s.auto_rotate => notes.push("Automatic rotation is off".into()),
        None => {
            if img.width() > img.height() {
                m = m.then(Xf::quarter(1, img.width(), img.height()));
                img = rotate_quarter(&img, 1);
                notes.push(
                    "No 1D barcode to orient by: turned landscape → portrait (check it isn't upside down)".into(),
                );
            } else {
                notes.push("No 1D barcode to orient by: orientation kept as-is".into());
            }
        }
    }
    if !s.extra_rotation.is_multiple_of(4) {
        m = m.then(Xf::quarter(s.extra_rotation % 4, img.width(), img.height()));
        img = rotate_quarter(&img, s.extra_rotation % 4);
    }

    // 4. Trim to content and fit onto the label at 2× working resolution.
    let ot = otsu(&img);
    let content = content_bbox(&img, ot.saturating_sub(10).max(40), 2).unwrap_or((0, 0, img.width(), img.height()));
    // Keep only about one output dot of slack beyond the ink so anti-aliased edges survive;
    // anything more is wasted print area.
    let pad = (img.width() as f32 / tw as f32).max(img.height() as f32 / th as f32).ceil() as u32 + 1;
    let content = clamp_rect(
        (
            content.0.saturating_sub(pad),
            content.1.saturating_sub(pad),
            content.2 + 2 * pad,
            content.3 + 2 * pad,
        ),
        img.width(),
        img.height(),
    );
    let img = imageops::crop_imm(&img, content.0, content.1, content.2, content.3).to_image();
    m = m.then(Xf::translate(-(content.0 as f64), -(content.1 as f64)));

    let margin = (s.margin_mm / 25.4 * s.dpi).round() as u32;
    let (aw, ah) = (tw.saturating_sub(2 * margin).max(1), th.saturating_sub(2 * margin).max(1));
    let scale = (aw as f32 / img.width() as f32).min(ah as f32 / img.height() as f32);
    let (fw, fh) = (
        ((img.width() as f32 * scale).round() as u32).max(1),
        ((img.height() as f32 * scale).round() as u32).max(1),
    );
    if fw < tw / 2 && fh < th / 2 {
        notes.push("Content is much smaller than the label; check the crop".into());
    }
    let ox = (tw - fw) / 2;
    let oy = (th - fh) / 2;

    m = m
        .then(Xf::scale(fw as f64 / img.width() as f64, fh as f64 / img.height() as f64))
        .then(Xf::translate(ox as f64, oy as f64));

    let work_scale = 2u32;
    let (work, gray_t) = match page.pdf.as_ref().filter(|_| s.vector_render) {
        // PDFs: draw the vector content straight onto the dot grid (and a 2× copy for the
        // barcode reader) instead of resampling a raster. Text edges land exactly on dots.
        Some(pdf) => {
            let full = render_pdf_with(pdf, m, tw, th);
            let work = render_pdf_with(pdf, m.then(Xf::scale(work_scale as f64, work_scale as f64)), tw * work_scale, th * work_scale);
            match (full, work) {
                // The page goes on past the crop: blank whatever lies outside it, so nothing
                // half-cut (a stray code, the next label) sneaks onto the edge.
                (Some(mut f), Some(mut w)) => {
                    blank_outside(&mut f, (ox, oy, fw, fh));
                    blank_outside(&mut w, (ox * work_scale, oy * work_scale, fw * work_scale, fh * work_scale));
                    (w, f)
                }
                _ => resampled(&img, fw, fh, ox, oy, tw, th, work_scale),
            }
        }
        None => resampled(&img, fw, fh, ox, oy, tw, th, work_scale),
    };

    // 5. Threshold to the printer grid.
    let mut bitmap = threshold(&gray_t, page.is_vector, s.darkness);
    if s.bolder {
        bitmap = embolden(&bitmap);
    }

    // 6. Barcodes.
    let mut reports = if s.regenerate_barcodes {
        barcodes::regenerate_all(&work, work_scale, &mut bitmap, s.thicker_bars)
    } else {
        barcodes::scan_only(&bitmap)
    };

    // 7. Optional OCR re-typesetting.
    let (mut ocr, mut before_ocr) = (Vec::new(), None);
    if s.ocr {
        let before = bitmap.clone();
        match crate::ocr::retypeset(&work, work_scale, &gray_t, &mut bitmap, &reports, &s.ocr_settings) {
            Ok(lines) => {
                let n = lines.iter().filter(|l| matches!(l.status, crate::ocr::LineStatus::Retypeset { .. })).count();
                if n > 0 {
                    before_ocr = Some(before);
                }
                ocr = lines;
            }
            Err(e) => notes.push(format!("OCR unavailable: {e}")),
        }
    }

    // 8. Alignment nudge.
    let (dx, dy) = ((s.offset_x_mm / 25.4 * s.dpi).round() as i64, (s.offset_y_mm / 25.4 * s.dpi).round() as i64);
    if dx != 0 || dy != 0 {
        bitmap = shift(&bitmap, dx, dy);
        before_ocr = before_ocr.map(|b| shift(&b, dx, dy));
        let mv = |r: (u32, u32, u32, u32)| ((r.0 as i64 + dx).max(0) as u32, (r.1 as i64 + dy).max(0) as u32, r.2, r.3);
        for l in ocr.iter_mut() {
            l.region = mv(l.region);
        }
        for b in reports.iter_mut() {
            b.region = mv(b.region);
        }
    }

    Ok(Output { bitmap, crop, barcodes: reports, notes, ocr, before_ocr })
}

#[allow(clippy::too_many_arguments)]
fn resampled(img: &GrayImage, fw: u32, fh: u32, ox: u32, oy: u32, tw: u32, th: u32, ws: u32) -> (GrayImage, GrayImage) {
    let fitted = imageops::resize(img, fw * ws, fh * ws, imageops::FilterType::Lanczos3);
    let mut work = GrayImage::from_pixel(tw * ws, th * ws, Luma([255]));
    imageops::overlay(&mut work, &fitted, (ox * ws) as i64, (oy * ws) as i64);
    let gray = downsample2(&work);
    (work, gray)
}

/// Render a PDF page with `m` (source-render pixels → output pixels) into a w×h gray image.
fn render_pdf_with(pdf: &PdfPage, m: Xf, w: u32, h: u32) -> Option<GrayImage> {
    use hayro::hayro_interpret::util::TransformExt;
    use hayro::kurbo::Affine;
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        return None;
    }
    let doc = hayro::hayro_syntax::Pdf::new(pdf.data.clone()).ok()?;
    let pages = doc.pages();
    let page = pages.get(pdf.index)?;
    let to_src = Affine::scale(pdf.scale as f64) * page.initial_transform(true).to_kurbo();
    let full = Affine::new(m.0) * to_src;
    let cache = hayro::RenderCache::new();
    let mut ctx = hayro::vello_cpu::RenderContext::new(w as u16, h as u16);
    hayro::render_into(
        page,
        &cache,
        &hayro::hayro_interpret::InterpreterSettings::default(),
        // Bilinear for embedded images: many carrier PDFs wrap a 1-bit scan of the label, and
        // nearest-neighbour at a non-integer scale is what makes small text look ragged.
        &hayro::RenderSettings {
            force_image_interpolation: std::env::var_os("SHIPPINGLABELCRAFT_NEAREST").is_none(),
            ..Default::default()
        },
        &mut ctx,
        full,
    );
    ctx.flush();
    let mut pix = hayro::vello_cpu::Pixmap::new(w as u16, h as u16);
    ctx.render_with(
        &mut pix,
        &mut hayro::vello_cpu::Resources::default(),
        hayro::vello_cpu::RasterizerSettings {
            target_init: hayro::vello_cpu::TargetInit::Clear(hayro::vello_cpu::color::palette::css::WHITE),
            ..Default::default()
        },
    );
    let mut gray = GrayImage::new(w, h);
    for (i, px) in pix.data().iter().enumerate() {
        let a = px.a as f32 / 255.0;
        let l = 0.299 * px.r as f32 + 0.587 * px.g as f32 + 0.114 * px.b as f32 + 255.0 * (1.0 - a);
        gray.as_mut()[i] = l.clamp(0.0, 255.0) as u8;
    }
    Some(gray)
}

/// Thicken dark strokes by one dot (to the right and down), keeping counters open.
fn embolden(b: &GrayImage) -> GrayImage {
    let mut out = b.clone();
    for (x, y, p) in b.enumerate_pixels() {
        if p[0] < 128 {
            if x + 1 < b.width() {
                out.put_pixel(x + 1, y, Luma([0]));
            }
            if y + 1 < b.height() && x % 2 == 0 {
                out.put_pixel(x, y + 1, Luma([0]));
            }
        }
    }
    out
}

pub(crate) fn shift(b: &GrayImage, dx: i64, dy: i64) -> GrayImage {
    let mut out = GrayImage::from_pixel(b.width(), b.height(), Luma([255]));
    imageops::overlay(&mut out, b, dx, dy);
    out
}

fn clamp_rect(r: (u32, u32, u32, u32), w: u32, h: u32) -> (u32, u32, u32, u32) {
    let x = r.0.min(w.saturating_sub(1));
    let y = r.1.min(h.saturating_sub(1));
    let rw = r.2.min(w - x).max(1);
    let rh = r.3.min(h - y).max(1);
    (x, y, rw, rh)
}

/// Otsu's global threshold.
pub fn otsu(img: &GrayImage) -> u8 {
    let mut hist = [0u64; 256];
    for p in img.as_raw() {
        hist[*p as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    let sum: f64 = hist.iter().enumerate().map(|(i, &c)| i as f64 * c as f64).sum();
    let (mut sum_b, mut w_b, mut best, mut best_t) = (0.0f64, 0u64, 0.0f64, 128u8);
    for (t, &count) in hist.iter().enumerate() {
        w_b += count;
        if w_b == 0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0 {
            break;
        }
        sum_b += t as f64 * count as f64;
        let m_b = sum_b / w_b as f64;
        let m_f = (sum - sum_b) / w_f as f64;
        let between = w_b as f64 * w_f as f64 * (m_b - m_f).powi(2);
        if between > best {
            best = between;
            best_t = t as u8;
        }
    }
    best_t
}

/// Bounding box of pixels darker than `t`, ignoring rows/cols with fewer than `min_count` hits.
fn content_bbox(img: &GrayImage, t: u8, min_count: u32) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = img.dimensions();
    let mut rows = vec![0u32; h as usize];
    let mut cols = vec![0u32; w as usize];
    for (x, y, p) in img.enumerate_pixels() {
        if p[0] < t {
            rows[y as usize] += 1;
            cols[x as usize] += 1;
        }
    }
    let y0 = rows.iter().position(|&c| c >= min_count)? as u32;
    let y1 = rows.iter().rposition(|&c| c >= min_count)? as u32;
    let x0 = cols.iter().position(|&c| c >= min_count)? as u32;
    let x1 = cols.iter().rposition(|&c| c >= min_count)? as u32;
    Some((x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// Find the label on the page. Returns the rect (source pixels) and a note on how.
/// What's on one page: every shipping label found, plus a packing slip if there's a big
/// block of other content left over.
pub struct PageLayout {
    pub labels: Vec<(u32, u32, u32, u32)>,
    pub slip: Option<(u32, u32, u32, u32)>,
}

/// Find every label on a page (two-up sheets, a label beside its packing slip…).
pub fn find_labels(src: &GrayImage, is_vector: bool) -> PageLayout {
    let (w, h) = src.dimensions();
    let full = (0, 0, w, h);
    let (first, how) = auto_crop(src, is_vector);
    // A label photographed or screenshotted on a dark background is one label.
    if first == full || how.contains("dark surroundings") {
        return PageLayout { labels: vec![first], slip: None };
    }
    let mut labels = vec![first];
    let mut masked = src.clone();
    let blank = |img: &mut GrayImage, r: (u32, u32, u32, u32)| {
        let pad = (w.max(h) / 100).max(2);
        let (x0, y0) = (r.0.saturating_sub(pad), r.1.saturating_sub(pad));
        let (x1, y1) = ((r.0 + r.2 + pad).min(w), (r.1 + r.3 + pad).min(h));
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, Luma([255]));
            }
        }
    };
    blank(&mut masked, first);
    let area = |r: (u32, u32, u32, u32)| r.2 as f32 * r.3 as f32;
    let label_like = |r: (u32, u32, u32, u32)| {
        let asp = r.2.max(r.3) as f32 / r.2.min(r.3).max(1) as f32;
        (1.0..=2.2).contains(&asp)
    };
    let mut slip = None;
    for _ in 0..5 {
        let (r, _) = auto_crop(&masked, is_vector);
        if r == full || area(r) < area(first) * 0.15 {
            break;
        }
        let ratio = area(r) / area(first);
        if label_like(r) && (0.6..=1.6).contains(&ratio) {
            labels.push(r);
            blank(&mut masked, r);
            continue;
        }
        // A sizeable leftover block of content: most likely a packing slip.
        if slip.is_none() && area(r) > area(full) * 0.04 {
            // Widen to all ink in that band (box outlines and rules get dropped by the
            // block finder), labels excluded since they're blanked out.
            let pad = h / 50;
            let (y0, y1) = (r.1.saturating_sub(pad), (r.1 + r.3 + pad).min(h));
            let (mut bx0, mut by0, mut bx1, mut by1) = (u32::MAX, u32::MAX, 0, 0);
            for y in y0..y1 {
                for x in 0..w {
                    if masked.get_pixel(x, y)[0] < 128 {
                        bx0 = bx0.min(x);
                        by0 = by0.min(y);
                        bx1 = bx1.max(x);
                        by1 = by1.max(y);
                    }
                }
            }
            slip = Some(if bx1 > bx0 { (bx0, by0, bx1 - bx0 + 1, by1 - by0 + 1) } else { r });
        }
        break;
    }
    // Reading order: top to bottom, then left to right.
    labels.sort_by_key(|r| (r.1 / (h / 8).max(1), r.0));
    PageLayout { labels, slip }
}

pub fn auto_crop(src: &GrayImage, is_vector: bool) -> ((u32, u32, u32, u32), String) {
    let (w, h) = src.dimensions();
    let aspect = w.max(h) as f32 / w.min(h) as f32;
    let full = (0, 0, w, h);

    // Work on a downscaled copy for speed.
    let f = (1000.0 / w.max(h) as f32).min(1.0);
    let small = if f < 1.0 {
        imageops::resize(src, ((w as f32 * f) as u32).max(1), ((h as f32 * f) as u32).max(1), imageops::FilterType::Triangle)
    } else {
        src.clone()
    };
    let (sw, sh) = small.dimensions();
    let t = otsu(&small).clamp(100, 200);
    let dark: Vec<bool> = small.as_raw().iter().map(|&p| p < t).collect();
    let up = |r: (u32, u32, u32, u32)| {
        clamp_rect(
            (
                (r.0 as f32 / f) as u32,
                (r.1 as f32 / f) as u32,
                (r.2 as f32 / f).ceil() as u32 + 2,
                (r.3 as f32 / f).ceil() as u32 + 2,
            ),
            w,
            h,
        )
    };


    // (b) Label on a dark background (screenshot of a viewer, photo on a desk): take the
    //     largest light region.
    let border: Vec<bool> = (0..sw)
        .flat_map(|x| [dark[x as usize], dark[((sh - 1) * sw + x) as usize]])
        .chain((0..sh).flat_map(|y| [dark[(y * sw) as usize], dark[(y * sw + sw - 1) as usize]]))
        .collect();
    let border_dark = border.iter().filter(|&&d| d).count() as f32 / border.len() as f32;
    if border_dark > 0.5 {
        let light: Vec<bool> = dark.iter().map(|d| !d).collect();
        if let Some(c) = components(&light, sw, sh)
            .into_iter()
            .filter(|c| ((c.x1 - c.x0 + 1) * (c.y1 - c.y0 + 1)) as f32 > area_of(sw, sh) * 0.08)
            .max_by_key(|c| (c.x1 - c.x0) * (c.y1 - c.y0))
        {
            // Inset a little so the paper's own edge doesn't print as a frame.
            let (iw, ih) = ((c.x1 - c.x0 + 1) / 60 + 1, (c.y1 - c.y0 + 1) / 60 + 1);
            let r = up((c.x0 + iw, c.y0 + ih, (c.x1 - c.x0 + 1).saturating_sub(2 * iw), (c.y1 - c.y0 + 1).saturating_sub(2 * ih)));
            return (r, "Cropped the label out of its dark surroundings".into());
        }
    }

    // (a) A 4×6-shaped page/image is the label itself.
    if (1.38..=1.62).contains(&aspect) {
        let _ = is_vector;
        return (full, "Already label-shaped: using the whole page".into());
    }

    // (c) Look for a printed frame around the label: a large, thin, ~2:3 component.
    let comps = components(&dark, sw, sh);
    let area = (sw * sh) as f32;
    let frame = comps
        .iter()
        .filter(|c| {
            let (bw, bh) = (c.x1 - c.x0 + 1, c.y1 - c.y0 + 1);
            let a = (bw * bh) as f32;
            let asp = bw.max(bh) as f32 / bw.min(bh) as f32;
            a > area * 0.12 && (1.3..=1.7).contains(&asp) && (c.count as f32) < a * 0.35
        })
        .max_by_key(|c| (c.x1 - c.x0) * (c.y1 - c.y0));
    if let Some(c) = frame {
        // Some carriers print a strip just outside the border (eBay: a restriction notice
        // and a 2D code under it). Pull in ink sitting right against the frame.
        let (mut x0, mut y0, mut x1, mut y1) = (c.x0, c.y0, c.x1, c.y1);
        let reach = ((x1 - x0).max(y1 - y0) as f32 * 0.03).ceil() as u32;
        let frame_area = ((x1 - x0 + 1) * (y1 - y0 + 1)) as f32;
        let cap = ((x1 - x0).max(y1 - y0) as f32 * 0.15) as u32;
        let mut grew = false;
        loop {
            let mut changed = false;
            for k in &comps {
                let inside = k.x0 >= x0 && k.y0 >= y0 && k.x1 <= x1 && k.y1 <= y1;
                // Touching or within `reach` of the box (a 2D code poking out counts whole).
                let near = k.x0 <= x1 + reach && k.x1 + reach >= x0 && k.y0 <= y1 + reach && k.y1 + reach >= y0;
                let small = (((k.x1 - k.x0 + 1) * (k.y1 - k.y0 + 1)) as f32) < frame_area * 0.1;
                // Never grow more than ~15% past the printed border on any side.
                let bounded = k.x0 + cap >= c.x0 && k.x1 <= c.x1 + cap && k.y0 + cap >= c.y0 && k.y1 <= c.y1 + cap;
                if !inside && near && small && bounded {
                    x0 = x0.min(k.x0);
                    y0 = y0.min(k.y0);
                    x1 = x1.max(k.x1);
                    y1 = y1.max(k.y1);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
            grew = true;
        }
        let r = up((x0, y0, x1 - x0 + 1, y1 - y0 + 1));
        if r.2 * r.3 < w * h * 95 / 100 {
            let note = if grew { "Found the label's printed border (plus the strip printed beside it)" } else { "Found the label's printed border" };
            return (r, note.into());
        }
    }

    // (d) Group ink into blocks (gaps under ~2.5% of the page merge) and take the
    //     biggest one, if it's plausibly label-shaped (label vs. instructions text).
    // Full-width/height rules (fold lines, dashed cut lines) would glue everything
    // together, so drop thin isolated rows/columns that span most of the page first.
    let mut ink = dark.clone();
    let (swi, shi) = (sw as usize, sh as usize);
    let row_count = |m: &[bool], y: usize| m[y * swi..(y + 1) * swi].iter().filter(|&&d| d).count();
    let row_extent = |m: &[bool], y: usize| {
        let r = &m[y * swi..(y + 1) * swi];
        match (r.iter().position(|&d| d), r.iter().rposition(|&d| d)) {
            (Some(a), Some(b)) => b - a,
            _ => 0,
        }
    };
    let rows: Vec<usize> = (3..shi.saturating_sub(3))
        .filter(|&y| {
            let c = row_count(&dark, y);
            row_extent(&dark, y) > swi * 6 / 10
                && row_count(&dark, y - 3) * 10 < c.max(1)
                && row_count(&dark, y + 3) * 10 < c.max(1)
        })
        .collect();
    for y in rows {
        for dy in 0..=2usize {
            for yy in [y - dy, y + dy] {
                ink[yy * swi..(yy + 1) * swi].iter_mut().for_each(|d| *d = false);
            }
        }
    }
    let col = |m: &[bool], x: usize| (0..shi).filter(|&y| m[y * swi + x]).count();
    let col_extent = |m: &[bool], x: usize| {
        let ys: Vec<usize> = (0..shi).filter(|&y| m[y * swi + x]).collect();
        ys.last().map_or(0, |l| l - ys[0])
    };
    let cols: Vec<usize> = (3..swi.saturating_sub(3))
        .filter(|&x| {
            let c = col(&dark, x);
            col_extent(&dark, x) > shi * 6 / 10 && col(&dark, x - 3) * 10 < c.max(1) && col(&dark, x + 3) * 10 < c.max(1)
        })
        .collect();
    for x in cols {
        for dx in 0..=2usize {
            for xx in [x - dx, x + dx] {
                for y in 0..shi {
                    ink[y * swi + xx] = false;
                }
            }
        }
    }
    let rad = ((sw.max(sh) as f32) * 0.03).ceil() as i32;
    let grown = dilate(&ink, sw, sh, rad);
    let comps = components(&grown, sw, sh);
    if let Some(c) = comps.iter().max_by_key(|c| (c.x1 - c.x0) * (c.y1 - c.y0)) {
        // Undo the dilation on the bbox.
        let r0 = (
            (c.x0 as i32 + rad).max(0) as u32,
            (c.y0 as i32 + rad).max(0) as u32,
            ((c.x1 - c.x0 + 1) as i32 - 2 * rad).max(1) as u32,
            ((c.y1 - c.y0 + 1) as i32 - 2 * rad).max(1) as u32,
        );
        let asp = r0.2.max(r0.3) as f32 / r0.2.min(r0.3) as f32;
        let r = up(r0);
        let covers = (r.2 as f32 * r.3 as f32) / (w as f32 * h as f32);
        if std::env::var_os("SHIPPINGLABELCRAFT_DEBUG").is_some() {
            eprintln!("blob crop {r:?} asp {asp:.2} covers {covers:.2} comps {}", comps.len());
        }
        if covers < 0.9 && (1.0..=2.2).contains(&asp) {
            return (r, "Cropped to the largest block of content (adjust if needed)".into());
        }
    }
    (full, "Using the whole image".into())
}

fn blank_outside(img: &mut GrayImage, (x, y, w, h): (u32, u32, u32, u32)) {
    for (px, py, p) in img.enumerate_pixels_mut() {
        if px < x || py < y || px >= x + w || py >= y + h {
            p[0] = 255;
        }
    }
}

fn area_of(w: u32, h: u32) -> f32 {
    (w * h) as f32
}

pub(crate) struct Comp {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    pub count: u32,
}

/// 8-connected components via iterative flood fill.
pub(crate) fn components(mask: &[bool], w: u32, h: u32) -> Vec<Comp> {
    let mut seen = vec![false; mask.len()];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut c = Comp { x0: u32::MAX, y0: u32::MAX, x1: 0, y1: 0, count: 0 };
        while let Some(i) = stack.pop() {
            let (x, y) = ((i as u32) % w, (i as u32) / w);
            c.x0 = c.x0.min(x);
            c.y0 = c.y0.min(y);
            c.x1 = c.x1.max(x);
            c.y1 = c.y1.max(y);
            c.count += 1;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = (ny as u32 * w + nx as u32) as usize;
                    if mask[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if c.count > 3 {
            out.push(c);
        }
    }
    out
}

/// Square dilation (separable, via running counts).
pub(crate) fn dilate(mask: &[bool], w: u32, h: u32, r: i32) -> Vec<bool> {
    let (w, h, r) = (w as usize, h as usize, r.max(0) as usize);
    let pass = |src: &[bool], len: usize, lines: usize, idx: &dyn Fn(usize, usize) -> usize| {
        let mut out = vec![false; src.len()];
        for l in 0..lines {
            let mut pre = vec![0u32; len + 1];
            for i in 0..len {
                pre[i + 1] = pre[i] + src[idx(l, i)] as u32;
            }
            for i in 0..len {
                let a = i.saturating_sub(r);
                let b = (i + r + 1).min(len);
                if pre[b] > pre[a] {
                    out[idx(l, i)] = true;
                }
            }
        }
        out
    };
    let horiz = pass(mask, w, h, &|y, x| y * w + x);
    pass(&horiz, h, w, &|x, y| y * w + x)
}

/// Rotate by `q` quarter turns clockwise (lossless).
pub fn rotate_quarter(img: &GrayImage, q: u8) -> GrayImage {
    match q % 4 {
        1 => imageops::rotate90(img),
        2 => imageops::rotate180(img),
        3 => imageops::rotate270(img),
        _ => img.clone(),
    }
}

/// [`rotate_fine`] plus the pixel mapping it applied.
pub fn rotate_fine_xf(img: &GrayImage, angle: f32) -> (GrayImage, Xf) {
    let out = rotate_fine(img, angle);
    let (s, c) = (angle as f64).sin_cos();
    let (cx, cy) = (img.width() as f64 / 2.0, img.height() as f64 / 2.0);
    let (ncx, ncy) = (out.width() as f64 / 2.0, out.height() as f64 / 2.0);
    let x = Xf([c, s, -s, c, -c * cx + s * cy + ncx, -s * cx - c * cy + ncy]);
    (out, x)
}

/// Rotate by an arbitrary angle (radians, clockwise-positive in image coords), expanding the canvas.
pub fn rotate_fine(img: &GrayImage, angle: f32) -> GrayImage {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (s, c) = angle.sin_cos();
    let nw = (w * c.abs() + h * s.abs()).ceil() as u32;
    let nh = (w * s.abs() + h * c.abs()).ceil() as u32;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (ncx, ncy) = (nw as f32 / 2.0, nh as f32 / 2.0);
    let mut out = GrayImage::from_pixel(nw, nh, Luma([255]));
    for y in 0..nh {
        for x in 0..nw {
            // Inverse map destination → source.
            let dx = x as f32 + 0.5 - ncx;
            let dy = y as f32 + 0.5 - ncy;
            let sx = c * dx + s * dy + cx - 0.5;
            let sy = -s * dx + c * dy + cy - 0.5;
            if sx < -1.0 || sy < -1.0 || sx > w || sy > h {
                continue;
            }
            out.put_pixel(x, y, Luma([bilinear(img, sx, sy)]));
        }
    }
    out
}

fn bilinear(img: &GrayImage, x: f32, y: f32) -> u8 {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let get = |xx: i32, yy: i32| -> f32 {
        if xx < 0 || yy < 0 || xx >= w || yy >= h {
            255.0
        } else {
            img.get_pixel(xx as u32, yy as u32)[0] as f32
        }
    };
    let v = get(x0, y0) * (1.0 - fx) * (1.0 - fy)
        + get(x0 + 1, y0) * fx * (1.0 - fy)
        + get(x0, y0 + 1) * (1.0 - fx) * fy
        + get(x0 + 1, y0 + 1) * fx * fy;
    v.round().clamp(0.0, 255.0) as u8
}

fn downsample2(img: &GrayImage) -> GrayImage {
    let (w, h) = (img.width() / 2, img.height() / 2);
    let mut out = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let s = img.get_pixel(2 * x, 2 * y)[0] as u32
                + img.get_pixel(2 * x + 1, 2 * y)[0] as u32
                + img.get_pixel(2 * x, 2 * y + 1)[0] as u32
                + img.get_pixel(2 * x + 1, 2 * y + 1)[0] as u32;
            out.put_pixel(x, y, Luma([(s / 4) as u8]));
        }
    }
    out
}

/// Black/white conversion. Vector sources get a fixed cut-off (they are already clean);
/// rasters get global Otsu combined with a local-contrast test so faint text survives
/// without hollowing out solid black areas.
pub fn threshold(img: &GrayImage, is_vector: bool, darkness: i32) -> GrayImage {
    let (w, h) = img.dimensions();
    let mut out = GrayImage::from_pixel(w, h, Luma([255]));
    if is_vector {
        let t = (150 + darkness * 2).clamp(20, 240) as u8;
        for (d, s) in out.pixels_mut().zip(img.pixels()) {
            if s[0] < t {
                d[0] = 0;
            }
        }
        return out;
    }
    let g = (otsu(img) as i32 + darkness * 2).clamp(20, 235);
    // Integral image for local mean.
    let (wi, hi) = (w as usize, h as usize);
    let mut integ = vec![0u64; (wi + 1) * (hi + 1)];
    for y in 0..hi {
        let mut row = 0u64;
        for x in 0..wi {
            row += img.as_raw()[y * wi + x] as u64;
            integ[(y + 1) * (wi + 1) + x + 1] = integ[y * (wi + 1) + x + 1] + row;
        }
    }
    let r = 12usize;
    for y in 0..hi {
        for x in 0..wi {
            let v = img.as_raw()[y * wi + x] as i32;
            let (x0, y0) = (x.saturating_sub(r), y.saturating_sub(r));
            let (x1, y1) = ((x + r + 1).min(wi), (y + r + 1).min(hi));
            let sum = integ[y1 * (wi + 1) + x1] + integ[y0 * (wi + 1) + x0]
                - integ[y0 * (wi + 1) + x1]
                - integ[y1 * (wi + 1) + x0];
            let mean = sum as f32 / ((x1 - x0) * (y1 - y0)) as f32;
            let local = mean * 0.82 + darkness as f32 * 2.0;
            let dark = v < g || (v as f32) < local && v < g + 45;
            if dark {
                out.as_mut()[y * wi + x] = 0;
            }
        }
    }
    out
}

/// Estimate the small rotation (degrees, clockwise-positive, |a| ≤ 12) that best aligns
/// ink into horizontal rows and vertical columns (projection-profile method).
pub fn estimate_skew(img: &GrayImage) -> f32 {
    let f = (800.0 / img.width().max(img.height()) as f32).min(1.0);
    let small = imageops::resize(
        img,
        ((img.width() as f32 * f) as u32).max(1),
        ((img.height() as f32 * f) as u32).max(1),
        imageops::FilterType::Triangle,
    );
    let t = otsu(&small);
    let pts: Vec<(f32, f32)> = small
        .enumerate_pixels()
        .filter(|(_, _, p)| p[0] < t)
        .map(|(x, y, _)| (x as f32, y as f32))
        .collect();
    if pts.len() < 200 {
        return 0.0;
    }
    let n = (small.width() + small.height()) as usize * 2 + 8;
    let score = |deg: f32| -> f64 {
        let (s, c) = deg.to_radians().sin_cos();
        let mut rows = vec![0u32; n];
        let mut cols = vec![0u32; n];
        let off = n as f32 / 2.0;
        for &(x, y) in &pts {
            let ry = (s * x + c * y + off * 0.5) as usize;
            let rx = (c * x - s * y + off * 0.5) as usize;
            if ry < n {
                rows[ry] += 1;
            }
            if rx < n {
                cols[rx] += 1;
            }
        }
        rows.iter().chain(cols.iter()).map(|&v| (v as f64).powi(2)).sum()
    };
    let mut best = (0.0f32, score(0.0));
    let mut a = -12.0f32;
    while a <= 12.0 {
        let sc = score(a);
        if sc > best.1 * 1.0005 {
            best = (a, sc);
        }
        a += 0.5;
    }
    let center = best.0;
    let mut a = center - 0.5;
    while a <= center + 0.5 {
        let sc = score(a);
        if sc > best.1 {
            best = (a, sc);
        }
        a += 0.05;
    }
    best.0
}
