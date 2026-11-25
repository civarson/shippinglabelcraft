//! The LabelCraft window: a dotted craft-mat canvas with the original and the finished
//! label as paper sheets, and an inspector of cards on the right.

use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, CornerRadius, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Vec2, pos2, vec2,
};
use image::GrayImage;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use crate::barcodes::Status;
use crate::ocr::{LineKind, LineStatus, OcrLine};
use crate::pipeline::{Output, Settings, Source, SourcePage};
use crate::theme::{self, R_CARD, R_CONTROL, R_PILL, Tokens};
use crate::widgets::{self, lerp};
use crate::printers::{self, RawLang};
use crate::{extras, pdfout, print};

const MOD: &str = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl " };

/// Preferences remembered between launches.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(default)]
struct Prefs {
    printer: String,
    dpi: f32,
    width_mm: f32,
    height_mm: f32,
    margin_mm: f32,
    darkness: i32,
    regenerate: bool,
    show_outlines: bool,
    cups_extra: String,
    /// 0 = light, 1 = follow system, 2 = dark
    theme: usize,
    light_theme: String,
    dark_theme: String,
    ui_scale: f32,
    reduce_motion: bool,
    offset_x_mm: f32,
    offset_y_mm: f32,
    vector_render: bool,
    auto_crop: bool,
    straighten: bool,
    auto_rotate: bool,
    bolder: bool,
    thicker_bars: bool,
    ocr: bool,
    ocr_confidence: f32,
    ocr_review: bool,
    copies: u32,
    auto_print: bool,
    archive: bool,
    archive_dir: String,
    /// Printer model profile: "auto" (from the printer's name), a profile id, or "custom".
    printer_profile: String,
    /// Open new label files that appear in a folder (Downloads by default).
    watch: bool,
    watch_dir: String,
    /// Offer packing slips found beside a label as an extra label to print.
    print_slips: bool,
    /// Check GitHub for a newer release at start-up.
    check_updates: bool,
    skipped_version: String,
    recent: Vec<Recent>,
    /// Delete archived prints after this many days (0 = keep them).
    archive_keep_days: u32,
    /// Direct printing in the printer's own language: "off", "auto", "zpl" or "tspl".
    raw_mode: String,
    /// Bumped when a default changes enough that saved values should be refreshed.
    rev: u32,
}

/// A recently opened label file.
#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
#[serde(default)]
struct Recent {
    path: String,
    name: String,
    /// Unix seconds.
    opened: u64,
}

/// One printable thing in the open file: a label (or a packing slip) on a page.
#[derive(Clone, PartialEq)]
struct Item {
    page: usize,
    crop: (u32, u32, u32, u32),
    slip: bool,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// 1: edge margin default 1.5 mm → 0.5 mm.
const PREFS_REV: u32 = 1;

impl Default for Prefs {
    fn default() -> Self {
        let s = Settings::default();
        Self {
            printer: String::new(),
            dpi: s.dpi,
            width_mm: s.width_mm,
            height_mm: s.height_mm,
            margin_mm: s.margin_mm,
            darkness: 0,
            regenerate: true,
            show_outlines: true,
            cups_extra: String::new(),
            theme: 1,
            light_theme: theme::PAPER.name.into(),
            dark_theme: theme::NIGHT.name.into(),
            ui_scale: 1.0,
            reduce_motion: false,
            offset_x_mm: 0.0,
            offset_y_mm: 0.0,
            vector_render: true,
            auto_crop: true,
            straighten: true,
            auto_rotate: true,
            bolder: false,
            thicker_bars: false,
            ocr: false,
            ocr_confidence: 0.9,
            ocr_review: true,
            copies: 1,
            auto_print: false,
            archive: false,
            archive_dir: String::new(),
            printer_profile: "auto".into(),
            watch: false,
            watch_dir: String::new(),
            print_slips: true,
            check_updates: true,
            skipped_version: String::new(),
            recent: Vec::new(),
            archive_keep_days: 0,
            raw_mode: "off".into(),
            rev: PREFS_REV,
        }
    }
}

enum Msg {
    Loaded { generation: u64, source: Arc<Source>, page: Arc<SourcePage> },
    Processed { generation: u64, output: Box<Output> },
    Failed { generation: u64, error: String },
    /// Every label (and packing slip) found in file `file`.
    Layout { file: u64, items: Vec<Item> },
    /// A new label file appeared in the watched folder.
    Watched(PathBuf),
    /// A newer release exists: (version, page URL).
    Update(String, String),
    /// "Print all" finished.
    BatchDone { printed: usize, skipped: Vec<String>, error: Option<String> },
}

#[derive(Clone, Copy, PartialEq)]
enum ToastKind {
    Ok,
    Warn,
    Error,
}

/// A line the user retyped by hand.
#[derive(Clone)]
struct LineEdit {
    text: String,
    read_back_ok: bool,
    squeeze: f32,
}

/// The inline editor over a line on the label.
struct Editing {
    line: usize,
    buf: String,
    error: Option<String>,
    focus: bool,
}

/// The tracking-number double-check dialog.
struct Verify {
    line: usize,
    buf: String,
    error: Option<String>,
}

/// One undo step: the things you change about the open label.
#[derive(Clone, PartialEq)]
struct Doc {
    extra_rotation: u8,
    manual_crop: Option<(u32, u32, u32, u32)>,
    darkness: i32,
    regenerate: bool,
    edits: BTreeMap<usize, String>,
    verified: Vec<usize>,
}

impl Doc {
    /// The part that needs the label processed again.
    fn processing(&self) -> (u8, Option<(u32, u32, u32, u32)>, i32, bool) {
        (self.extra_rotation, self.manual_crop, self.darkness, self.regenerate)
    }
}

/// Tracking numbers below this OCR confidence are always double-checked.
const TRACKING_SURE: f32 = 0.9;

struct Toast {
    text: String,
    kind: ToastKind,
    at: Instant,
}

pub struct App {
    prefs: Prefs,
    settings: Settings,
    source: Option<Arc<Source>>,
    page: Option<Arc<SourcePage>>,
    src_tex: Option<(TextureHandle, f32)>,
    output: Option<Output>,
    out_tex: Option<TextureHandle>,
    printers: Vec<String>,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    generation: u64,
    busy: bool,
    dirty: bool,
    drag_start: Option<Pos2>,
    toast: Option<Toast>,
    settings_open: bool,
    settings_page: usize,
    before_tex: Option<TextureHandle>,
    ocr_reviewed: bool,
    comparing: bool,
    /// A freshly loaded label that may be auto-printed once processed.
    auto_print_pending: bool,
    scroll_to_text: bool,
    /// The label as processed, before any hand edits.
    base_bitmap: Option<GrayImage>,
    edits: BTreeMap<usize, LineEdit>,
    editing: Option<Editing>,
    verify: Option<Verify>,
    verified: HashSet<usize>,
    history: Vec<Doc>,
    cursor: usize,
    /// Edits (and checks) to put back once an undo/redo has been re-processed.
    pending_restore: Option<Doc>,
    /// The driver reported different horizontal and vertical resolutions (e.g. PeriPage "HD").
    driver_uneven: bool,
    /// Bumped on every file open, to match layout results to their file.
    file_id: u64,
    items: Vec<Item>,
    layout_ready: bool,
    current_item: usize,
    /// Crop to apply once a different page has loaded (switching labels across pages).
    crop_after_load: Option<(u32, u32, u32, u32)>,
    batch_running: bool,
    auto_batch_pending: bool,
    watcher: Option<(PathBuf, Arc<AtomicBool>)>,
    update: Option<(String, String)>,
    ctx: egui::Context,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::install(&cc.egui_ctx);
        let mut prefs: Prefs = cc.storage.and_then(|s| eframe::get_value(s, "prefs")).unwrap_or_default();
        if prefs.rev < 1 {
            // Old default margin wasted ~3% of the print width; move untouched values to the new one.
            if (prefs.margin_mm - 1.5).abs() < 0.01 {
                prefs.margin_mm = Settings::default().margin_mm;
            }
        }
        prefs.rev = PREFS_REV;
        let (tx, rx) = mpsc::channel();
        let printers = print::list_printers();
        let mut app = Self {
            settings: Settings::default(),
            prefs,
            source: None,
            page: None,
            src_tex: None,
            output: None,
            out_tex: None,
            printers,
            tx,
            rx,
            generation: 0,
            busy: false,
            dirty: false,
            drag_start: None,
            toast: None,
            settings_open: false,
            settings_page: 0,
            before_tex: None,
            ocr_reviewed: false,
            comparing: false,
            auto_print_pending: false,
            scroll_to_text: false,
            base_bitmap: None,
            edits: BTreeMap::new(),
            editing: None,
            verify: None,
            verified: HashSet::new(),
            history: Vec::new(),
            cursor: 0,
            pending_restore: None,
            driver_uneven: false,
            file_id: 0,
            items: Vec::new(),
            layout_ready: false,
            current_item: 0,
            crop_after_load: None,
            batch_running: false,
            auto_batch_pending: false,
            watcher: None,
            update: None,
            ctx: cc.egui_ctx.clone(),
        };
        app.apply_look(&cc.egui_ctx);
        if app.prefs.check_updates {
            app.check_updates();
        }
        if app.prefs.archive && !app.prefs.archive_dir.is_empty() {
            extras::purge_archive(std::path::Path::new(&app.prefs.archive_dir), app.prefs.archive_keep_days);
        }
        // Handy for screenshots/docs: open straight onto a settings page.
        if let Some(page) = std::env::var("LABELCRAFT_OPEN_SETTINGS").ok().and_then(|v| v.parse().ok()) {
            app.settings_open = true;
            app.settings_page = page;
        }
        if app.prefs.printer.is_empty() || !app.printers.contains(&app.prefs.printer) {
            // Prefer anything that looks like the PeriPage, else the system default.
            app.prefs.printer = app
                .printers
                .iter()
                .find(|p| {
                    let p = p.to_lowercase();
                    p.contains("peripage") || p.contains("pb41") || p.contains("ppg")
                })
                .cloned()
                .or_else(print::default_printer)
                .unwrap_or_default();
        }
        app.sync_settings();
        if let Some(p) = initial {
            app.open(p, &cc.egui_ctx);
        }
        app
    }

    fn sync_settings(&mut self) {
        let p = &self.prefs;
        let r = printers::resolve(&p.printer_profile, &p.printer, p.dpi);
        self.settings.dpi = r.dpi;
        // Never draw wider than the print head can print.
        self.settings.width_mm = r.profile.map_or(p.width_mm, |pr| p.width_mm.min(pr.head_mm));
        self.settings.height_mm = p.height_mm;
        self.settings.margin_mm = p.margin_mm;
        self.settings.darkness = p.darkness;
        self.settings.regenerate_barcodes = p.regenerate;
        self.settings.vector_render = p.vector_render;
        self.settings.auto_crop = p.auto_crop;
        self.settings.straighten = p.straighten;
        self.settings.auto_rotate = p.auto_rotate;
        self.settings.bolder = p.bolder;
        self.settings.thicker_bars = p.thicker_bars;
        self.settings.offset_x_mm = p.offset_x_mm;
        self.settings.offset_y_mm = p.offset_y_mm;
        self.settings.ocr = p.ocr;
        self.settings.ocr_settings.min_confidence = p.ocr_confidence;
    }

    /// Theme palettes, light/dark mode, interface size and motion.
    fn apply_look(&self, ctx: &egui::Context) {
        theme::set_palettes(ctx, &self.prefs.light_theme, &self.prefs.dark_theme);
        apply_theme(ctx, self.prefs.theme);
        ctx.set_zoom_factor(self.prefs.ui_scale.clamp(0.75, 1.5));
        let t = if self.prefs.reduce_motion { 0.0 } else { 0.2 };
        ctx.all_styles_mut(|s| s.animation_time = t);
    }

