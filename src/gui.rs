//! Drag-and-drop window: original on the left, print-ready label on the right.

use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};
use image::GrayImage;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use crate::barcodes::Status;
use crate::pipeline::{Output, Settings, Source, SourcePage};
use crate::{pdfout, print};

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
    cups_extra: String,
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
            cups_extra: String::new(),
        }
    }
}

enum Msg {
    Loaded { generation: u64, source: Arc<Source>, page: Arc<SourcePage> },
    Processed { generation: u64, output: Box<Output> },
    Failed { generation: u64, error: String },
}

pub struct App {
    prefs: Prefs,
    settings: Settings,
    source: Option<Arc<Source>>,
    page: Option<Arc<SourcePage>>,
    src_tex: Option<(TextureHandle, f32)>, // texture, preview px per source px
    output: Option<Output>,
    out_tex: Option<TextureHandle>,
    printers: Vec<String>,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    generation: u64,
    busy: bool,
    dirty: bool,
    drag_start: Option<Pos2>,
    status: Option<(String, bool)>, // message, is_error
    show_boxes: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let prefs: Prefs = cc.storage.and_then(|s| eframe::get_value(s, "prefs")).unwrap_or_default();
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
            status: None,
            show_boxes: true,
        };
        if app.prefs.printer.is_empty() || !app.printers.contains(&app.prefs.printer) {
            // Prefer anything that looks like the PeriPage, else the system default.
            app.prefs.printer = app
                .printers
                .iter()
                .find(|p| p.to_lowercase().contains("peripage") || p.to_lowercase().contains("pb41"))
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

    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.generation += 1;
        let generation = self.generation;
        self.busy = true;
        self.status = Some((format!("Opening {}…", path.display()), false));
        self.settings.manual_crop = None;
        self.settings.extra_rotation = 0;
        self.settings.page = 0;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match Source::open(&path).and_then(|s| s.render_page(0).map(|p| (s, p))) {
                Ok((s, p)) => Msg::Loaded { generation, source: Arc::new(s), page: Arc::new(p) },
                Err(e) => Msg::Failed { generation, error: format!("{e:#}") },
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn change_page(&mut self, page: usize, ctx: &egui::Context) {
        let Some(src) = self.source.clone() else { return };
        self.generation += 1;
        let generation = self.generation;
        self.settings.page = page;
        self.settings.manual_crop = None;
        self.busy = true;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match src.render_page(page) {
                Ok(p) => Msg::Loaded { generation, source: src, page: Arc::new(p) },
                Err(e) => Msg::Failed { generation, error: format!("{e:#}") },
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn reprocess(&mut self, ctx: &egui::Context) {
        let Some(page) = self.page.clone() else { return };
        self.generation += 1;
        let generation = self.generation;
        self.busy = true;
        self.dirty = false;
        let settings = self.settings.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match crate::pipeline::process(&page, &settings) {
                Ok(o) => Msg::Processed { generation, output: Box::new(o) },
                Err(e) => Msg::Failed { generation, error: format!("{e:#}") },
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
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
                    self.status = None;
                    self.reprocess(ctx);
                }
                Msg::Processed { generation, output } if generation == self.generation => {
                    let img = gray_to_color(&output.bitmap);
                    self.out_tex = Some(ctx.load_texture("label", img, TextureOptions::NEAREST));
                    self.output = Some(*output);
                    self.busy = false;
                }
                Msg::Failed { generation, error } if generation == self.generation => {
                    self.busy = false;
                    self.status = Some((error, true));
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
            self.status = Some(("Pick a printer first.".into(), true));
            return;
        }
        self.status = Some(match print::print(&self.prefs.printer, &out.bitmap, &self.settings, &self.prefs.cups_extra) {
            Ok(()) => (format!("Sent to {}.", self.prefs.printer), false),
            Err(e) => (format!("Print failed: {e:#}"), true),
        });
    }

    fn save(&mut self, pdf: bool) {
        let Some(out) = &self.output else { return };
        let stem = self
            .source
            .as_ref()
            .and_then(|s| std::path::Path::new(&s.name).file_stem().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "label".into());
        let (ext, name) = if pdf { ("pdf", "PDF") } else { ("png", "PNG image") };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{stem}-print.{ext}"))
            .add_filter(name, &[ext])
            .save_file()
        else {
            return;
        };
        let bytes = if pdf {
            Ok(pdfout::label_pdf(&out.bitmap, self.settings.width_mm, self.settings.height_mm))
        } else {
            pdfout::label_png(&out.bitmap, self.settings.dpi)
        };
        self.status = Some(match bytes.and_then(|b| std::fs::write(&path, b).map_err(Into::into)) {
            Ok(()) => (format!("Saved {}", path.display()), false),
            Err(e) => (format!("Save failed: {e:#}"), true),
        });
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.add_space(6.0);
        if ui.add_sized([ui.available_width(), 30.0], egui::Button::new("Open label…")).clicked()
            && let Some(p) = rfd::FileDialog::new()
                .add_filter("Labels", &["pdf", "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif"])
                .pick_file()
        {
            self.open(p, &ctx);
        }
        if let Some(src) = &self.source {
            ui.label(RichText::new(&src.name).weak());
            if src.page_count > 1 {
                let n = src.page_count;
                let mut page = self.settings.page;
                ui.horizontal(|ui| {
                    ui.label("Page");
                    if ui.add(egui::DragValue::new(&mut page).range(0..=n - 1).custom_formatter(|v, _| format!("{} of {n}", v as usize + 1))).changed() {
                        self.change_page(page, &ctx);
                    }
                });
            }
        } else {
            ui.label(RichText::new("…or drop a PDF / image anywhere").weak());
        }

        ui.separator();
        ui.heading("Adjust");
        ui.horizontal(|ui| {
            ui.label("Rotate");
            if ui.button("⟲ 90°").clicked() {
                self.settings.extra_rotation = (self.settings.extra_rotation + 3) % 4;
                self.dirty = true;
            }
            if ui.button("⟳ 90°").clicked() {
                self.settings.extra_rotation = (self.settings.extra_rotation + 1) % 4;
                self.dirty = true;
            }
            if ui.button("180°").clicked() {
                self.settings.extra_rotation = (self.settings.extra_rotation + 2) % 4;
                self.dirty = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label(if self.settings.manual_crop.is_some() { "Crop: manual" } else { "Crop: automatic" });
            if self.settings.manual_crop.is_some() && ui.button("Reset").clicked() {
                self.settings.manual_crop = None;
                self.dirty = true;
            }
        });
        if ui.add(egui::Slider::new(&mut self.prefs.darkness, -20..=20).text("Darkness")).changed() {
            self.dirty = true;
        }
        if ui.checkbox(&mut self.prefs.regenerate, "Regenerate barcodes").on_hover_text(
            "Re-draw every barcode that decodes, pixel-perfect, then re-scan it.\nIf the re-scan doesn't match exactly, the original is kept.",
        ).changed() {
            self.dirty = true;
        }
        ui.checkbox(&mut self.show_boxes, "Outline barcodes in preview");

        ui.separator();
        ui.heading("Print");
        egui::ComboBox::from_id_salt("printer")
            .width(ui.available_width() - 30.0)
            .selected_text(if self.prefs.printer.is_empty() { "Choose printer" } else { &self.prefs.printer })
            .show_ui(ui, |ui| {
                for p in &self.printers {
                    ui.selectable_value(&mut self.prefs.printer, p.clone(), p);
                }
            });
        if ui.small_button("↻ Refresh printers").clicked() {
            self.printers = print::list_printers();
        }
        let ready = self.output.is_some() && !self.busy;
        let print_btn = egui::Button::new(RichText::new("🖨  Print label").size(17.0)).fill(Color32::from_rgb(38, 110, 70));
        if ui.add_enabled(ready, print_btn.min_size(vec2(ui.available_width(), 38.0))).clicked() {
            self.do_print();
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(ready, egui::Button::new("Save PNG…")).clicked() {
                self.save(false);
            }
            if ui.add_enabled(ready, egui::Button::new("Save PDF…")).clicked() {
                self.save(true);
            }
        });

        egui::CollapsingHeader::new("Label & printer settings").show(ui, |ui| {
            let mut changed = false;
            egui::Grid::new("dims").num_columns(2).show(ui, |ui| {
                ui.label("Width (mm)");
                changed |= ui.add(egui::DragValue::new(&mut self.prefs.width_mm).range(40.0..=120.0)).changed();
                ui.end_row();
                ui.label("Height (mm)");
                changed |= ui.add(egui::DragValue::new(&mut self.prefs.height_mm).range(40.0..=300.0)).changed();
                ui.end_row();
                ui.label("Edge margin (mm)");
                changed |= ui.add(egui::DragValue::new(&mut self.prefs.margin_mm).range(0.0..=8.0).speed(0.1)).changed();
                ui.end_row();
                ui.label("Printer DPI");
                egui::ComboBox::from_id_salt("dpi").selected_text(format!("{}", self.prefs.dpi)).show_ui(ui, |ui| {
                    for d in [203.0, 300.0] {
                        changed |= ui.selectable_value(&mut self.prefs.dpi, d, format!("{d}")).changed();
                    }
                });
                ui.end_row();
            });
            if !cfg!(windows) {
                ui.label("Extra CUPS options");
                ui.text_edit_singleline(&mut self.prefs.cups_extra)
                    .on_hover_text("Space-separated, passed to lp as -o options, e.g. media=w288h432");
            }
            if changed {
                self.dirty = true;
            }
        });

        ui.separator();
        self.report(ui);
    }

    fn report(&self, ui: &mut egui::Ui) {
        if let Some((msg, err)) = &self.status {
            ui.label(RichText::new(msg).color(if *err { Color32::from_rgb(220, 80, 70) } else { Color32::from_rgb(90, 180, 120) }));
        }
        if self.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Working…");
            });
        }
        let Some(out) = &self.output else { return };
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(RichText::new("Barcodes").strong());
            if out.barcodes.is_empty() {
                ui.label(RichText::new("None could be read. The label was cleaned up but no barcode was regenerated.").color(Color32::from_rgb(230, 170, 60)));
            }
            for b in &out.barcodes {
                let color = match b.status {
                    Status::Regenerated { .. } | Status::Readable => Color32::from_rgb(90, 180, 120),
                    Status::Kept(_) => Color32::from_rgb(230, 170, 60),
                };
                ui.label(RichText::new(b.summary()).color(color).size(12.0));
            }
            ui.label(
                RichText::new("Any barcode on the original that isn't listed here couldn't be read, and was printed as cleaned-up pixels.")
                    .weak()
                    .size(11.0),
            );
            if !out.notes.is_empty() {
                ui.add_space(4.0);
                for n in &out.notes {
                    ui.label(RichText::new(format!("· {n}")).weak().size(12.0));
                }
            }
        });
    }

    fn previews(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_size();
        let gap = 16.0;
        let col_w = (avail.x - gap - 12.0) / 2.0;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(col_w);
                ui.label(RichText::new("Original — drag to set the crop").weak());
                self.source_view(ui, vec2(col_w, avail.y - 24.0));
            });
            ui.add_space(gap);
            ui.vertical(|ui| {
                ui.set_width(col_w);
                ui.label(RichText::new(format!(
                    "Ready to print — {}×{} mm @ {} dpi",
                    self.settings.width_mm, self.settings.height_mm, self.settings.dpi
                ))
                .weak());
                self.label_view(ui, vec2(col_w, avail.y - 24.0));
            });
        });
    }

    fn source_view(&mut self, ui: &mut egui::Ui, max: Vec2) {
        let Some((tex, scale)) = &self.src_tex else {
            drop_hint(ui, max);
            return;
        };
        let tsize = tex.size_vec2();
        let fit = (max.x / tsize.x).min(max.y / tsize.y);
        let (rect, resp) = ui.allocate_exact_size(tsize * fit, Sense::drag());
        let p = ui.painter_at(rect);
        p.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        // screen ↔ source pixel mapping
        let k = fit / scale; // screen px per source px
        let to_screen = |x: f32, y: f32| rect.min + vec2(x * k, y * k);
        let to_src = |q: Pos2| ((q.x - rect.min.x) / k, (q.y - rect.min.y) / k);

        let crop = self.settings.manual_crop.or(self.output.as_ref().map(|o| o.crop));
        if let Some(c) = crop {
            let r = Rect::from_min_max(to_screen(c.0 as f32, c.1 as f32), to_screen((c.0 + c.2) as f32, (c.1 + c.3) as f32));
            p.rect_stroke(r, 0.0, Stroke::new(2.0, Color32::from_rgb(60, 140, 240)), StrokeKind::Outside);
        }
        if resp.drag_started() {
            self.drag_start = resp.interact_pointer_pos();
        }
        if let (Some(a), Some(b)) = (self.drag_start, resp.interact_pointer_pos()) {
            let r = Rect::from_two_pos(a, b).intersect(rect);
            p.rect_stroke(r, 0.0, Stroke::new(2.0, Color32::from_rgb(240, 160, 40)), StrokeKind::Outside);
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
    }

    fn label_view(&self, ui: &mut egui::Ui, max: Vec2) {
        let Some(tex) = &self.out_tex else {
            let (rect, _) = ui.allocate_exact_size(max, Sense::hover());
            if self.busy {
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Preparing…", FontId::proportional(16.0), Color32::GRAY);
            }
            return;
        };
        let tsize = tex.size_vec2();
        let fit = (max.x / tsize.x).min(max.y / tsize.y);
        let (rect, _) = ui.allocate_exact_size(tsize * fit, Sense::hover());
        let p = ui.painter_at(rect.expand(3.0));
        p.rect_filled(rect.expand(2.0), 2.0, Color32::from_gray(90));
        p.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        if let (true, Some(out)) = (self.show_boxes, &self.output) {
            for b in &out.barcodes {
                let (x, y, w, h) = b.region;
                let r = Rect::from_min_size(rect.min + vec2(x as f32, y as f32) * fit, vec2(w as f32, h as f32) * fit);
                let c = match b.status {
                    Status::Kept(_) => Color32::from_rgb(230, 160, 40),
                    _ => Color32::from_rgb(40, 170, 90),
                };
                p.rect_stroke(r.expand(2.0), 2.0, Stroke::new(1.5, c), StrokeKind::Outside);
            }
        }
        if self.busy {
            p.rect_filled(rect, 0.0, Color32::from_black_alpha(60));
        }
    }
}

fn drop_hint(ui: &mut egui::Ui, max: Vec2) {
    let (rect, _) = ui.allocate_exact_size(max, Sense::hover());
    let p = ui.painter();
    p.rect_stroke(rect.shrink(8.0), 12.0, Stroke::new(2.0, Color32::from_gray(110)), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, "Drop a shipping label here\n(PDF, PNG, JPG)", FontId::proportional(18.0), Color32::from_gray(160));
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

        egui::Panel::left("controls").resizable(false).exact_size(300.0).show(ui, |ui| {
            ui.add_space(4.0);
            ui.heading("LabelCraft");
            self.controls(ui);
        });
        egui::CentralPanel::default_margins().show(ui, |ui| self.previews(ui));

        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let screen = ctx.content_rect();
            let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("drop")));
            p.rect_filled(screen, 0.0, Color32::from_black_alpha(160));
            p.text(screen.center(), Align2::CENTER_CENTER, "Drop to load", FontId::proportional(28.0), Color32::WHITE);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::P) && i.modifiers.command) && self.output.is_some() && !self.busy {
            self.do_print();
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "prefs", &self.prefs);
    }
}
