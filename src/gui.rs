//! The LabelCraft window: a dotted craft-mat canvas with the original and the finished
//! label as paper sheets, and an inspector of cards on the right.

use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, CornerRadius, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Vec2, pos2, vec2,
};
use image::GrayImage;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Instant;

use crate::barcodes::Status;
use crate::pipeline::{Output, Settings, Source, SourcePage};
use crate::theme::{self, R_CARD, R_CONTROL, R_PILL, Tokens};
use crate::widgets::{self, lerp};
use crate::{pdfout, print};

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
}

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
        }
    }
}

enum Msg {
    Loaded { generation: u64, source: Arc<Source>, page: Arc<SourcePage> },
    Processed { generation: u64, output: Box<Output> },
    Failed { generation: u64, error: String },
}

#[derive(Clone, Copy, PartialEq)]
enum ToastKind {
    Ok,
    Warn,
    Error,
}

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
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::install(&cc.egui_ctx);
        let prefs: Prefs = cc.storage.and_then(|s| eframe::get_value(s, "prefs")).unwrap_or_default();
        apply_theme(&cc.egui_ctx, prefs.theme);
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
        };
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
        self.settings.dpi = p.dpi;
        self.settings.width_mm = p.width_mm;
        self.settings.height_mm = p.height_mm;
        self.settings.margin_mm = p.margin_mm;
        self.settings.darkness = p.darkness;
        self.settings.regenerate_barcodes = p.regenerate;
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
                    self.output = Some(*output);
                    self.busy = false;
                }
                Msg::Failed { generation, error } if generation == self.generation => {
                    self.busy = false;
                    self.toast(ToastKind::Error, error);
                }
                _ => {} // stale
            }
        }
        if self.dirty && !self.busy {
            self.reprocess(ctx);
        }
    }

    fn do_print(&mut self) {
        let Some(out) = &self.output else { return };
        if self.prefs.printer.is_empty() {
            self.toast(ToastKind::Warn, "Pick a printer first");
            return;
        }
        match print::print(&self.prefs.printer, &out.bitmap, &self.settings, &self.prefs.cups_extra) {
            Ok(()) => {
                let p = self.prefs.printer.clone();
                self.toast(ToastKind::Ok, format!("Sent to {p}"));
            }
            Err(e) => self.toast(ToastKind::Error, format!("Couldn't print: {e:#}")),
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
                if widgets::icon_button(ui, t, "settings-2", "Label & printer settings", 34.0).clicked() {
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
                if widgets::primary_button(ui, t, "printer", "Print label", Some(&format!("{MOD}P")), ready).clicked() {
                    self.do_print();
                }
                ui.horizontal(|ui| {
                    let half = (ui.available_width() - 8.0) / 2.0;
                    if widgets::ghost_button(ui, t, "download", "Save PNG", half, ready).clicked() {
                        self.save(false);
                    }
                    if widgets::ghost_button(ui, t, "file-down", "Save PDF", half, ready).clicked() {
                        self.save(true);
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
            ui.add_space(8.0);
        });
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

    fn label_sheet(&self, ui: &mut egui::Ui, t: &Tokens, rect: Rect) {
        let sub = format!("{} × {} mm · exactly what the printer gets", self.settings.width_mm, self.settings.height_mm);
        let body = self.sheet_header(ui, t, rect, "Ready to print", &sub);
        let (tw, th) = self.settings.target_size();
        let fit = (body.width() / tw as f32).min(body.height() / th as f32);
        let paper = Rect::from_min_size(body.min, vec2(tw as f32, th as f32) * fit);
        let p = ui.painter().clone();
        p.add(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: t.shadow }.as_shape(paper, 10));
        p.rect_filled(paper, 10, t.paper);
        if let Some(tex) = &self.out_tex {
            p.image(tex.id(), paper.shrink(2.0), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
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
        let mut open = self.settings_open;
        egui::Window::new("Label & printer")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::RIGHT_TOP, vec2(-16.0, 66.0))
            .default_width(300.0)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                let mut changed = false;
                egui::Grid::new("dims").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                    ui.label("Label width");
                    changed |= ui.add(egui::DragValue::new(&mut self.prefs.width_mm).range(40.0..=120.0).suffix(" mm")).changed();
                    ui.end_row();
                    ui.label("Label height");
                    changed |= ui.add(egui::DragValue::new(&mut self.prefs.height_mm).range(40.0..=300.0).suffix(" mm")).changed();
                    ui.end_row();
                    ui.label("Edge margin");
                    changed |= ui.add(egui::DragValue::new(&mut self.prefs.margin_mm).range(0.0..=8.0).speed(0.1).suffix(" mm")).changed();
                    ui.end_row();
                    ui.label("Printer resolution");
                    egui::ComboBox::from_id_salt("dpi").selected_text(format!("{} dpi", self.prefs.dpi)).show_ui(ui, |ui| {
                        for d in [203.0, 300.0] {
                            changed |= ui.selectable_value(&mut self.prefs.dpi, d, format!("{d} dpi")).changed();
                        }
                    });
                    ui.end_row();
                });
                if !cfg!(windows) {
                    widgets::caption(ui, t, "Extra CUPS options");
                    ui.add(egui::TextEdit::singleline(&mut self.prefs.cups_extra).hint_text("e.g. media=w288h432").desired_width(f32::INFINITY));
                }
                ui.label(
                    RichText::new("Built for a PeriPage PB41: 100 × 150 mm at 203 dpi.")
                        .font(theme::body(11.5))
                        .color(t.text_faint),
                );
                if changed {
                    self.dirty = true;
                }
            });
        self.settings_open = open;
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
        let before = (self.settings.darkness, self.settings.regenerate_barcodes, self.settings.dpi);
        self.sync_settings();
        if before != (self.settings.darkness, self.settings.regenerate_barcodes, self.settings.dpi) {
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
        let (open, print_now) = ctx.input(|i| (i.key_pressed(egui::Key::O) && i.modifiers.command, i.key_pressed(egui::Key::P) && i.modifiers.command));
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