    /// Fingerprint of everything that changes the processed label.
    fn processing_key(&self) -> String {
        let s = &self.settings;
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            s.dpi, s.width_mm, s.height_mm, s.margin_mm, s.darkness, s.regenerate_barcodes, s.vector_render, s.auto_crop,
            s.straighten, s.auto_rotate, s.bolder, s.thicker_bars, s.offset_x_mm, s.offset_y_mm, s.ocr, s.ocr_settings.min_confidence
        )
    }

    fn review_pending(&self) -> bool {
        self.prefs.ocr_review && !self.ocr_reviewed && self.output.as_ref().is_some_and(|o| o.before_ocr.is_some())
    }

    fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), kind, at: Instant::now() });
    }

    fn spawn(&mut self, ctx: &egui::Context, job: impl FnOnce(u64) -> Msg + Send + 'static) {
        self.generation += 1;
        let generation = self.generation;
        self.busy = true;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job(generation));
            ctx.request_repaint();
        });
    }

    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.auto_print_pending = self.prefs.auto_print;
        self.auto_batch_pending = self.prefs.auto_print;
        self.remember_recent(&path);
        self.start_layout(path.clone(), ctx);
        self.settings.manual_crop = None;
        self.settings.extra_rotation = 0;
        self.settings.page = 0;
        self.spawn(ctx, move |generation| match Source::open(&path).and_then(|s| s.render_page(0).map(|p| (s, p))) {
            Ok((s, p)) => Msg::Loaded { generation, source: Arc::new(s), page: Arc::new(p) },
            Err(e) => Msg::Failed { generation, error: format!("{e:#}") },
        });
    }

    fn browse(&mut self, ctx: &egui::Context) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("Shipping labels", &["pdf", "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif"])
            .pick_file()
        {
            self.open(p, ctx);
        }
    }

    fn change_page(&mut self, page: usize, ctx: &egui::Context) {
        let Some(src) = self.source.clone() else { return };
        self.settings.page = page;
        self.settings.manual_crop = None;
        self.spawn(ctx, move |generation| match src.render_page(page) {
            Ok(p) => Msg::Loaded { generation, source: src, page: Arc::new(p) },
            Err(e) => Msg::Failed { generation, error: format!("{e:#}") },
        });
    }

    fn reprocess(&mut self, ctx: &egui::Context) {
        let Some(page) = self.page.clone() else { return };
        self.dirty = false;
        let settings = self.settings.clone();
        self.spawn(ctx, move |generation| match crate::pipeline::process(&page, &settings) {
            Ok(o) => Msg::Processed { generation, output: Box::new(o) },
            Err(e) => Msg::Failed { generation, error: format!("{e:#}") },
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Loaded { generation, source, page } if generation == self.generation => {
                    self.src_tex = Some(preview_texture(ctx, &page.gray));
                    self.source = Some(source);
                    self.page = Some(page);
                    self.output = None;
                    self.out_tex = None;
                    self.history.clear();
                    self.cursor = 0;
                    self.pending_restore = None;
                    if let Some(c) = self.crop_after_load.take() {
                        self.settings.manual_crop = Some(c);
                    }
                    self.reprocess(ctx);
                }
                Msg::Processed { generation, output } if generation == self.generation => {
                    let img = gray_to_color(&output.bitmap);
                    // Smooth when shrunk on screen, crisp when zoomed in.
                    let opts = TextureOptions {
                        magnification: egui::TextureFilter::Nearest,
                        minification: egui::TextureFilter::Linear,
                        mipmap_mode: Some(egui::TextureFilter::Linear),
                        ..TextureOptions::LINEAR
                    };
                    self.out_tex = Some(ctx.load_texture("label", img, opts));
                    self.before_tex = output.before_ocr.as_ref().map(|b| ctx.load_texture("label-before", gray_to_color(b), opts));
                    self.base_bitmap = Some(output.bitmap.clone());
                    self.output = Some(*output);
                    self.busy = false;
                    self.ocr_reviewed = false;
                    self.comparing = false;
                    // Hand edits belong to the label as it was; a re-process starts clean.
                    self.edits.clear();
                    self.editing = None;
                    self.verified.clear();
                    self.verify = self.tracking_pending().map(|line| Verify { line, buf: self.line_text(line), error: None });
                    // Handy for screenshots/docs: LABELCRAFT_DEMO_EDIT="N=text;M=text", LABELCRAFT_DEMO_OPEN=N.
                    if let Ok(demo) = std::env::var("LABELCRAFT_DEMO_EDIT") {
                        for part in demo.split(';') {
                            if let Some((n, text)) = part.split_once('=').and_then(|(n, t)| Some((n.parse::<usize>().ok()?, t))) {
                                if let Err(e) = self.commit_edit(ctx, n, text) {
                                    self.toast(ToastKind::Error, e);
                                }
                            }
                        }
                    }
                    if let Some(doc) = self.pending_restore.take() {
                        self.restore_edits(ctx, &doc);
                    }
                    if let Some(n) = std::env::var("LABELCRAFT_DEMO_OPEN").ok().and_then(|v| v.parse::<usize>().ok()) {
                        self.editing = Some(Editing { line: n, buf: self.line_text(n), error: None, focus: true });
                    }
                    self.maybe_auto_print();
                }
                Msg::Failed { generation, error } if generation == self.generation => {
                    self.busy = false;
                    self.toast(ToastKind::Error, error);
                }
                Msg::Layout { file, items } if file == self.file_id => {
                    self.items = items;
                    self.layout_ready = true;
                    if let Some(n) = std::env::var("LABELCRAFT_DEMO_ITEM").ok().and_then(|v| v.parse().ok()) {
                        self.select_item(n, ctx);
                    }
                    if self.items.len() > 1 && std::mem::take(&mut self.auto_batch_pending) {
                        self.auto_print_pending = false;
                        self.print_all();
                    } else {
                        self.maybe_auto_print();
                    }
                }
                Msg::Watched(path) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.toast(ToastKind::Ok, format!("New label: {name}"));
                    self.open(path, ctx);
                }
                Msg::Update(version, url) => self.update = Some((version, url)),
                Msg::BatchDone { printed, skipped, error } => {
                    self.batch_running = false;
                    match (error, skipped.is_empty()) {
                        (Some(e), _) => self.toast(ToastKind::Error, format!("Printed {printed}, then stopped: {e}")),
                        (None, true) => self.toast(ToastKind::Ok, format!("Printed all {printed}")),
                        (None, false) => self.toast(ToastKind::Warn, format!("Printed {printed}; skipped {}", skipped.join("; "))),
                    }
                }
                _ => {} // stale
            }
        }
        if self.dirty && !self.busy {
            self.reprocess(ctx);
        }
    }

    // ───────────────────────────── files, queue, watching ─────────────────────────────

    fn remember_recent(&mut self, path: &std::path::Path) {
        let p = path.to_string_lossy().into_owned();
        // Clipboard pastes are temporary files: not worth remembering.
        if path.starts_with(std::env::temp_dir()) {
            return;
        }
        self.prefs.recent.retain(|r| r.path != p);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
        self.prefs.recent.insert(0, Recent { path: p, name, opened: now_secs() });
        self.prefs.recent.truncate(20);
    }

    /// Find every label (and packing slip) in the file, in the background.
    fn start_layout(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.file_id += 1;
        self.items.clear();
        self.layout_ready = false;
        self.current_item = 0;
        self.crop_after_load = None;
        let (file, tx, ctx, slips) = (self.file_id, self.tx.clone(), ctx.clone(), self.prefs.print_slips);
        std::thread::spawn(move || {
            let mut items = Vec::new();
            if let Ok(src) = Source::open(&path) {
                for page in 0..src.page_count.min(30) {
                    let Ok(pg) = src.render_page(page) else { continue };
                    let l = crate::pipeline::find_labels(&pg.gray, pg.is_vector);
                    items.extend(l.labels.into_iter().map(|crop| Item { page, crop, slip: false }));
                    if slips {
                        items.extend(l.slip.map(|crop| Item { page, crop, slip: true }));
                    }
                }
            }
            let _ = tx.send(Msg::Layout { file, items });
            ctx.request_repaint();
        });
    }

    fn select_item(&mut self, i: usize, ctx: &egui::Context) {
        let Some(item) = self.items.get(i).cloned() else { return };
        self.current_item = i;
        self.editing = None;
        self.verify = None;
        if item.page != self.settings.page {
            self.crop_after_load = Some(item.crop);
            self.change_page(item.page, ctx);
        } else {
            self.settings.manual_crop = Some(item.crop);
            self.dirty = true;
        }
    }

    /// Print every label (and slip) in the file, one after another, in the background.
    /// Labels whose barcodes don't all verify are skipped. Re-typeset text can't be reviewed
    /// per label, so when review is on the batch prints the original text.
    fn print_all(&mut self) {
        let Some(src) = self.source.clone() else { return };
        if self.prefs.printer.is_empty() {
            self.toast(ToastKind::Warn, "Pick a printer first");
            return;
        }
        let items = self.items.clone();
        let mut settings = self.settings.clone();
        if self.prefs.ocr && self.prefs.ocr_review {
            settings.ocr = false;
        }
        settings.extra_rotation = 0;
        let (printer, cups, copies, lang) = (self.prefs.printer.clone(), self.prefs.cups_extra.clone(), self.prefs.copies.max(1), self.raw_lang());
        let archive = (self.prefs.archive && !self.prefs.archive_dir.is_empty()).then(|| PathBuf::from(&self.prefs.archive_dir));
        let keep_days = self.prefs.archive_keep_days;
        let stem = std::path::Path::new(&src.name).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "label".into());
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.batch_running = true;
        self.toast(ToastKind::Ok, format!("Printing {} labels…", items.len()));
        std::thread::spawn(move || {
            let (mut printed, mut skipped, mut error) = (0, Vec::new(), None);
            let mut labels_seen = 0;
            for item in &items {
                let what = if item.slip {
                    "packing slip".to_string()
                } else {
                    labels_seen += 1;
                    format!("label {labels_seen}")
                };
                let result = (|| -> anyhow::Result<bool> {
                    let page = src.render_page(item.page)?;
                    let mut s = settings.clone();
                    s.page = item.page;
                    s.manual_crop = Some(item.crop);
                    let out = crate::pipeline::process(&page, &s)?;
                    let verified = !out.barcodes.is_empty() && out.barcodes.iter().all(|b| !matches!(b.status, Status::Kept(_)));
                    if !item.slip && !verified {
                        return Ok(false);
                    }
                    match lang {
                        Some(RawLang::Zpl) => print::print_raw(&printer, &printers::to_zpl(&out.bitmap, 0, copies))?,
                        Some(RawLang::Tspl) => print::print_raw(&printer, &printers::to_tspl(&out.bitmap, s.width_mm, s.height_mm, 0, copies))?,
                        None => {
                            print::print(&printer, &out.bitmap, &s, &cups, copies)?;
                        }
                    }
                    if let Some(dir) = &archive {
                        let path = dir.join(format!("labelcraft-{stem}-{}-{}.png", what.replace(' ', "-"), now_secs()));
                        if let Ok(b) = pdfout::label_png(&out.bitmap, s.dpi) {
                            let _ = std::fs::write(path, b);
                        }
                        extras::purge_archive(dir, keep_days);
                    }
                    Ok(true)
                })();
                match result {
                    Ok(true) => printed += 1,
                    Ok(false) => skipped.push(format!("{what} (barcodes not verified)")),
                    Err(e) => {
                        error = Some(format!("{what}: {e:#}"));
                        break;
                    }
                }
            }
            let _ = tx.send(Msg::BatchDone { printed, skipped, error });
            ctx.request_repaint();
        });
    }

    /// Start, stop or move the folder watcher to match the settings.
    fn sync_watcher(&mut self) {
        let want = self.prefs.watch.then(|| {
            if self.prefs.watch_dir.is_empty() { extras::default_watch_dir() } else { PathBuf::from(&self.prefs.watch_dir) }
        });
        if want.as_ref() == self.watcher.as_ref().map(|w| &w.0) {
            return;
        }
        if let Some((_, stop)) = self.watcher.take() {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(dir) = want {
            let stop = Arc::new(AtomicBool::new(false));
            let (flag, tx, ctx, d) = (stop.clone(), self.tx.clone(), self.ctx.clone(), dir.clone());
            std::thread::spawn(move || {
                extras::watch(
                    d,
                    || !flag.load(Ordering::Relaxed),
                    |p| {
                        let _ = tx.send(Msg::Watched(p));
                        ctx.request_repaint();
                    },
                )
            });
            self.watcher = Some((dir, stop));
        }
    }

    fn check_updates(&mut self) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            if let Some((v, url)) = extras::newer_release() {
                let _ = tx.send(Msg::Update(v, url));
                ctx.request_repaint();
            }
        });
    }

    fn paste(&mut self, ctx: &egui::Context) {
        match extras::clipboard_label() {
            Ok(p) => self.open(p, ctx),
            Err(e) => self.toast(ToastKind::Warn, e),
        }
    }

    fn print_calibration(&mut self, save: bool) {
        let s = &self.settings;
        let g = extras::calibration_page(s.width_mm, s.height_mm, s.dpi, (s.offset_x_mm, s.offset_y_mm));
        if save {
            let Some(path) = rfd::FileDialog::new().set_file_name("labelcraft-calibration.png").add_filter("PNG image", &["png"]).save_file() else { return };
            match pdfout::label_png(&g, s.dpi).map_err(|e| e.to_string()).and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string())) {
                Ok(()) => self.toast(ToastKind::Ok, "Saved calibration page"),
                Err(e) => self.toast(ToastKind::Error, format!("Couldn't save: {e}")),
            }
            return;
        }
        if self.prefs.printer.is_empty() {
            self.toast(ToastKind::Warn, "Pick a printer first");
            return;
        }
        let sent = match self.raw_lang() {
            Some(lang) => print::print_raw(&self.prefs.printer, &self.raw_job(lang, &g, 1)),
            None => print::print(&self.prefs.printer, &g, &self.settings, &self.prefs.cups_extra, 1).map(|_| ()),
        };
        match sent {
            Ok(()) => self.toast(ToastKind::Ok, "Calibration page sent"),
            Err(e) => self.toast(ToastKind::Error, format!("Couldn't print: {e:#}")),
        }
    }

    /// Save a copy with all text and barcodes blurred, for sharing.
    fn save_shareable(&mut self) {
        let Some(out) = &self.output else { return };
        let text: Vec<(u32, u32, u32, u32)> =
            if out.ocr.is_empty() { crate::ocr::text_regions(&out.bitmap) } else { out.ocr.iter().map(|l| l.region).collect() };
        let codes: Vec<(u32, u32, u32, u32)> = out.barcodes.iter().map(|b| b.region).collect();
        let g = extras::redacted(&out.bitmap, &text, &codes);
        let stem = self
            .source
            .as_ref()
            .and_then(|s| std::path::Path::new(&s.name).file_stem().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "label".into());
        let Some(path) = rfd::FileDialog::new().set_file_name(format!("{stem}-shareable.png")).add_filter("PNG image", &["png"]).save_file() else { return };
        match g.save(&path) {
            Ok(()) => self.toast(ToastKind::Ok, "Saved a copy with names, addresses and codes blurred"),
            Err(e) => self.toast(ToastKind::Error, format!("Couldn't save: {e}")),
        }
    }

    // ───────────────────────────── printer model ─────────────────────────────

    fn resolved_printer(&self) -> printers::Resolved {
        printers::resolve(&self.prefs.printer_profile, &self.prefs.printer, self.prefs.dpi)
    }

    /// The language to print in directly, if direct printing is on.
    fn raw_lang(&self) -> Option<RawLang> {
        match self.prefs.raw_mode.as_str() {
            "auto" => self.resolved_printer().profile.and_then(|p| p.raw),
            other => RawLang::from_id(other),
        }
    }

    fn raw_job(&self, lang: RawLang, bitmap: &GrayImage, copies: u32) -> Vec<u8> {
        // Darkness is already baked into the bitmap's threshold; leave the printer's heat alone.
        match lang {
            RawLang::Zpl => printers::to_zpl(bitmap, 0, copies),
            RawLang::Tspl => printers::to_tspl(bitmap, self.settings.width_mm, self.settings.height_mm, 0, copies),
        }
    }

    /// Print once through the driver into a file instead of the printer.
    fn capture_driver(&mut self) {
        let Some(out) = &self.output else {
            self.toast(ToastKind::Warn, "Open a label first");
            return;
        };
        if self.prefs.printer.is_empty() {
            self.toast(ToastKind::Warn, "Pick a printer first");
            return;
        }
        let safe: String = self.prefs.printer.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let Some(path) = rfd::FileDialog::new().set_file_name(format!("driver-capture-{safe}.bin")).save_file() else { return };
        let bitmap = out.bitmap.clone();
        match print::capture_driver_output(&self.prefs.printer, &bitmap, &self.settings, &self.prefs.cups_extra, &path) {
            Ok(dpi) => {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let res = dpi.map_or(String::new(), |(x, y)| format!(" · driver at {x}×{y} dpi"));
                self.toast(ToastKind::Ok, format!("Captured {} KB{res}. Nothing was printed.", size / 1024));
                if dpi.is_some_and(|(x, y)| (x - y).abs() > 1.0) {
                    self.driver_uneven = true;
                }
            }
            Err(e) => self.toast(ToastKind::Error, format!("Couldn't capture: {e:#}")),
        }
    }

    fn save_raw(&mut self, lang: RawLang) {
        let Some(out) = &self.output else {
            self.toast(ToastKind::Warn, "Open a label first");
            return;
        };
        let job = self.raw_job(lang, &out.bitmap, 1);
        let stem = self
            .source
            .as_ref()
            .and_then(|s| std::path::Path::new(&s.name).file_stem().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "label".into());
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{stem}-print.{}", lang.ext()))
            .add_filter(lang.name(), &[lang.ext()])
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, job) {
            Ok(()) => self.toast(ToastKind::Ok, format!("Saved {} job", lang.name())),
            Err(e) => self.toast(ToastKind::Error, format!("Couldn't save: {e}")),
        }
    }

    // ───────────────────────────── hand edits ─────────────────────────────

    /// Text of line `i` as it is on the label now.
    fn line_text(&self, i: usize) -> String {
        self.edits
            .get(&i)
            .map(|e| e.text.clone())
            .or_else(|| self.output.as_ref().and_then(|o| o.ocr.get(i)).map(|l| l.text.clone()))
            .unwrap_or_default()
    }

    /// Why tracking line `i` needs a double-check, if it does.
    fn verify_reason(&self, i: usize) -> Option<String> {
        let l = self.output.as_ref()?.ocr.get(i)?;
        if l.kind != LineKind::Tracking || self.verified.contains(&i) {
            return None;
        }
        // Only text LabelCraft drew needs checking: original print that was kept is untouched.
        if self.edits.contains_key(&i) {
            Some("You changed it.".into())
        } else if matches!(l.status, LineStatus::Kept(_)) {
            None
        } else if l.confidence < TRACKING_SURE {
            Some(format!("The text reader was only {:.0}% sure.", l.confidence * 100.0))
        } else {
            None
        }
    }

    fn tracking_pending(&self) -> Option<usize> {
        let n = self.output.as_ref().map_or(0, |o| o.ocr.len());
        (0..n).find(|&i| self.verify_reason(i).is_some())
    }

    /// Redraw the label from the processed bitmap with `edits` applied.
    fn render_edits(&self, edits: &BTreeMap<usize, LineEdit>) -> Result<(GrayImage, BTreeMap<usize, LineEdit>), String> {
        let (Some(out), Some(base)) = (&self.output, &self.base_bitmap) else { return Err("No label open".into()) };
        let mut bitmap = base.clone();
        let mut done = BTreeMap::new();
        for (&i, e) in edits {
            let line = out.ocr.get(i).ok_or("That line no longer exists")?;
            let r = crate::ocr::edit_line(&mut bitmap, line, &e.text)?;
            done.insert(i, LineEdit { text: e.text.clone(), read_back_ok: r.reads_back(&e.text), squeeze: r.squeeze });
        }
        Ok((bitmap, done))
    }

    fn install_edits(&mut self, ctx: &egui::Context, bitmap: GrayImage, edits: BTreeMap<usize, LineEdit>) {
        let opts = TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Linear,
            mipmap_mode: Some(egui::TextureFilter::Linear),
            ..TextureOptions::LINEAR
        };
        self.out_tex = Some(ctx.load_texture("label", gray_to_color(&bitmap), opts));
        if let Some(out) = &mut self.output {
            out.bitmap = bitmap;
        }
        self.edits = edits;
    }

    /// Put `text` on line `i`. Numbers that are also in a barcode can't change.
    fn commit_edit(&mut self, ctx: &egui::Context, i: usize, text: &str) -> Result<(), String> {
        let text = text.trim().to_string();
        let Some(line) = self.output.as_ref().and_then(|o| o.ocr.get(i)).cloned() else { return Err("No such line".into()) };
        if text == self.line_text(i) {
            return Ok(());
        }
        if line.kind == LineKind::BarcodeNumber && crate::ocr::digits_of(&text) != crate::ocr::digits_of(&line.text) {
            return Err("The numbers on this line are also in a barcode, so they can't change.".into());
        }
        let mut edits = self.edits.clone();
        if text == line.text && matches!(line.status, LineStatus::Retypeset { .. }) {
            edits.remove(&i);
        } else {
            edits.insert(i, LineEdit { text: text.clone(), read_back_ok: true, squeeze: 1.0 });
        }
        let (bitmap, edits) = self.render_edits(&edits)?;
        self.install_edits(ctx, bitmap, edits);
        if self.prefs.ocr_review {
            self.ocr_reviewed = false;
        }
        if line.kind == LineKind::Tracking {
            self.verified.remove(&i);
            self.verify = Some(Verify { line: i, buf: self.line_text(i), error: None });
        }
        if let Some(e) = self.edits.get(&i).filter(|e| !e.read_back_ok) {
            let _ = e;
            self.toast(ToastKind::Warn, "Changed, but it reads back slightly differently: check it closely");
        }
        Ok(())
    }

    fn undo_edit(&mut self, ctx: &egui::Context, i: usize) {
        let mut edits = self.edits.clone();
        edits.remove(&i);
        if let Ok((bitmap, edits)) = self.render_edits(&edits) {
            self.install_edits(ctx, bitmap, edits);
        }
        self.verified.remove(&i);
    }

    fn revert_all_edits(&mut self, ctx: &egui::Context) {
        if let Some(base) = self.base_bitmap.clone() {
            self.install_edits(ctx, base, BTreeMap::new());
        }
        self.verified.clear();
        self.editing = None;
    }

    // ───────────────────────────── undo / redo ─────────────────────────────

    fn doc(&self) -> Doc {
        let mut verified: Vec<usize> = self.verified.iter().copied().collect();
        verified.sort_unstable();
        Doc {
            extra_rotation: self.settings.extra_rotation,
            manual_crop: self.settings.manual_crop,
            darkness: self.prefs.darkness,
            regenerate: self.prefs.regenerate,
            edits: self.edits.iter().map(|(&i, e)| (i, e.text.clone())).collect(),
            verified,
        }
    }

    /// Record a step once the label has settled (nothing processing, nothing being dragged or typed).
    fn record_history(&mut self, ctx: &egui::Context) {
        if self.output.is_none() || self.busy || self.dirty || self.pending_restore.is_some() || self.editing.is_some() {
            return;
        }
        if ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let doc = self.doc();
        if self.history.get(self.cursor) == Some(&doc) {
            return;
        }
        if self.history.is_empty() {
            self.history.push(doc);
            self.cursor = 0;
            return;
        }
        // A re-process clears hand edits by design: fold that into the step that caused it.
        let prev = &self.history[self.cursor];
        if doc.edits.is_empty() && !prev.edits.is_empty() && doc.processing() == prev.processing() && self.cursor > 0 {
            let before = &self.history[self.cursor - 1];
            if before.processing() != doc.processing() {
                self.history[self.cursor] = doc;
                return;
            }
        }
        self.history.truncate(self.cursor + 1);
        self.history.push(doc);
        if self.history.len() > 200 {
            self.history.remove(0);
        }
        self.cursor = self.history.len() - 1;
    }

    fn can_undo(&self) -> bool {
        self.cursor > 0 && !self.busy
    }

    fn can_redo(&self) -> bool {
        self.cursor + 1 < self.history.len() && !self.busy
    }

    fn undo(&mut self, ctx: &egui::Context) {
        if self.can_undo() {
            self.cursor -= 1;
            self.apply_doc(ctx, self.history[self.cursor].clone());
        }
    }

    fn redo(&mut self, ctx: &egui::Context) {
        if self.can_redo() {
            self.cursor += 1;
            self.apply_doc(ctx, self.history[self.cursor].clone());
        }
    }

    fn apply_doc(&mut self, ctx: &egui::Context, doc: Doc) {
        self.editing = None;
        self.verify = None;
        if doc.processing() != self.doc().processing() {
            self.settings.extra_rotation = doc.extra_rotation;
            self.settings.manual_crop = doc.manual_crop;
            self.prefs.darkness = doc.darkness;
            self.prefs.regenerate = doc.regenerate;
            self.sync_settings();
            self.pending_restore = Some(doc);
            self.dirty = true;
        } else {
            self.restore_edits(ctx, &doc);
        }
    }

    /// Put a step's hand edits and tracking checks back on the current label.
    fn restore_edits(&mut self, ctx: &egui::Context, doc: &Doc) {
        let edits: BTreeMap<usize, LineEdit> =
            doc.edits.iter().map(|(&i, t)| (i, LineEdit { text: t.clone(), read_back_ok: true, squeeze: 1.0 })).collect();
        match self.render_edits(&edits) {
            Ok((bitmap, edits)) => self.install_edits(ctx, bitmap, edits),
            Err(e) => self.toast(ToastKind::Error, format!("Couldn't restore that step: {e}")),
        }
        self.verified = doc.verified.iter().copied().collect();
        self.verify = None;
    }

    fn do_print(&mut self) {
        if let Some(line) = self.tracking_pending() {
            self.verify = Some(Verify { line, buf: self.line_text(line), error: None });
            self.toast(ToastKind::Warn, "Double-check the tracking number first");
            return;
        }
        if self.review_pending() {
            self.toast(ToastKind::Warn, "Check the re-typeset text first");
            return;
        }
        let Some(out) = &self.output else { return };
        if self.prefs.printer.is_empty() {
            self.toast(ToastKind::Warn, "Pick a printer first");
            return;
        }
        let copies = self.prefs.copies.max(1);
        let sent = match self.raw_lang() {
            Some(lang) => print::print_raw(&self.prefs.printer, &self.raw_job(lang, &out.bitmap, copies)).map(|_| None),
            None => print::print(&self.prefs.printer, &out.bitmap, &self.settings, &self.prefs.cups_extra, copies),
        };
        let uneven = matches!(sent, Ok(Some((x, y))) if (x - y).abs() > 1.0);
        let sent = sent.map(|_| ());
        if uneven {
            self.driver_uneven = true;
        }
        match sent {
            Ok(()) => {
                let p = self.prefs.printer.clone();
                let what = if copies > 1 { format!("{copies} copies sent to {p}") } else { format!("Sent to {p}") };
                let archived = self.archive_copy();
                match archived {
                    Some(Err(e)) => self.toast(ToastKind::Warn, format!("{what}, but couldn't save a copy: {e}")),
                    _ => self.toast(ToastKind::Ok, what),
                }
            }
            Err(e) => self.toast(ToastKind::Error, format!("Couldn't print: {e:#}")),
        }
        if uneven {
            self.toast(ToastKind::Warn, "Sent. Tip: this is an \"HD\" driver (uneven resolution); the SD driver gives sharper barcodes");
        }
    }

    /// Save a copy of the printed label into the archive folder, if that's switched on.
    fn archive_copy(&self) -> Option<Result<(), String>> {
        if !self.prefs.archive || self.prefs.archive_dir.is_empty() {
            return None;
        }
        let out = self.output.as_ref()?;
        let stem = self
            .source
            .as_ref()
            .and_then(|s| std::path::Path::new(&s.name).file_stem().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "label".into());
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let path = std::path::Path::new(&self.prefs.archive_dir).join(format!("labelcraft-{stem}-{secs}.png"));
        let r = pdfout::label_png(&out.bitmap, self.settings.dpi)
            .map_err(|e| e.to_string())
            .and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string()));
        extras::purge_archive(std::path::Path::new(&self.prefs.archive_dir), self.prefs.archive_keep_days);
        Some(r)
    }

    fn maybe_auto_print(&mut self) {
        // Wait until we know whether the file holds several labels (then "Print all" runs).
        if !self.layout_ready || self.output.is_none() || self.busy {
            return;
        }
        if !std::mem::take(&mut self.auto_print_pending) {
            return;
        }
        let Some(out) = &self.output else { return };
        let all_ok = !out.barcodes.is_empty() && out.barcodes.iter().all(|b| !matches!(b.status, Status::Kept(_)));
        if !all_ok {
            self.toast(ToastKind::Warn, "Not printed automatically: not every barcode was verified");
        } else if self.tracking_pending().is_some() {
            self.toast(ToastKind::Warn, "Not printed automatically: the tracking number needs a double-check");
        } else if self.review_pending() {
            self.toast(ToastKind::Warn, "Not printed automatically: re-typeset text needs a look first");
        } else {
            self.do_print();
        }
    }

    fn save(&mut self, pdf: bool) {
        let Some(out) = &self.output else { return };
        let stem = self
            .source
            .as_ref()
            .and_then(|s| std::path::Path::new(&s.name).file_stem().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "label".into());
        let (ext, name) = if pdf { ("pdf", "PDF") } else { ("png", "PNG image") };
        let Some(path) = rfd::FileDialog::new().set_file_name(format!("{stem}-print.{ext}")).add_filter(name, &[ext]).save_file()
        else {
            return;
        };
        let bytes = if pdf {
            Ok(pdfout::label_pdf(&out.bitmap, self.settings.width_mm, self.settings.height_mm))
        } else {
            pdfout::label_png(&out.bitmap, self.settings.dpi)
        };
        match bytes.and_then(|b| std::fs::write(&path, b).map_err(Into::into)) {
            Ok(()) => {
                let n = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.toast(ToastKind::Ok, format!("Saved {n}"));
            }
            Err(e) => self.toast(ToastKind::Error, format!("Couldn't save: {e:#}")),
        }
    }

    // ───────────────────────────── chrome ─────────────────────────────

    fn top_bar(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let ctx = ui.ctx().clone();
        ui.horizontal_centered(|ui| {
            ui.add_space(4.0);
            let (r, _) = ui.allocate_exact_size(vec2(240.0, 34.0), Sense::hover());
            let logo = Rect::from_min_size(pos2(r.left(), r.center().y - 16.0), Vec2::splat(32.0));
            widgets::logo(ui.painter(), logo, t);
            let p = ui.painter();
            p.text(pos2(logo.right() + 11.0, r.center().y - 8.5), Align2::LEFT_CENTER, "LabelCraft", theme::semibold(16.0), t.text);
            p.text(pos2(logo.right() + 11.0, r.center().y + 9.0), Align2::LEFT_CENTER, "Crisp shipping labels, ready to print", theme::body(11.5), t.text_faint);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                if widgets::icon_button(ui, t, "settings-2", &format!("Settings ({MOD},)"), 34.0).clicked() {
                    self.settings_open = !self.settings_open;
                }
                let mut theme = self.prefs.theme;
                if widgets::segmented(ui, t, &[("sun", "Light"), ("monitor", "Match system"), ("moon", "Dark")], &mut theme) {
                    self.prefs.theme = theme;
                    apply_theme(&ctx, theme);
                }
                ui.add_space(6.0);
                if widgets::ghost_button(ui, t, "folder-open", "Open…", 92.0, true).on_hover_text(format!("{MOD}O")).clicked() {
                    self.browse(&ctx);
                }
                let recent = widgets::icon_button(ui, t, "history", "Recent labels", 34.0);
                egui::Popup::menu(&recent).show(|ui| {
                    ui.set_min_width(280.0);
                    if self.prefs.recent.is_empty() {
                        ui.label(RichText::new("Nothing opened yet").color(t.text_faint));
                    }
                    let mut open = None;
                    for r in &self.prefs.recent {
                        let ago = now_secs().saturating_sub(r.opened);
                        let when = match ago {
                            0..3600 => format!("{} min ago", ago / 60),
                            3600..86400 => format!("{} h ago", ago / 3600),
                            _ => format!("{} d ago", ago / 86400),
                        };
                        if ui.button(format!("{}   ·   {when}", r.name)).on_hover_text(&r.path).clicked() {
                            open = Some(r.path.clone());
                        }
                    }
                    if let Some(p) = open {
                        let pb = PathBuf::from(&p);
                        if pb.is_file() {
                            self.open(pb, &ctx);
                        } else {
                            self.prefs.recent.retain(|r| r.path != p);
                            self.toast(ToastKind::Warn, "That file has moved or been deleted");
                        }
                    }
                });
                if let Some((v, url)) = self.update.clone().filter(|(v, _)| *v != self.prefs.skipped_version) {
                    ui.add_space(6.0);
                    if widgets::icon_button(ui, t, "x", "Skip this version", 26.0).clicked() {
                        self.prefs.skipped_version = v.clone();
                    }
                    if ui
                        .add(egui::Button::new(RichText::new(format!("Update {v}")).font(theme::semibold(12.0)).color(t.on_accent)).fill(t.accent).corner_radius(R_PILL))
                        .on_hover_text("Open the release page")
                        .clicked()
                    {
                        ctx.open_url(egui::OpenUrl::new_tab(url));
                    }
                }
                ui.add_space(10.0);
                let redo_tip = if cfg!(target_os = "macos") { "Redo (⇧⌘Z)".to_string() } else { "Redo (Ctrl Y)".to_string() };
                if ui.add_enabled_ui(self.can_redo(), |ui| widgets::icon_button(ui, t, "redo-2", &redo_tip, 34.0)).inner.clicked() {
                    self.redo(&ctx);
                }
                if ui.add_enabled_ui(self.can_undo(), |ui| widgets::icon_button(ui, t, "undo-2", &format!("Undo ({MOD}Z)"), 34.0)).inner.clicked() {
                    self.undo(&ctx);
                }
            });
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui, t: &Tokens) {
        let (w, h) = self.settings.target_size();
        ui.horizontal_centered(|ui| {
            ui.add_space(6.0);
            let dot = if self.busy { t.warn } else if self.output.is_some() { t.ok } else { t.text_faint };
            let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 3.5, dot);
            let s = format!(
                "{} × {} mm  ·  {} dpi  ·  {w} × {h} dots",
                self.settings.width_mm, self.settings.height_mm, self.settings.dpi
            );
            ui.label(RichText::new(s).font(theme::mono(11.0)).color(t.text_muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(6.0);
                let name = self.source.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| "No label loaded".into());
                ui.add(egui::Label::new(RichText::new(name).font(theme::body(11.5)).color(t.text_faint)).truncate());
            });
        });
    }

    // ───────────────────────────── inspector ─────────────────────────────

    fn inspector(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let ready = self.output.is_some() && !self.busy;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(14.0);
            ui.spacing_mut().item_spacing.y = 12.0;

            // ── Labels in this file ──
            if self.items.len() > 1 {
                self.queue_card(ui, t);
            }
            // ── Print ──
            widgets::card(ui, t, "printer", "Print", |_| {}, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                let w = ui.available_width();
                egui::ComboBox::from_id_salt("printer")
                    .width(w)
                    .height(240.0)
                    .selected_text(if self.prefs.printer.is_empty() { "Choose a printer" } else { &self.prefs.printer })
                    .show_ui(ui, |ui| {
                        for p in &self.printers {
                            ui.selectable_value(&mut self.prefs.printer, p.clone(), p);
                        }
                        ui.separator();
                        if ui.button("Refresh list").clicked() {
                            self.printers = print::list_printers();
                        }
                    });
                let tracking = self.tracking_pending();
                let pending = self.review_pending();
                let (icon, label) = if tracking.is_some() {
                    ("circle-alert", "Check the tracking number")
                } else if pending {
                    ("sparkles", "Check the text first")
                } else {
                    ("printer", "Print label")
                };
                let copies = if self.prefs.copies > 1 { format!("×{}  {MOD}P", self.prefs.copies) } else { format!("{MOD}P") };
                let blocked = pending || tracking.is_some();
                if widgets::primary_button(ui, t, icon, label, (!blocked).then_some(copies.as_str()), ready).clicked() {
                    if let Some(line) = tracking {
                        self.verify = Some(Verify { line, buf: self.line_text(line), error: None });
                    } else if pending {
                        self.scroll_to_text = true;
                    } else {
                        self.do_print();
                    }
                }
                {
                    let r = self.resolved_printer();
                    let mode = match self.raw_lang() {
                        Some(l) => format!("Direct {} (experimental)", l.name()),
                        None => "Through the printer driver".to_string(),
                    };
                    let model = r.profile.map_or("Custom printer".to_string(), |p| p.name.split(" (").next().unwrap_or(p.name).to_string());
                    ui.label(RichText::new(format!("{model} · {} dpi · {mode}", r.dpi)).font(theme::body(11.0)).color(t.text_faint));
                }
                ui.horizontal(|ui| {
                    let third = (ui.available_width() - 16.0) / 3.0;
                    if widgets::ghost_button(ui, t, "download", "PNG", third, ready).on_hover_text("Save as PNG").clicked() {
                        self.save(false);
                    }
                    if widgets::ghost_button(ui, t, "file-down", "PDF", third, ready).on_hover_text("Save as PDF").clicked() {
                        self.save(true);
                    }
                    if widgets::ghost_button(ui, t, "eye-off", "Share", third, ready)
                        .on_hover_text("Save a copy with names, addresses and codes blurred, for screenshots and bug reports")
                        .clicked()
                    {
                        self.save_shareable();
                    }
                });
            });

            // ── Adjust ──
            widgets::card(ui, t, "sparkles", "Adjust", |_| {}, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Rotate").font(theme::medium(13.0)).color(t.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::icon_button(ui, t, "refresh-cw", "Turn 180°", 32.0).clicked() {
                            self.settings.extra_rotation = (self.settings.extra_rotation + 2) % 4;
                            self.dirty = true;
                        }
                        if widgets::icon_button(ui, t, "rotate-cw", "Turn right", 32.0).clicked() {
                            self.settings.extra_rotation = (self.settings.extra_rotation + 1) % 4;
                            self.dirty = true;
                        }
                        if widgets::icon_button(ui, t, "rotate-ccw", "Turn left", 32.0).clicked() {
                            self.settings.extra_rotation = (self.settings.extra_rotation + 3) % 4;
                            self.dirty = true;
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Crop").font(theme::medium(13.0)).color(t.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.settings.manual_crop.is_some() {
                            if widgets::icon_button(ui, t, "x", "Back to automatic", 26.0).clicked() {
                                self.settings.manual_crop = None;
                                self.dirty = true;
                            }
                            widgets::badge(ui, "Manual", t.accent, t.accent_soft);
                        } else {
                            widgets::badge(ui, "Automatic", t.text_muted, t.sunken);
                        }
                    });
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Darkness").font(theme::medium(13.0)).color(t.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let v = self.prefs.darkness;
                        let s = if v == 0 { "Balanced".to_string() } else { format!("{v:+}") };
                        ui.label(RichText::new(s).font(theme::mono(12.0)).color(t.text_muted));
                    });
                });
                ui.spacing_mut().slider_width = ui.available_width();
                ui.add(egui::Slider::new(&mut self.prefs.darkness, -20..=20).show_value(false));
                ui.add_space(-8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Lighter").font(theme::body(11.0)).color(t.text_faint));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("Darker").font(theme::body(11.0)).color(t.text_faint));
                    });
                });
                ui.separator();
                widgets::switch(ui, t, &mut self.prefs.regenerate, "Regenerate barcodes", Some("Redraw each code, then re-scan to prove it"));
                widgets::switch(ui, t, &mut self.prefs.show_outlines, "Outline barcodes", None);
            });

            // ── Barcodes ──
            self.barcode_card(ui, t);
            // ── Text (OCR) ──
            if self.prefs.ocr {
                self.text_card(ui, t);
            }
            ui.add_space(8.0);
        });
    }

    fn queue_card(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let n_labels = self.items.iter().filter(|i| !i.slip).count();
        let mut pick = None;
        let mut all = false;
        let ctx = ui.ctx().clone();
        widgets::card(
            ui,
            t,
            "files",
            "In this file",
            |ui| widgets::badge(ui, &format!("{n_labels} label{}", if n_labels == 1 { "" } else { "s" }), t.accent, t.accent_soft),
            |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let mut n = 0;
                for (i, item) in self.items.iter().enumerate() {
                    let name = if item.slip {
                        format!("Packing slip · page {}", item.page + 1)
                    } else {
                        n += 1;
                        format!("Label {n} · page {}", item.page + 1)
                    };
                    let on = i == self.current_item;
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                    let hover = resp.hovered();
                    if on || hover {
                        ui.painter().rect_filled(r, R_CONTROL, if on { t.accent_soft } else { t.sunken });
                    }
                    theme::paint_icon(ui, if item.slip { "files" } else { "sticker" }, Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), Vec2::splat(15.0)), if on { t.accent } else { t.text_muted });
                    ui.painter().text(pos2(r.left() + 32.0, r.center().y), Align2::LEFT_CENTER, name, theme::medium(12.5), if on { t.text } else { t.text_muted });
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        pick = Some(i);
                    }
                }
                ui.add_space(6.0);
                let label = if self.batch_running { "Printing…".to_string() } else { format!("Print all {}", self.items.len()) };
                if widgets::primary_button(ui, t, "printer", &label, None, !self.batch_running && self.source.is_some()).clicked() {
                    all = true;
                }
                if self.prefs.ocr && self.prefs.ocr_review {
                    widgets::caption(ui, t, "Print all uses the original text: re-typeset text needs a look per label.");
                }
                widgets::caption(ui, t, "Labels whose barcodes don't all verify are skipped.");
            },
        );
        if let Some(i) = pick {
            self.select_item(i, &ctx);
        }
        if all {
            self.print_all();
        }
    }

    fn text_card(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let Some(out) = &self.output else { return };
        let done = out.ocr.iter().filter(|l| matches!(l.status, LineStatus::Retypeset { .. })).count();
        let total = out.ocr.len();
        let pending = self.review_pending();
        let mut reviewed_click = false;
        let mut compare = false;
        let (mut revert_all, mut undo_click, mut edit_click) = (false, None, None);
        if std::mem::take(&mut self.scroll_to_text) {
            ui.scroll_to_cursor(Some(Align::TOP));
        }
        widgets::card(
            ui,
            t,
            "sparkles",
            "Text",
            |ui| {
                if total > 0 {
                    widgets::badge(ui, &format!("{done} of {total} re-typeset"), t.accent, t.accent_soft);
                }
            },
            |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                if total == 0 {
                    ui.label(RichText::new("No text lines found to re-typeset.").font(theme::body(12.5)).color(t.text_faint));
                    return;
                }
                if pending {
                    egui::Frame::new()
                        .fill(t.warn_soft)
                        .corner_radius(CornerRadius::same(R_CONTROL))
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new("Compare the redrawn text with the original, especially names, numbers and ZIP codes.")
                                    .font(theme::body(12.0))
                                    .color(t.text),
                            );
                        });
                }
                if out.before_ocr.is_some() {
                    ui.horizontal(|ui| {
                        let half = (ui.available_width() - 8.0) / 2.0;
                        let hold = widgets::ghost_button(ui, t, "refresh-cw", "Hold: original", half, true);
                        compare = hold.is_pointer_button_down_on();
                        if pending && widgets::ghost_button(ui, t, "check", "Looks right", half, true).clicked() {
                            reviewed_click = true;
                        }
                    });
                }
                if !self.edits.is_empty() {
                    ui.horizontal(|ui| {
                        let n = self.edits.len();
                        ui.label(RichText::new(format!("{n} line{} edited by hand", if n == 1 { "" } else { "s" })).font(theme::medium(12.0)).color(t.text));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(egui::Button::new(RichText::new("Revert all").font(theme::medium(12.0)).color(t.accent)).frame(false)).clicked() {
                                revert_all = true;
                            }
                        });
                    });
                }
                for (i, l) in out.ocr.iter().enumerate() {
                    let edit = self.edits.get(&i);
                    let (ok, detail) = match (&l.status, edit) {
                        (_, Some(e)) => {
                            let mut d = format!("Edited · was {}", l.text);
                            if e.squeeze < 0.99 {
                                d.push_str(&format!(" · squeezed to {:.0}%", e.squeeze * 100.0));
                            }
                            if !e.read_back_ok {
                                d.push_str(" · reads back differently");
                            }
                            (true, d)
                        }
                        (LineStatus::Retypeset { barcode_match: true }, _) => (true, "Redrawn · matches a barcode".to_string()),
                        (LineStatus::Retypeset { .. }, _) => (true, format!("Redrawn · {:.0}% sure, read twice", l.confidence * 100.0)),
                        (LineStatus::Kept(why), _) => (false, format!("Original kept · {why}")),
                    };
                    let shown = edit.map_or(l.text.as_str(), |e| e.text.as_str());
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                        let dot = if edit.is_some() { t.accent } else if ok { t.accent.gamma_multiply(0.5) } else { t.warn };
                        ui.painter().circle_filled(r.center(), 4.0, dot);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.set_max_width(ui.available_width() - 30.0);
                            let resp = ui.add(egui::Label::new(RichText::new(shown).font(theme::mono(12.0)).color(if ok { t.text } else { t.text_muted })).truncate().sense(Sense::click()));
                            if resp.on_hover_text("Click to edit on the label").clicked() {
                                edit_click = Some(i);
                            }
                            ui.add(egui::Label::new(RichText::new(detail).font(theme::body(10.5)).color(t.text_faint)).truncate());
                        });
                        if edit.is_some() {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if widgets::icon_button(ui, t, "undo-2", "Undo this edit", 24.0).clicked() {
                                    undo_click = Some(i);
                                }
                            });
                        }
                    });
                }
            },
        );
        self.comparing = compare;
        let ctx = ui.ctx().clone();
        if revert_all {
            self.revert_all_edits(&ctx);
        }
        if let Some(i) = undo_click {
            self.undo_edit(&ctx, i);
        }
        if let Some(i) = edit_click {
            if self.verify.is_none() {
                self.editing = Some(Editing { line: i, buf: self.line_text(i), error: None, focus: true });
            }
        }
        if reviewed_click {
            self.ocr_reviewed = true;
            self.toast(ToastKind::Ok, "Thanks, ready to print");
        }
    }

    fn barcode_card(&self, ui: &mut egui::Ui, t: &Tokens) {
        let Some(out) = &self.output else {
            widgets::card(ui, t, "scan-barcode", "Barcodes", |_| {}, |ui| {
                ui.label(
                    RichText::new("Open a label and every barcode will show up here once it has been redrawn and checked.")
                        .font(theme::body(12.5))
                        .color(t.text_faint),
                );
            });
            return;
        };
        let total = out.barcodes.len();
        let good = out.barcodes.iter().filter(|b| !matches!(b.status, Status::Kept(_))).count();
        widgets::card(
            ui,
            t,
            "scan-barcode",
            "Barcodes",
            |ui| {
                if total == 0 {
                    widgets::badge(ui, "none found", t.warn, t.warn_soft);
                } else if good == total {
                    widgets::badge(ui, &format!("{good} of {total} verified"), t.ok, t.ok_soft);
                } else {
                    widgets::badge(ui, &format!("{good} of {total} verified"), t.warn, t.warn_soft);
                }
            },
            |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                if total == 0 {
                    ui.label(
                        RichText::new("No barcode could be read. The label is cleaned up, but nothing was redrawn.")
                            .font(theme::body(12.5))
                            .color(t.warn),
                    );
                }
                for b in &out.barcodes {
                    let (ok, detail) = match &b.status {
                        Status::Regenerated { module_px: 0 } => (true, "Redrawn & verified".to_string()),
                        Status::Regenerated { module_px } => (true, format!("Redrawn & verified · {module_px}-dot bars")),
                        Status::Readable => (true, "Reads correctly".to_string()),
                        Status::Kept(why) => (false, format!("Kept original · {why}")),
                    };
                    egui::Frame::new()
                        .fill(if ok { t.surface_alt } else { t.warn_soft })
                        .corner_radius(CornerRadius::same(R_CONTROL))
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                                theme::paint_icon(ui, if ok { "circle-check" } else { "circle-alert" }, r, if ok { t.ok } else { t.warn });
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    ui.label(RichText::new(&b.format).font(theme::semibold(12.5)).color(t.text));
                                    ui.add(
                                        egui::Label::new(RichText::new(&b.text).font(theme::mono(11.5)).color(t.text_muted))
                                            .truncate(),
                                    )
                                    .on_hover_text(&b.text);
                                    ui.label(RichText::new(detail).font(theme::body(11.0)).color(if ok { t.text_faint } else { t.warn }));
                                });
                            });
                        });
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Missing a barcode from this list? It couldn't be read from the original, so it prints as cleaned-up pixels.")
                        .font(theme::body(11.0))
                        .color(t.text_faint),
                );
                if !out.notes.is_empty() {
                    ui.add_space(2.0);
                    widgets::caption(ui, t, "What changed");
                    for n in &out.notes {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("•").color(t.accent));
                            ui.label(RichText::new(n).font(theme::body(11.5)).color(t.text_muted));
                        });
                    }
                }
            },
        );
    }

    // ───────────────────────────── canvas ─────────────────────────────

    fn canvas(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let full = ui.max_rect();
        widgets::dotted_canvas(ui.painter(), full, t);
        if self.src_tex.is_none() {
            self.empty_state(ui, t, full);
            return;
        }
        let pad = 28.0;
        let area = full.shrink(pad);
        let gap = 36.0;
        // Label gets ~58% of the width, original the rest.
        let label_w = (area.width() - gap) * 0.58;
        let orig_rect = Rect::from_min_size(area.min, vec2(area.width() - gap - label_w, area.height()));
        let label_rect = Rect::from_min_size(pos2(orig_rect.right() + gap, area.top()), vec2(label_w, area.height()));
        self.original_sheet(ui, t, orig_rect);
        self.label_sheet(ui, t, label_rect);
        // the little arrow between them
        let mid = pos2(orig_rect.right() + gap / 2.0, area.center().y);
        let c = Rect::from_center_size(mid, Vec2::splat(26.0));
        ui.painter().circle_filled(c.center(), 13.0, t.surface);
        ui.painter().circle_stroke(c.center(), 13.0, Stroke::new(1.0, t.border));
        theme::paint_icon(ui, "sparkles", c.shrink(6.0), t.accent);
    }

    fn sheet_header(&self, ui: &egui::Ui, t: &Tokens, rect: Rect, title: &str, sub: &str) -> Rect {
        let p = ui.painter();
        p.text(rect.min, Align2::LEFT_TOP, title.to_uppercase(), theme::semibold(10.5), t.text_muted);
        p.text(rect.min + vec2(0.0, 15.0), Align2::LEFT_TOP, sub, theme::body(12.0), t.text_faint);
        Rect::from_min_max(rect.min + vec2(0.0, 40.0), rect.max)
    }

    fn original_sheet(&mut self, ui: &mut egui::Ui, t: &Tokens, rect: Rect) {
        let page_info = self.source.as_ref().filter(|s| s.page_count > 1).map(|s| s.page_count);
        let sub = match page_info {
            Some(n) => format!("Page {} of {n} · drag to crop", self.settings.page + 1),
            None => "Drag across it to crop by hand".into(),
        };
        let mut body = self.sheet_header(ui, t, rect, "Original", &sub);
        if let Some(n) = page_info {
            // tiny page stepper under the header
            let bar = Rect::from_min_size(body.min, vec2(body.width(), 30.0));
            body.min.y += 38.0;
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(Layout::left_to_right(Align::Center)));
            let ctx = ui.ctx().clone();
            if widgets::ghost_button(&mut child, t, "rotate-ccw", "Previous", 100.0, self.settings.page > 0).clicked() {
                self.change_page(self.settings.page - 1, &ctx);
            }
            if widgets::ghost_button(&mut child, t, "rotate-cw", "Next", 80.0, self.settings.page + 1 < n).clicked() {
                self.change_page(self.settings.page + 1, &ctx);
            }
        }
        let Some((tex, scale)) = &self.src_tex else { return };
        let tsize = tex.size_vec2();
        let fit = (body.width() / tsize.x).min(body.height() / tsize.y);
        let img = Rect::from_min_size(body.min, tsize * fit);
        let resp = ui.interact(img, ui.scope_id().with("orig"), Sense::drag());
        let p = ui.painter();
        p.add(theme::card_shadow(t).as_shape(img, 4));
        p.image(tex.id(), img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        p.rect_stroke(img, 4, Stroke::new(1.0, t.border), StrokeKind::Outside);

        let k = fit * scale; // screen px per source px
        let to_screen = |x: f32, y: f32| img.min + vec2(x * k, y * k);
        let to_src = |q: Pos2| ((q.x - img.min.x) / k, (q.y - img.min.y) / k);
        let crop = self.settings.manual_crop.or(self.output.as_ref().map(|o| o.crop));
        if let Some(c) = crop {
            let r = Rect::from_min_max(to_screen(c.0 as f32, c.1 as f32), to_screen((c.0 + c.2) as f32, (c.1 + c.3) as f32))
                .intersect(img);
            // dim everything outside the crop
            let dim = Color32::from_black_alpha(70);
            for outside in [
                Rect::from_min_max(img.min, pos2(img.right(), r.top())),
                Rect::from_min_max(pos2(img.left(), r.bottom()), img.max),
                Rect::from_min_max(pos2(img.left(), r.top()), pos2(r.left(), r.bottom())),
                Rect::from_min_max(pos2(r.right(), r.top()), pos2(img.right(), r.bottom())),
            ] {
                if outside.is_positive() {
                    p.rect_filled(outside, 0.0, dim);
                }
            }
            p.rect_stroke(r, 3, Stroke::new(2.0, t.accent), StrokeKind::Outside);
            for corner in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
                p.circle_filled(corner, 4.0, t.accent);
                p.circle_stroke(corner, 4.0, Stroke::new(1.5, Color32::WHITE));
            }
        }
        if resp.drag_started() {
            self.drag_start = resp.interact_pointer_pos();
        }
        if let (Some(a), Some(b)) = (self.drag_start, resp.interact_pointer_pos()) {
            let r = Rect::from_two_pos(a, b).intersect(img);
            p.rect_filled(r, 3, t.accent.gamma_multiply(0.12));
            p.rect_stroke(r, 3, Stroke::new(2.0, t.accent), StrokeKind::Outside);
            if resp.drag_stopped() {
                let (x0, y0) = to_src(r.min);
                let (x1, y1) = to_src(r.max);
                if (x1 - x0) > 40.0 && (y1 - y0) > 40.0 {
                    self.settings.manual_crop = Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32));
                    self.dirty = true;
                }
                self.drag_start = None;
            }
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    fn label_sheet(&mut self, ui: &mut egui::Ui, t: &Tokens, rect: Rect) {
        let sub = format!("{} × {} mm · exactly what the printer gets", self.settings.width_mm, self.settings.height_mm);
        let body = self.sheet_header(ui, t, rect, "Ready to print", &sub);
        let (tw, th) = self.settings.target_size();
        let fit = (body.width() / tw as f32).min(body.height() / th as f32);
        let paper = Rect::from_min_size(body.min, vec2(tw as f32, th as f32) * fit);
        let p = ui.painter().clone();
        p.add(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: t.shadow }.as_shape(paper, 10));
        p.rect_filled(paper, 10, t.paper);
        let tex = if self.comparing { self.before_tex.as_ref().or(self.out_tex.as_ref()) } else { self.out_tex.as_ref() };
        if let Some(tex) = tex {
            p.image(tex.id(), paper.shrink(2.0), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            if self.comparing {
                let tag = Rect::from_min_size(paper.min + vec2(10.0, 10.0), vec2(78.0, 22.0));
                p.rect_filled(tag, R_PILL, t.text);
                p.text(tag.center(), Align2::CENTER_CENTER, "Original", theme::semibold(11.0), t.surface);
            }
            if let (Some(out), false, false) = (&self.output, self.comparing, self.busy) {
                let k = paper.shrink(2.0).width() / tw as f32;
                let to_screen = |(x, y, w, h): (u32, u32, u32, u32)| {
                    Rect::from_min_size(paper.min + vec2(2.0, 2.0) + vec2(x as f32, y as f32) * k, vec2(w as f32, h as f32) * k)
                };
                let mut clicked = None;
                for (i, l) in out.ocr.iter().enumerate() {
                    let r = to_screen(l.region);
                    let edited = self.edits.contains_key(&i);
                    let kept = matches!(l.status, LineStatus::Kept(_)) && !edited;
                    let open = self.editing.as_ref().is_some_and(|e| e.line == i);
                    let resp = ui.interact(r, ui.scope_id().with(("ocr-line", i)), Sense::click());
                    let hover = resp.hovered() && self.editing.is_none() && self.verify.is_none();
                    if hover {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                        let tip = match (l.kind, kept) {
                            (LineKind::Tracking, _) => "Tracking number · click to edit (you'll be asked to double-check)",
                            (LineKind::BarcodeNumber, _) => "Click to edit · its numbers are in a barcode and stay fixed",
                            (_, true) => "Original kept · click to type it",
                            _ => "Click to edit",
                        };
                        resp.clone().on_hover_text(tip);
                    }
                    if resp.clicked() && self.editing.is_none() && self.verify.is_none() {
                        clicked = Some(i);
                    }
                    let stroke = if open || hover {
                        Some(Stroke::new(1.5, t.accent))
                    } else if edited {
                        Some(Stroke::new(1.5, t.accent.gamma_multiply(0.85)))
                    } else if !self.prefs.show_outlines {
                        None
                    } else if kept {
                        Some(Stroke::new(1.0, t.warn))
                    } else {
                        Some(Stroke::new(1.0, t.accent.gamma_multiply(0.45)))
                    };
                    if let Some(st) = stroke {
                        if kept && !(open || hover) {
                            dashed_round_rect(&p, r.expand(1.5), 3.0, st);
                        } else {
                            p.rect_stroke(r.expand(1.5), 3, st, StrokeKind::Outside);
                        }
                    }
                }
                if let Some(i) = clicked {
                    self.editing = Some(Editing { line: i, buf: self.line_text(i), error: None, focus: true });
                }
                if let Some(line) = self.editing.as_ref().map(|e| e.line) {
                    if let Some(l) = self.output.as_ref().and_then(|o| o.ocr.get(line)) {
                        let anchor = to_screen(l.region);
                        self.line_editor(ui.ctx(), t, anchor);
                    }
                }
            }
            if let (true, Some(out)) = (self.prefs.show_outlines, &self.output) {
                let k = paper.shrink(2.0).width() / tw as f32;
                for b in &out.barcodes {
                    let (x, y, w, h) = b.region;
                    let r = Rect::from_min_size(paper.min + vec2(2.0, 2.0) + vec2(x as f32, y as f32) * k, vec2(w as f32, h as f32) * k)
                        .expand(3.0);
                    let c = if matches!(b.status, Status::Kept(_)) { t.warn } else { t.ok };
                    p.rect_filled(r, 5, c.gamma_multiply(0.08));
                    p.rect_stroke(r, 5, Stroke::new(1.5, c), StrokeKind::Outside);
                }
            }
        }
        p.rect_stroke(paper, 10, Stroke::new(1.0, t.border), StrokeKind::Outside);
        if self.busy {
            p.rect_filled(paper, 10, t.surface.gamma_multiply(0.72));
            let c = paper.center();
            let pill = Rect::from_center_size(c, vec2(210.0, 44.0));
            p.add(theme::card_shadow(t).as_shape(pill, R_PILL));
            p.rect_filled(pill, R_PILL, t.surface);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_center_size(pos2(pill.left() + 26.0, c.y), Vec2::splat(18.0))));
            child.add(egui::Spinner::new().size(18.0).color(t.accent));
            p.text(pos2(pill.left() + 44.0, c.y), Align2::LEFT_CENTER, "Crafting your label…", theme::medium(13.0), t.text);
        }
    }

    /// The inline editor floating over a line on the label.
    fn line_editor(&mut self, ctx: &egui::Context, t: &Tokens, anchor: Rect) {
        let Some(ed) = self.editing.as_mut() else { return };
        let i = ed.line;
        let Some(line): Option<OcrLine> = self.output.as_ref().and_then(|o| o.ocr.get(i)).cloned() else {
            self.editing = None;
            return;
        };
        let edited = self.edits.contains_key(&i);
        let (mut keep, mut cancel, mut undo) = (false, false, false);
        let width = anchor.width().max(300.0);
        egui::Area::new(egui::Id::unique("line-editor"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor.left_top() - vec2(10.0, 9.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(t.surface)
                    .stroke(Stroke::new(1.5, t.accent))
                    .corner_radius(CornerRadius::same(R_CONTROL + 2))
                    .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: t.shadow })
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 6.0;
                        let te = egui::TextEdit::singleline(&mut ed.buf)
                            .font(theme::mono(15.0))
                            .desired_width(width)
                            .frame(egui::Frame::new().fill(t.sunken).corner_radius(CornerRadius::same(R_CONTROL)).inner_margin(egui::Margin::symmetric(8, 5)));
                        let r = ui.add(te);
                        if std::mem::take(&mut ed.focus) {
                            r.request_focus();
                        }
                        let (enter, esc) = ui.input(|inp| (inp.key_pressed(egui::Key::Enter), inp.key_pressed(egui::Key::Escape)));
                        keep |= enter && (r.has_focus() || r.lost_focus());
                        cancel |= esc;
                        let note = match line.kind {
                            LineKind::Tracking => Some(("circle-alert", "Tracking number: you'll double-check it before printing")),
                            LineKind::BarcodeNumber => Some(("lock", "Numbers on this line are in a barcode and can't change")),
                            LineKind::Text => None,
                        };
                        if let Some((icon, text)) = note {
                            ui.horizontal(|ui| {
                                ui.add(egui::Image::new(theme::icon(icon)).tint(t.text_muted).fit_to_exact_size(Vec2::splat(13.0)));
                                ui.label(RichText::new(text).font(theme::body(11.5)).color(t.text_muted));
                            });
                        }
                        if let Some(err) = &ed.error {
                            ui.label(RichText::new(err).font(theme::body(11.5)).color(t.danger));
                        }
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("OCR read:").font(theme::body(11.5)).color(t.text_faint));
                            ui.add(egui::Label::new(RichText::new(&line.text).font(theme::mono(11.5)).color(t.text_muted)).truncate());
                        });
                        ui.horizontal(|ui| {
                            for sym in ["™", "®", "©"] {
                                if ui.add(egui::Button::new(RichText::new(sym).font(theme::mono(13.0)).color(t.text)).fill(t.sunken).min_size(vec2(28.0, 26.0))).on_hover_text("Add at the end").clicked() {
                                    ed.buf.push_str(sym);
                                    ed.focus = true;
                                }
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.add(egui::Button::new(RichText::new("Keep").font(theme::semibold(12.5)).color(t.on_accent)).fill(t.accent).min_size(vec2(64.0, 26.0))).clicked() {
                                    keep = true;
                                }
                                if ui.add(egui::Button::new(RichText::new("Cancel").font(theme::medium(12.5)).color(t.text)).fill(t.sunken).min_size(vec2(64.0, 26.0))).clicked() {
                                    cancel = true;
                                }
                                if edited && ui.add(egui::Button::new(RichText::new("Undo edit").font(theme::medium(12.5)).color(t.text_muted)).frame(false)).clicked() {
                                    undo = true;
                                }
                            });
                        });
                    });
            });
        if cancel {
            self.editing = None;
        } else if undo {
            self.editing = None;
            self.undo_edit(ctx, i);
        } else if keep {
            let text = self.editing.as_ref().map(|e| e.buf.clone()).unwrap_or_default();
            match self.commit_edit(ctx, i, &text) {
                Ok(()) => self.editing = None,
                Err(e) => {
                    if let Some(ed) = self.editing.as_mut() {
                        ed.error = Some(e);
                        ed.focus = true;
                    }
                }
            }
        }
    }

    /// Double-check a tracking number: original print, what will be printed, and the barcode.
    fn verify_dialog(&mut self, ctx: &egui::Context, t: &Tokens) {
        let Some(v) = self.verify.as_ref() else { return };
        let i = v.line;
        let Some(line) = self.output.as_ref().and_then(|o| o.ocr.get(i)).cloned() else {
            self.verify = None;
            return;
        };
        let reason = self.verify_reason(i).unwrap_or_else(|| "Make sure it matches the original.".into());
        let now = self.line_text(i);
        let edited = self.edits.contains_key(&i);
        let tex = self.before_tex.clone().or_else(|| self.out_tex.clone());
        let (tw, th) = self.settings.target_size();
        let (mut confirm, mut redraw, mut undo, mut later) = (false, false, false, false);
        egui::Modal::new(egui::Id::unique("verify-tracking"))
            .backdrop_color(Color32::from_black_alpha(90))
            .frame(
                egui::Frame::new()
                    .fill(t.surface)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(CornerRadius::same(R_CARD + 4))
                    .shadow(egui::Shadow { offset: [0, 18], blur: 48, spread: 0, color: t.shadow })
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ctx, |ui| {
                let w = 460.0;
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(theme::icon("circle-alert")).tint(t.warn).fit_to_exact_size(Vec2::splat(20.0)));
                    ui.label(RichText::new("Double-check the tracking number").font(theme::semibold(17.0)).color(t.text));
                });
                ui.label(RichText::new(reason).font(theme::body(12.5)).color(t.text_muted));

                ui.label(RichText::new("ORIGINAL").font(theme::semibold(10.5)).color(t.text_faint));
                if let Some(tex) = &tex {
                    let (x, y, rw, rh) = line.region;
                    let pad = 6.0;
                    let (x0, y0) = ((x as f32 - pad).max(0.0), (y as f32 - pad).max(0.0));
                    let (x1, y1) = ((x as f32 + rw as f32 + pad).min(tw as f32), (y as f32 + rh as f32 + pad).min(th as f32));
                    let aspect = (x1 - x0) / (y1 - y0);
                    let size = vec2(w, (w / aspect).min(110.0));
                    let size = vec2(size.y * aspect, size.y).min(vec2(w, 110.0));
                    let (r, _) = ui.allocate_exact_size(vec2(w, size.y + 12.0), Sense::hover());
                    ui.painter().rect_filled(r, R_CONTROL, Color32::WHITE);
                    ui.painter().rect_stroke(r, R_CONTROL, Stroke::new(1.0, t.border), StrokeKind::Inside);
                    let img = Rect::from_center_size(r.center(), size);
                    let uv = Rect::from_min_max(pos2(x0 / tw as f32, y0 / th as f32), pos2(x1 / tw as f32, y1 / th as f32));
                    ui.painter().image(tex.id(), img, uv, Color32::WHITE);
                }

                ui.label(RichText::new(if matches!(line.status, LineStatus::Kept(_)) && !edited { "WHAT IT SAYS (AS READ)" } else { "WILL PRINT AS" }).font(theme::semibold(10.5)).color(t.text_faint));
                ui.label(RichText::new(&now).font(theme::mono(17.0)).color(t.text));

                if let Some(bd) = &line.barcode_digits {
                    let d = crate::ocr::digits_of(&now);
                    let ok = !d.is_empty() && bd.contains(&d);
                    let (fg, bg, icon, msg) = if ok {
                        (t.ok, t.ok_soft, "circle-check", "Matches the barcode".to_string())
                    } else {
                        (t.danger, t.warn_soft, "triangle-alert", format!("Doesn't match the barcode: {bd}"))
                    };
                    egui::Frame::new().fill(bg).corner_radius(CornerRadius::same(R_CONTROL)).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            ui.add(egui::Image::new(theme::icon(icon)).tint(fg).fit_to_exact_size(Vec2::splat(14.0)));
                            ui.label(RichText::new(msg).font(theme::medium(12.0)).color(fg));
                        });
                    });
                }

                ui.label(RichText::new("FIX IT").font(theme::semibold(10.5)).color(t.text_faint));
                if let Some(v) = self.verify.as_mut() {
                    ui.add(
                        egui::TextEdit::singleline(&mut v.buf)
                            .font(theme::mono(14.0))
                            .desired_width(w)
                            .frame(egui::Frame::new().fill(t.sunken).corner_radius(CornerRadius::same(R_CONTROL)).inner_margin(egui::Margin::symmetric(8, 6))),
                    );
                    if let Some(err) = &v.error {
                        ui.label(RichText::new(err).font(theme::body(11.5)).color(t.danger));
                    }
                }
                let changed = self.verify.as_ref().is_some_and(|v| v.buf.trim() != now);
                let mismatch = line.barcode_digits.as_ref().is_some_and(|bd| {
                    let d = crate::ocr::digits_of(&now);
                    d.is_empty() || !bd.contains(&d)
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Later").font(theme::medium(12.5)).color(t.text_muted)).frame(false)).clicked() {
                        later = true;
                    }
                    if edited && ui.add(egui::Button::new(RichText::new("Undo my change").font(theme::medium(12.5)).color(t.text)).fill(t.sunken).min_size(vec2(0.0, 30.0))).clicked() {
                        undo = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if changed {
                            if ui.add(egui::Button::new(RichText::new("Redraw with my text").font(theme::semibold(12.5)).color(t.on_accent)).fill(t.accent).min_size(vec2(0.0, 30.0))).clicked() {
                                redraw = true;
                            }
                        } else if ui.add(egui::Button::new(RichText::new(if mismatch { "Keep it anyway" } else { "It matches the label" }).font(theme::semibold(12.5)).color(t.on_accent)).fill(t.accent).min_size(vec2(0.0, 30.0))).clicked() {
                            confirm = true;
                        }
                    });
                });
            });
        if later {
            self.verify = None;
        } else if undo {
            self.undo_edit(ctx, i);
            let buf = self.line_text(i);
            self.verify = self.verify_reason(i).map(|_| Verify { line: i, buf, error: None });
        } else if confirm {
            self.verified.insert(i);
            self.verify = self.tracking_pending().map(|line| Verify { line, buf: self.line_text(line), error: None });
            if self.verify.is_none() {
                self.toast(ToastKind::Ok, "Tracking number checked");
            }
        } else if redraw {
            let text = self.verify.as_ref().map(|v| v.buf.clone()).unwrap_or_default();
            if let Err(e) = self.commit_edit(ctx, i, &text) {
                if let Some(v) = self.verify.as_mut() {
                    v.error = Some(e);
                }
            }
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, t: &Tokens, full: Rect) {
        let zone = Rect::from_center_size(full.center(), vec2(full.width().min(560.0) - 40.0, 310.0));
        let resp = ui.interact(zone, ui.scope_id().with("drop"), Sense::click());
        let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
        let p = ui.painter();
        p.add(theme::card_shadow(t).as_shape(zone, R_CARD + 6));
        p.rect_filled(zone, R_CARD + 6, lerp(t.surface, t.surface_alt, hover * 0.6));
        dashed_round_rect(p, zone.shrink(10.0), 16.0, Stroke::new(1.5, lerp(t.border_strong, t.accent, hover)));
        let c = zone.center() - vec2(0.0, 58.0);
        p.circle_filled(c, 34.0, t.accent_soft);
        theme::paint_icon(ui, "upload", Rect::from_center_size(c - vec2(0.0, 2.0 + 3.0 * hover), Vec2::splat(30.0)), t.accent);
        p.text(zone.center() + vec2(0.0, 2.0), Align2::CENTER_CENTER, "Drop a shipping label here", theme::semibold(19.0), t.text);
        p.text(zone.center() + vec2(0.0, 28.0), Align2::CENTER_CENTER, "PDF, PNG or JPG · or click to browse", theme::body(13.0), t.text_muted);
        // three little steps
        let steps = [("1", "Drop it in"), ("2", "Check the barcodes"), ("3", "Print")];
        let y = zone.bottom() - 62.0;
        let widths: Vec<f32> = steps.iter().map(|(_, s)| p.layout_no_wrap(s.to_string(), theme::medium(12.0), t.text).size().x + 36.0).collect();
        let total: f32 = widths.iter().sum::<f32>() + 10.0 * (steps.len() as f32 - 1.0);
        let mut x = zone.center().x - total / 2.0;
        for ((n, s), w) in steps.iter().zip(widths) {
            let r = Rect::from_min_size(pos2(x, y), vec2(w, 28.0));
            p.rect_filled(r, R_PILL, t.sunken);
            p.circle_filled(pos2(r.left() + 14.0, r.center().y), 9.0, t.accent);
            p.text(pos2(r.left() + 14.0, r.center().y), Align2::CENTER_CENTER, *n, theme::semibold(11.0), t.on_accent);
            p.text(pos2(r.left() + 28.0, r.center().y), Align2::LEFT_CENTER, *s, theme::medium(12.0), t.text_muted);
            x += w + 10.0;
        }
        if resp.clicked() {
            let ctx = ui.ctx().clone();
            self.browse(&ctx);
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context, t: &Tokens) {
        if !self.settings_open {
            return;
        }
        const PAGES: [(&str, &str, &str); 7] = [
            ("sun", "Appearance", "Themes, size and motion"),
            ("sticker", "Label", "Size, resolution and alignment"),
            ("crop", "Image", "Cropping, rotation and ink"),
            ("scan-barcode", "Barcodes", "How codes are redrawn"),
            ("sparkles", "Text (OCR)", "Re-typeset text, carefully"),
            ("printer", "Printing", "Printer, copies and archive"),
            ("package", "About", "Version and credits"),
        ];
        let screen = ctx.content_rect();
        let size = vec2(820.0_f32.min(screen.width() - 40.0), 580.0_f32.min(screen.height() - 40.0));
        let before = self.processing_key();
        let before_look = (self.prefs.light_theme.clone(), self.prefs.dark_theme.clone(), self.prefs.theme, self.prefs.ui_scale, self.prefs.reduce_motion);
        let modal = egui::Modal::new(egui::Id::unique("settings"))
            .backdrop_color(Color32::from_black_alpha(90))
            .frame(
                egui::Frame::new()
                    .fill(t.surface)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(CornerRadius::same(R_CARD + 4))
                    .shadow(egui::Shadow { offset: [0, 18], blur: 48, spread: 0, color: t.shadow }),
            )
            .show(ctx, |ui| {
                ui.set_min_size(size);
                ui.set_max_size(size);
                ui.horizontal_top(|ui| {
                    // ── navigation ──
                    let (nav, _) = ui.allocate_exact_size(vec2(218.0, size.y), Sense::hover());
                    ui.painter().rect_filled(
                        nav,
                        CornerRadius { nw: R_CARD + 4, sw: R_CARD + 4, ne: 0, se: 0 },
                        t.surface_alt,
                    );
                    ui.painter().vline(nav.right(), nav.y_range(), Stroke::new(1.0, t.border));
                    let mut nav_ui = ui.new_child(egui::UiBuilder::new().max_rect(nav.shrink2(vec2(12.0, 16.0))).layout(Layout::top_down(Align::LEFT)));
                    nav_ui.label(RichText::new("Settings").font(theme::semibold(18.0)).color(t.text));
                    nav_ui.add_space(10.0);
                    for (i, (icon, title, sub)) in PAGES.iter().enumerate() {
                        let (r, resp) = nav_ui.allocate_exact_size(vec2(nav_ui.available_width(), 46.0), Sense::click());
                        let sel = self.settings_page == i;
                        let hover = nav_ui.ctx().animate_bool(resp.id, resp.hovered());
                        let p = nav_ui.painter();
                        if sel {
                            p.rect_filled(r, R_CONTROL, t.surface);
                            p.rect_stroke(r, R_CONTROL, Stroke::new(1.0, t.border), StrokeKind::Inside);
                        } else if hover > 0.0 {
                            p.rect_filled(r, R_CONTROL, t.sunken.gamma_multiply(hover));
                        }
                        theme::paint_icon(&nav_ui, icon, Rect::from_center_size(pos2(r.left() + 20.0, r.center().y), Vec2::splat(16.0)), if sel { t.accent } else { t.text_muted });
                        p.text(pos2(r.left() + 38.0, r.top() + 15.0), Align2::LEFT_CENTER, *title, theme::medium(13.0), t.text);
                        p.text(pos2(r.left() + 38.0, r.top() + 32.0), Align2::LEFT_CENTER, *sub, theme::body(11.0), t.text_faint);
                        if resp.clicked() {
                            self.settings_page = i;
                        }
                    }
                    // ── content ──
                    ui.vertical(|ui| {
                        ui.set_width(size.x - 218.0 - 8.0);
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            let (_, title, sub) = PAGES[self.settings_page];
                            ui.vertical(|ui| {
                                ui.label(RichText::new(title).font(theme::semibold(20.0)).color(t.text));
                                ui.label(RichText::new(sub).font(theme::body(12.5)).color(t.text_faint));
                            });
                            ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                                ui.add_space(10.0);
                                if widgets::icon_button(ui, t, "x", "Close", 32.0).clicked() {
                                    self.settings_open = false;
                                }
                            });
                        });
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            egui::Frame::new().inner_margin(egui::Margin { left: 14, right: 18, top: 4, bottom: 18 }).show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 12.0;
                                match self.settings_page {
                                    0 => self.page_appearance(ui, t),
                                    1 => self.page_label(ui, t),
                                    2 => self.page_image(ui, t),
                                    3 => self.page_barcodes(ui, t),
                                    4 => self.page_ocr(ui, t),
                                    5 => self.page_printing(ui, t),
                                    _ => self.page_about(ui, t),
                                }
                            });
                        });
                    });
                });
            });
        if modal.should_close() {
            self.settings_open = false;
        }
        self.sync_settings();
        if self.processing_key() != before {
            self.dirty = true;
        }
        let after_look = (self.prefs.light_theme.clone(), self.prefs.dark_theme.clone(), self.prefs.theme, self.prefs.ui_scale, self.prefs.reduce_motion);
        if after_look != before_look {
            self.apply_look(ctx);
        }
    }

    fn page_appearance(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        section(ui, t, "Mode", Some("Follow your system, or keep one look all the time."));
        ui.horizontal(|ui| {
            for (i, (icon, label)) in [("sun", "Light"), ("monitor", "Match system"), ("moon", "Dark")].iter().enumerate() {
                if choice_chip(ui, t, icon, label, self.prefs.theme == i).clicked() {
                    self.prefs.theme = i;
                }
            }
        });
        section(ui, t, "Light theme", Some("Used in light mode."));
        self.theme_grid(ui, t, false);
        section(ui, t, "Dark theme", Some("Used in dark mode. Set the mode to Light or Dark to keep one all the time."));
        self.theme_grid(ui, t, true);
        section(ui, t, "Interface", None);
        setting_row(ui, t, "Interface size", Some("Makes everything bigger or smaller"), |ui| {
            ui.add(egui::Slider::new(&mut self.prefs.ui_scale, 0.8..=1.4).step_by(0.05).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
        });
        widgets::switch(ui, t, &mut self.prefs.reduce_motion, "Reduce motion", Some("Turn off animations"));
        widgets::switch(ui, t, &mut self.prefs.show_outlines, "Outline barcodes and text in the preview", None);
    }

    fn theme_grid(&mut self, ui: &mut egui::Ui, t: &Tokens, dark_slot: bool) {
        let current = if dark_slot { self.prefs.dark_theme.clone() } else { self.prefs.light_theme.clone() };
        let list: Vec<theme::Tokens> = theme::THEMES.iter().copied().filter(|p| p.dark == dark_slot).collect();
        let per_row = ((ui.available_width() + 10.0) / 138.0).floor().max(1.0) as usize;
        for row in list.chunks(per_row) {
            ui.horizontal(|ui| {
                for pal in row {
                    if theme_swatch(ui, t, pal, pal.name == current).clicked() {
                        if dark_slot {
                            self.prefs.dark_theme = pal.name.into();
                        } else {
                            self.prefs.light_theme = pal.name.into();
                        }
                    }
                }
            });
        }
    }

    fn page_label(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        section(ui, t, "Label size", Some("Pick a common size or set your own."));
        const PRESETS: [(&str, f32, f32); 7] = [
            ("100 × 150 mm (4 × 6 in)", 100.0, 150.0),
            ("4 × 6 in exact", 101.6, 152.4),
            ("A6 (105 × 148 mm)", 105.0, 148.0),
            ("4 × 4 in", 101.6, 101.6),
            ("4 × 3 in", 101.6, 76.2),
            ("4 × 2 in", 101.6, 50.8),
            ("2.25 × 1.25 in", 57.15, 31.75),
        ];
        ui.horizontal_wrapped(|ui| {
            for (name, w, h) in PRESETS {
                let on = (self.prefs.width_mm - w).abs() < 0.05 && (self.prefs.height_mm - h).abs() < 0.05;
                if choice_chip(ui, t, "sticker", name, on).clicked() {
                    self.prefs.width_mm = w;
                    self.prefs.height_mm = h;
                }
            }
        });
        setting_row(ui, t, "Width", None, |ui| {
            ui.add(egui::DragValue::new(&mut self.prefs.width_mm).range(20.0..=120.0).speed(0.1).suffix(" mm"));
        });
        setting_row(ui, t, "Height", None, |ui| {
            ui.add(egui::DragValue::new(&mut self.prefs.height_mm).range(20.0..=300.0).speed(0.1).suffix(" mm"));
        });
        section(ui, t, "Printer", None);
        let r = self.resolved_printer();
        let sub = match r.profile {
            Some(p) => format!("Set by the printer model ({}). Change it under Printing.", p.name),
            None => "Custom printer: match your printer's resolution".to_string(),
        };
        setting_row(ui, t, "Resolution", Some(&sub), |ui| {
            if r.profile.is_some() {
                ui.label(RichText::new(format!("{} dpi", r.dpi)).font(theme::mono(12.5)).color(t.text_muted));
            } else {
                egui::ComboBox::from_id_salt("dpi").selected_text(format!("{} dpi", self.prefs.dpi)).show_ui(ui, |ui| {
                    for d in [203.0, 300.0] {
                        ui.selectable_value(&mut self.prefs.dpi, d, format!("{d} dpi"));
                    }
                });
            }
        });
        if let Some(p) = r.profile.filter(|p| self.prefs.width_mm > p.head_mm) {
            note(ui, t, &format!("This printer prints at most {} mm wide, so the label is fitted to that width.", p.head_mm));
        }
        setting_row(ui, t, "Edge margin", Some("Blank border around the content. Smaller = bigger, sharper print; 0 can clip edges if labels feed slightly off"), |ui| {
            ui.add(egui::DragValue::new(&mut self.prefs.margin_mm).range(0.0..=8.0).speed(0.1).suffix(" mm"));
        });
        section(ui, t, "Alignment", Some("If prints come out shifted, nudge them back. + moves right / down."));
        setting_row(ui, t, "Horizontal", None, |ui| {
            ui.add(egui::DragValue::new(&mut self.prefs.offset_x_mm).range(-10.0..=10.0).speed(0.1).suffix(" mm"));
        });
        setting_row(ui, t, "Vertical", None, |ui| {
            ui.add(egui::DragValue::new(&mut self.prefs.offset_y_mm).range(-10.0..=10.0).speed(0.1).suffix(" mm"));
        });
        section(ui, t, "Calibration page", Some("A test label with a border on the outermost dots, millimetre rulers and print patterns. Print one, then adjust the offsets until every edge shows."));
        ui.horizontal(|ui| {
            if widgets::ghost_button(ui, t, "ruler", "Print calibration page", 210.0, true).clicked() {
                self.print_calibration(false);
            }
            if widgets::ghost_button(ui, t, "download", "Save as PNG", 130.0, true).clicked() {
                self.print_calibration(true);
            }
        });
    }

    fn page_image(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        section(ui, t, "Finding the label", None);
        widgets::switch(ui, t, &mut self.prefs.auto_crop, "Crop automatically", Some("Find the label on the page; off uses the whole page"));
        widgets::switch(ui, t, &mut self.prefs.straighten, "Straighten tilted labels", Some("Fixes small tilts in scans and photos"));
        widgets::switch(ui, t, &mut self.prefs.auto_rotate, "Rotate automatically", Some("Turns the label so its main barcode reads left-to-right"));
        section(ui, t, "Quality", None);
        widgets::switch(ui, t, &mut self.prefs.vector_render, "Draw PDFs at printer resolution", Some("Sharpest text from PDFs: drawn straight onto the 203 dpi grid"));
        widgets::switch(ui, t, &mut self.prefs.bolder, "Bolder text", Some("Thickens thin strokes by one dot, for faint prints"));
        setting_row(ui, t, "Darkness", Some("Also on the Adjust card"), |ui| {
            ui.add(egui::Slider::new(&mut self.prefs.darkness, -20..=20));
        });
    }

    fn page_barcodes(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        widgets::switch(ui, t, &mut self.prefs.regenerate, "Regenerate barcodes", Some("Redraw each readable code, then re-scan it to prove it's identical"));
        widgets::switch(ui, t, &mut self.prefs.thicker_bars, "Prefer thicker bars", Some("Rounds bar widths up: a little wider, easier to scan"));
        note(ui, t, "A barcode that can't be read, or whose redrawn version doesn't scan back to the exact same data, always keeps its original pixels.");
    }

    fn page_ocr(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        widgets::switch(ui, t, &mut self.prefs.ocr, "Re-typeset text", Some("Reads each line and redraws it in a crisp font. Off by default."));
        note(
            ui,
            t,
            "A line is only redrawn when two separate reads agree exactly, both are confident, any long number matches a barcode, and the new text reads back the same. Everything else keeps its original pixels. For PDFs, \"Draw PDFs at printer resolution\" already gives exact text without OCR.",
        );
        ui.add_enabled_ui(self.prefs.ocr, |ui| {
            setting_row(ui, t, "Confidence needed", Some("Higher means fewer lines redrawn, fewer risks"), |ui| {
                ui.add(egui::Slider::new(&mut self.prefs.ocr_confidence, 0.8..=0.99).step_by(0.01).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
            });
            widgets::switch(ui, t, &mut self.prefs.ocr_review, "Ask me to check the text before printing", Some("Strongly recommended"));
        });
    }

    fn page_printing(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        setting_row(ui, t, "Printer", None, |ui| {
            egui::ComboBox::from_id_salt("printer-settings")
                .width(240.0)
                .selected_text(if self.prefs.printer.is_empty() { "Choose a printer" } else { &self.prefs.printer })
                .show_ui(ui, |ui| {
                    for p in &self.printers {
                        ui.selectable_value(&mut self.prefs.printer, p.clone(), p);
                    }
                });
        });
        if ui.button("Refresh printer list").clicked() {
            self.printers = print::list_printers();
        }
        section(ui, t, "Printer model", Some("Sets resolution and print width so barcodes land on the printer's real dots."));
        let r = self.resolved_printer();
        let label_for = |id: &str| -> String {
            match id {
                "auto" => match printers::detect(&self.prefs.printer) {
                    Some(p) => format!("Automatic: {}", p.name),
                    None => "Automatic: not recognised (custom)".into(),
                },
                "custom" => "Custom (set resolution under Label)".into(),
                id => printers::by_id(id).map_or(id.to_string(), |p| p.name.to_string()),
            }
        };
        setting_row(ui, t, "Model", None, |ui| {
            egui::ComboBox::from_id_salt("printer-model").width(280.0).selected_text(label_for(&self.prefs.printer_profile)).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.prefs.printer_profile, "auto".to_string(), label_for("auto"));
                for p in printers::PROFILES {
                    ui.selectable_value(&mut self.prefs.printer_profile, p.id.to_string(), p.name);
                }
                ui.selectable_value(&mut self.prefs.printer_profile, "custom".to_string(), label_for("custom"));
            });
        });
        if let Some(p) = r.profile {
            let lang = p.raw.map_or("driver only".to_string(), |l| format!("speaks {}", l.name()));
            let tested = if p.tested { "tested on hardware" } else { "from manufacturer specs, not yet tested" };
            note(ui, t, &format!("{} dpi · prints up to {} mm wide · {lang} · {tested}", r.dpi, p.head_mm));
        }

        section(ui, t, "Direct printing (experimental)", Some("Send the label in the printer's own language instead of through its driver. Avoids driver scaling and margins."));
        let choices = [("off", "Off (use the driver)"), ("auto", "Automatic"), ("zpl", "ZPL"), ("tspl", "TSPL")];
        let name_of = |id: &str| choices.iter().find(|c| c.0 == id).map_or("Off", |c| c.1);
        setting_row(ui, t, "Language", None, |ui| {
            egui::ComboBox::from_id_salt("raw-mode").width(200.0).selected_text(name_of(&self.prefs.raw_mode)).show_ui(ui, |ui| {
                for (id, name) in choices {
                    ui.selectable_value(&mut self.prefs.raw_mode, id.to_string(), name);
                }
            });
        });
        match (self.prefs.raw_mode.as_str(), self.raw_lang()) {
            ("off", _) => {}
            ("auto", None) => note(ui, t, "This printer model has no known language, so the driver is used."),
            (_, Some(l)) => note(ui, t, &format!("Printing in {}. Untested on most printers: if the result is blank, inverted or upside down, switch back to Off and tell us your model.", l.name())),
            _ => {}
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Save a test job:").font(theme::body(12.0)).color(t.text_muted));
            if ui.button("ZPL…").clicked() {
                self.save_raw(RawLang::Zpl);
            }
            if ui.button("TSPL…").clicked() {
                self.save_raw(RawLang::Tspl);
            }
        });
        section(ui, t, "Driver capture", Some("Runs one print through the printer's driver and saves the bytes it would send, without printing. Used to teach LabelCraft a printer's own commands."));
        if ui.button("Capture what the driver sends…").clicked() {
            self.capture_driver();
        }
        if self.driver_uneven {
            note(ui, t, "This driver prints at different horizontal and vertical resolutions (PeriPage calls these \"HD\"). Barcodes are sharpest with the \"SD\" (203 × 203 dpi) driver.");
        }
        setting_row(ui, t, "Copies", None, |ui| {
            ui.add(egui::DragValue::new(&mut self.prefs.copies).range(1..=20));
        });
        widgets::switch(ui, t, &mut self.prefs.auto_print, "Print as soon as a label is ready", Some("Only when every barcode verified and no text needs checking"));
        section(ui, t, "Watch a folder", Some("Open new shipping labels as soon as they land, for example straight from the browser's downloads."));
        widgets::switch(ui, t, &mut self.prefs.watch, "Open new labels from a folder automatically", Some("PDF, PNG and JPG files that appear after LabelCraft starts. Its own saved files are ignored."));
        ui.add_enabled_ui(self.prefs.watch, |ui| {
            ui.horizontal(|ui| {
                let shown = if self.prefs.watch_dir.is_empty() {
                    format!("{} (default)", extras::default_watch_dir().display())
                } else {
                    self.prefs.watch_dir.clone()
                };
                ui.add(egui::Label::new(RichText::new(shown).font(theme::mono(11.5)).color(t.text_muted)).truncate());
                if ui.button("Choose folder…").clicked()
                    && let Some(d) = rfd::FileDialog::new().pick_folder()
                {
                    self.prefs.watch_dir = d.display().to_string();
                }
            });
        });
        if self.prefs.watch && self.prefs.auto_print {
            note(ui, t, "With \"Print as soon as a label is ready\" on, a downloaded label prints by itself once every barcode is verified.");
        }
        section(ui, t, "Several labels in one file", None);
        widgets::switch(ui, t, &mut self.prefs.print_slips, "Include packing slips", Some("When a page has a packing slip beside the label, list it as an extra 4 × 6 to print"));
        section(ui, t, "Keep a copy", None);
        widgets::switch(ui, t, &mut self.prefs.archive, "Save every printed label as a PNG", None);
        ui.add_enabled_ui(self.prefs.archive, |ui| {
            ui.horizontal(|ui| {
                let shown = if self.prefs.archive_dir.is_empty() { "No folder chosen".to_string() } else { self.prefs.archive_dir.clone() };
                ui.add(egui::Label::new(RichText::new(shown).font(theme::mono(11.5)).color(t.text_muted)).truncate());
                if ui.button("Choose folder…").clicked()
                    && let Some(d) = rfd::FileDialog::new().pick_folder()
                {
                    self.prefs.archive_dir = d.display().to_string();
                }
            });
            setting_row(ui, t, "Delete copies after", Some("They contain names and addresses. 0 keeps them forever."), |ui| {
                ui.add(egui::DragValue::new(&mut self.prefs.archive_keep_days).range(0..=365).suffix(" days"));
            });
        });
        if !cfg!(windows) {
            section(ui, t, "CUPS", None);
            setting_row(ui, t, "Extra options", Some("Passed to lp as -o, space-separated"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.prefs.cups_extra).hint_text("media=w288h432").desired_width(220.0));
            });
        }
    }

    fn page_about(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
            widgets::logo(ui.painter(), r, t);
            ui.vertical(|ui| {
                ui.label(RichText::new("LabelCraft").font(theme::semibold(18.0)).color(t.text));
                ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).font(theme::mono(12.0)).color(t.text_muted));
            });
        });
        ui.hyperlink_to("github.com/civarson/labelcraft", "https://github.com/civarson/labelcraft");
        section(ui, t, "Updates", None);
        widgets::switch(ui, t, &mut self.prefs.check_updates, "Tell me when a new version is out", Some("Asks GitHub once at start-up. Nothing about you is sent."));
        ui.horizontal(|ui| {
            if ui.button("Check now").clicked() {
                self.prefs.skipped_version.clear();
                self.check_updates();
                self.toast(ToastKind::Ok, "Checking GitHub… a button appears at the top if there's a newer version");
            }
            if let Some((v, _)) = &self.update {
                ui.label(RichText::new(format!("Version {v} is available")).font(theme::medium(12.0)).color(t.accent));
            }
        });
        if cfg!(windows) {
            section(ui, t, "Open with", Some("Adds LabelCraft to Explorer's \"Open with\" menu for PDF, PNG and JPG files (just for you, no admin rights)."));
            ui.horizontal(|ui| {
                if ui.button("Add to \"Open with\"").clicked() {
                    match extras::register_open_with(true) {
                        Ok(()) => self.toast(ToastKind::Ok, "Added. Right-click a label PDF → Open with → LabelCraft"),
                        Err(e) => self.toast(ToastKind::Error, format!("Couldn't add: {e}")),
                    }
                }
                if ui.button("Remove").clicked() {
                    let _ = extras::register_open_with(false);
                    self.toast(ToastKind::Ok, "Removed from \"Open with\"");
                }
            });
            note(ui, t, "If you move LabelCraft.exe, add it again so Explorer finds it.");
        }
        note(
            ui,
            t,
            "An independent hobby project, not affiliated with ArtCraft, PhotoCraft, SolidCraft or the other Crafting Apps. Its design is a fan's take on the principles they share.",
        );
        section(ui, t, "Built with", None);
        for line in [
            "Barcodes: zxing-cpp and zint",
            "PDF rendering: hayro",
            "Text recognition: PaddleOCR PP-OCRv6 tiny (Apache-2.0), run with rten",
            "Fonts: Inter, JetBrains Mono, Liberation Sans (SIL OFL)",
            "Icons: Lucide (ISC)",
            "Interface: egui",
        ] {
            ui.label(RichText::new(format!("•  {line}")).font(theme::body(12.5)).color(t.text_muted));
        }
        ui.add_space(8.0);
        if widgets::ghost_button(ui, t, "refresh-cw", "Reset all settings", 180.0, true).clicked() {
            let (printer, recent) = (self.prefs.printer.clone(), self.prefs.recent.clone());
            self.prefs = Prefs { printer, recent, ..Prefs::default() };
            self.toast(ToastKind::Ok, "Settings reset");
        }
    }

    fn toasts(&mut self, ctx: &egui::Context, t: &Tokens, area: Rect) {
        let Some(toast) = &self.toast else { return };
        let age = toast.at.elapsed().as_secs_f32();
        let life = if toast.kind == ToastKind::Error { 6.0 } else { 3.0 };
        if age > life {
            self.toast = None;
            return;
        }
        let alpha = (age / 0.18).min(1.0).min((life - age) / 0.35).clamp(0.0, 1.0);
        let lift = (1.0 - (age / 0.25).min(1.0)) * 12.0;
        let (icon, color) = match toast.kind {
            ToastKind::Ok => ("circle-check", t.ok),
            ToastKind::Warn => ("circle-alert", t.warn),
            ToastKind::Error => ("triangle-alert", t.danger),
        };
        let text = toast.text.clone();
        egui::Area::new(egui::Id::unique("toast"))
            .order(egui::Order::Foreground)
            .fixed_pos(area.min)
            .interactable(false)
            .show(ctx, |ui| {
                let painter = ui.painter().clone();
                let galley = painter.layout(text, theme::medium(13.0), t.text, 420.0);
                let size = vec2(galley.size().x + 58.0, galley.size().y.max(18.0) + 22.0);
                let r = Rect::from_center_size(pos2(area.center().x, area.bottom() - 40.0 - size.y / 2.0 + lift), size);
                ui.allocate_rect(r, Sense::hover());
                painter.add(egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: t.shadow.gamma_multiply(alpha) }.as_shape(r, R_PILL));
                painter.rect(r, R_PILL, t.surface.gamma_multiply(alpha), Stroke::new(1.0, t.border.gamma_multiply(alpha)), StrokeKind::Inside);
                painter.circle_filled(pos2(r.left() + 22.0, r.center().y), 11.0, color.gamma_multiply(0.15 * alpha));
                theme::paint_icon(ui, icon, Rect::from_center_size(pos2(r.left() + 22.0, r.center().y), Vec2::splat(16.0)), color.gamma_multiply(alpha));
                painter.galley(pos2(r.left() + 42.0, r.center().y - galley.size().y / 2.0), galley, t.text.gamma_multiply(alpha));
            });
        ctx.request_repaint();
    }
}

fn section(ui: &mut egui::Ui, t: &Tokens, title: &str, sub: Option<&str>) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).font(theme::semibold(14.0)).color(t.text));
    if let Some(s) = sub {
        ui.add_space(-8.0);
        ui.label(RichText::new(s).font(theme::body(12.0)).color(t.text_faint));
    }
}

fn note(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    egui::Frame::new()
        .fill(t.surface_alt)
        .corner_radius(CornerRadius::same(R_CONTROL))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).font(theme::body(12.0)).color(t.text_muted));
        });
}

fn setting_row(ui: &mut egui::Ui, t: &Tokens, label: &str, sub: Option<&str>, control: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new(label).font(theme::medium(13.0)).color(t.text));
            if let Some(s) = sub {
                ui.label(RichText::new(s).font(theme::body(11.5)).color(t.text_faint));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
}

fn choice_chip(ui: &mut egui::Ui, t: &Tokens, icon: &str, label: &str, on: bool) -> egui::Response {
    let g = ui.painter().layout_no_wrap(label.to_string(), theme::medium(12.5), t.text);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 44.0, 32.0), Sense::click());
    let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
    let p = ui.painter();
    let (fill, stroke) = if on { (t.accent_soft, t.accent) } else { (lerp(t.surface, t.surface_alt, hover), t.border) };
    p.rect(r, R_PILL, fill, Stroke::new(if on { 1.5 } else { 1.0 }, stroke), StrokeKind::Inside);
    theme::paint_icon(ui, icon, Rect::from_center_size(pos2(r.left() + 18.0, r.center().y), Vec2::splat(14.0)), if on { t.accent } else { t.text_muted });
    p.galley(pos2(r.left() + 32.0, r.center().y - g.size().y / 2.0), g, if on { t.accent } else { t.text });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A mini window rendered in the palette's own colours.
fn theme_swatch(ui: &mut egui::Ui, t: &Tokens, pal: &theme::Tokens, on: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(128.0, 92.0), Sense::click());
    let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
    let p = ui.painter();
    let card = Rect::from_min_size(r.min, vec2(r.width(), 66.0));
    p.rect(card, R_CONTROL, pal.canvas, Stroke::new(if on { 2.0 } else { 1.0 }, if on { t.accent } else { lerp(t.border, t.border_strong, hover) }), StrokeKind::Inside);
    let inner = card.shrink(8.0);
    // a little "app": sidebar card, label sheet, accent button
    let side = Rect::from_min_size(pos2(inner.right() - 44.0, inner.top()), vec2(44.0, inner.height()));
    p.rect_filled(side, 4, pal.surface);
    p.rect_filled(Rect::from_min_size(side.min + vec2(5.0, 6.0), vec2(34.0, 9.0)), 3, pal.accent);
    p.rect_filled(Rect::from_min_size(side.min + vec2(5.0, 20.0), vec2(26.0, 4.0)), 2, pal.text_muted);
    p.rect_filled(Rect::from_min_size(side.min + vec2(5.0, 28.0), vec2(20.0, 4.0)), 2, pal.text_faint);
    p.circle_filled(side.min + vec2(34.0, 40.0), 3.5, pal.ok);
    let sheet = Rect::from_min_size(inner.min + vec2(8.0, 0.0), vec2(30.0, inner.height()));
    p.rect_filled(sheet, 3, pal.paper);
    for i in 0..4 {
        p.hline(sheet.left() + 4.0..=sheet.right() - 4.0 - (i % 2) as f32 * 8.0, sheet.top() + 7.0 + i as f32 * 8.0, Stroke::new(1.5, Color32::from_gray(60)));
    }
    if on {
        let c = pos2(card.left() + 12.0, card.top() + 12.0);
        p.circle_filled(c, 8.0, t.accent);
        theme::paint_icon(ui, "check", Rect::from_center_size(c, Vec2::splat(11.0)), t.on_accent);
    }
    p.text(pos2(r.left() + 2.0, card.bottom() + 13.0), Align2::LEFT_CENTER, pal.name, theme::medium(12.0), if on { t.accent } else { t.text });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn apply_theme(ctx: &egui::Context, choice: usize) {
    ctx.set_theme(match choice {
        0 => egui::ThemePreference::Light,
        2 => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    });
}

fn dashed_round_rect(p: &egui::Painter, r: Rect, radius: f32, stroke: Stroke) {
    // Straight edges dashed, corners as short arcs.
    let dash = 7.0;
    let gap = 6.0;
    let edges = [
        (pos2(r.left() + radius, r.top()), pos2(r.right() - radius, r.top())),
        (pos2(r.right(), r.top() + radius), pos2(r.right(), r.bottom() - radius)),
        (pos2(r.right() - radius, r.bottom()), pos2(r.left() + radius, r.bottom())),
        (pos2(r.left(), r.bottom() - radius), pos2(r.left(), r.top() + radius)),
    ];
    for (a, b) in edges {
        p.extend(egui::Shape::dashed_line(&[a, b], stroke, dash, gap));
    }
    let corners = [
        (pos2(r.left() + radius, r.top() + radius), std::f32::consts::PI),
        (pos2(r.right() - radius, r.top() + radius), -std::f32::consts::FRAC_PI_2),
        (pos2(r.right() - radius, r.bottom() - radius), 0.0),
        (pos2(r.left() + radius, r.bottom() - radius), std::f32::consts::FRAC_PI_2),
    ];
    for (c, start) in corners {
        let pts: Vec<Pos2> = (0..=8)
            .map(|i| {
                let a = start + std::f32::consts::FRAC_PI_2 * i as f32 / 8.0;
                c + vec2(a.cos(), a.sin()) * radius
            })
            .collect();
        p.add(egui::Shape::line(pts, stroke));
    }
}

fn gray_to_color(g: &GrayImage) -> ColorImage {
    ColorImage::from_gray([g.width() as usize, g.height() as usize], g.as_raw())
}

/// Texture for the source preview, capped so it fits any GPU. Returns (texture, scale).
fn preview_texture(ctx: &egui::Context, g: &GrayImage) -> (TextureHandle, f32) {
    let long = g.width().max(g.height()) as f32;
    let scale = (2048.0 / long).min(1.0);
    let img = if scale < 1.0 {
        image::imageops::resize(g, (g.width() as f32 * scale) as u32, (g.height() as f32 * scale) as u32, image::imageops::FilterType::Triangle)
    } else {
        g.clone()
    };
    (ctx.load_texture("source", gray_to_color(&img), TextureOptions::LINEAR), scale)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get(&ctx);
        self.poll(&ctx);

        // Drag and drop.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if let Some(p) = dropped.into_iter().next() {
            self.open(p, &ctx);
        }
        // Prefs that feed processing.
        let before = self.processing_key();
        self.sync_settings();
        if before != self.processing_key() {
            self.dirty = true;
        }

        let bar = |fill: Color32| egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(12, 0));
        egui::Panel::top("top").exact_size(58.0).frame(bar(t.surface)).show(ui, |ui| self.top_bar(ui, &t));
        egui::Panel::bottom("status").exact_size(28.0).frame(bar(t.surface)).show(ui, |ui| self.status_bar(ui, &t));
        egui::Panel::right("inspector")
            .resizable(false)
            .exact_size(340.0)
            .frame(egui::Frame::new().fill(t.canvas).inner_margin(egui::Margin { left: 4, right: 16, top: 0, bottom: 0 }))
            .show(ui, |ui| self.inspector(ui, &t));
        let mut canvas_rect = Rect::NOTHING;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.canvas)).show(ui, |ui| {
            canvas_rect = ui.max_rect();
            self.canvas(ui, &t);
        });
        // Hairlines between chrome and canvas.
        let lp = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::unique("hairlines")));
        let screen = ctx.content_rect();
        lp.hline(screen.x_range(), screen.top() + 58.0, Stroke::new(1.0, t.border));
        lp.hline(screen.x_range(), screen.bottom() - 28.0, Stroke::new(1.0, t.border));

        self.settings_window(&ctx, &t);
        self.verify_dialog(&ctx, &t);

        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            egui::Area::new(egui::Id::unique("drop_overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(screen.min)
                .interactable(false)
                .show(&ctx, |ui| {
                    ui.allocate_rect(screen, Sense::hover());
                    let p = ui.painter().clone();
                    p.rect_filled(screen, 0.0, t.canvas.gamma_multiply(0.88));
                    let zone = screen.shrink(28.0);
                    dashed_round_rect(&p, zone, 22.0, Stroke::new(2.5, t.accent));
                    p.circle_filled(zone.center() - vec2(0.0, 30.0), 38.0, t.accent_soft);
                    theme::paint_icon(ui, "upload", Rect::from_center_size(zone.center() - vec2(0.0, 30.0), Vec2::splat(34.0)), t.accent);
                    p.text(zone.center() + vec2(0.0, 34.0), Align2::CENTER_CENTER, "Drop to craft your label", theme::semibold(22.0), t.text);
                });
        }
        if canvas_rect.is_positive() {
            self.toasts(&ctx, &t, canvas_rect);
        }

        // Shortcuts.
        let (open, print_now, prefs_key) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::O) && i.modifiers.command,
                i.key_pressed(egui::Key::P) && i.modifiers.command,
                i.key_pressed(egui::Key::Comma) && i.modifiers.command,
            )
        });
        self.sync_watcher();
        // Paste a label (screenshot or copied file), unless a text box has the keyboard.
        let paste = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                || (i.key_pressed(egui::Key::V) && i.modifiers.command)
        });
        if paste && !ctx.memory(|m| m.focused().is_some()) && !self.settings_open && self.editing.is_none() && self.verify.is_none() {
            self.paste(&ctx);
        }
        // Undo/redo, unless a text box has the keyboard (it has its own undo).
        if !ctx.memory(|m| m.focused().is_some()) && !self.settings_open {
            let (undo, redo) = ctx.input(|i| {
                let z = i.key_pressed(egui::Key::Z) && i.modifiers.command;
                (z && !i.modifiers.shift, (z && i.modifiers.shift) || (i.key_pressed(egui::Key::Y) && i.modifiers.command))
            });
            if undo {
                self.undo(&ctx);
            } else if redo {
                self.redo(&ctx);
            }
        }
        self.record_history(&ctx);
        if prefs_key {
            self.settings_open = !self.settings_open;
        }
        if open {
            self.browse(&ctx);
        }
        if print_now && self.output.is_some() && !self.busy {
            self.do_print();
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "prefs", &self.prefs);
    }
}

