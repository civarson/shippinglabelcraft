//! Small, hand-drawn widgets on top of egui, all coloured from theme tokens.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

use crate::theme::{self, R_CARD, R_CONTROL, R_PILL, R_SMALL, Tokens};

/// A rounded card with an icon + title header and optional trailing header content.
pub fn card<R>(
    ui: &mut Ui,
    t: &Tokens,
    icon: &str,
    title: &str,
    trailing: impl FnOnce(&mut Ui),
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    egui::Frame::new()
        .fill(t.surface)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(R_CARD))
        .inner_margin(egui::Margin::same(14))
        .shadow(theme::card_shadow(t))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                theme::paint_icon(ui, icon, r, t.accent);
                ui.label(egui::RichText::new(title).font(theme::semibold(13.5)).color(t.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), trailing);
            });
            ui.add_space(4.0);
            body(ui)
        })
        .inner
}

/// Big filled call-to-action button.
pub fn primary_button(ui: &mut Ui, t: &Tokens, icon: &str, label: &str, hint: Option<&str>, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), if enabled { Sense::click() } else { Sense::hover() });
    let hover = ui.ctx().animate_bool(resp.id.with("h"), enabled && resp.hovered());
    let pressed = enabled && resp.is_pointer_button_down_on();
    let fill = if !enabled {
        t.border
    } else {
        lerp(t.accent, t.accent_hover, hover)
    };
    let r = rect.translate(vec2(0.0, if pressed { 1.0 } else { -hover }));
    let p = ui.painter();
    if enabled {
        p.add(egui::Shadow { offset: [0, 3], blur: (6.0 + 6.0 * hover) as u8, spread: 0, color: t.accent.gamma_multiply(0.35) }.as_shape(r, R_CONTROL));
    }
    p.rect_filled(r, R_CONTROL, fill);
    let fg = if enabled { t.on_accent } else { t.text_faint };
    let text = p.layout_no_wrap(label.to_string(), theme::semibold(15.0), fg);
    let total = 20.0 + 8.0 + text.size().x;
    let x0 = r.center().x - total / 2.0;
    theme::paint_icon(ui, icon, Rect::from_min_size(pos2(x0, r.center().y - 10.0), vec2(20.0, 20.0)), fg);
    p.galley(pos2(x0 + 28.0, r.center().y - text.size().y / 2.0), text, fg);
    if let Some(h) = hint {
        let g = p.layout_no_wrap(h.to_string(), theme::medium(11.0), fg.gamma_multiply(0.75));
        let kr = Rect::from_min_size(pos2(r.right() - g.size().x - 22.0, r.center().y - 10.0), vec2(g.size().x + 12.0, 20.0));
        p.rect_filled(kr, R_SMALL, Color32::from_white_alpha(if enabled { 40 } else { 0 }));
        p.galley(kr.center() - g.size() / 2.0, g, fg.gamma_multiply(0.85));
    }
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Quiet outlined button with an icon.
pub fn ghost_button(ui: &mut Ui, t: &Tokens, icon: &str, label: &str, width: f32, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 34.0), if enabled { Sense::click() } else { Sense::hover() });
    let hover = ui.ctx().animate_bool(resp.id.with("h"), enabled && resp.hovered());
    let p = ui.painter();
    p.rect(rect, R_CONTROL, lerp(t.surface, t.surface_alt, hover), Stroke::new(1.0, lerp(t.border, t.border_strong, hover)), StrokeKind::Inside);
    let fg = if enabled { t.text } else { t.text_faint };
    let g = p.layout_no_wrap(label.to_string(), theme::medium(13.0), fg);
    let total = 16.0 + 7.0 + g.size().x;
    let x0 = rect.center().x - total / 2.0;
    theme::paint_icon(ui, icon, Rect::from_min_size(pos2(x0, rect.center().y - 8.0), vec2(16.0, 16.0)), if enabled { t.text_muted } else { t.text_faint });
    p.galley(pos2(x0 + 23.0, rect.center().y - g.size().y / 2.0), g, fg);
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// Square icon-only button.
pub fn icon_button(ui: &mut Ui, t: &Tokens, icon: &str, tip: &str, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let hover = ui.ctx().animate_bool(resp.id.with("h"), resp.hovered());
    let p = ui.painter();
    if hover > 0.0 {
        p.rect_filled(rect, R_CONTROL, t.sunken.gamma_multiply(hover));
    }
    let s = (size * 0.5).round();
    let color = if ui.is_enabled() { lerp(t.text_muted, t.text, hover) } else { t.text_faint.gamma_multiply(0.6) };
    theme::paint_icon(ui, icon, Rect::from_center_size(rect.center(), Vec2::splat(s)), color);
    if ui.is_enabled() {
        resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp.on_disabled_hover_text(tip)
    }
}

/// iOS-style toggle with a label and optional caption.
pub fn switch(ui: &mut Ui, t: &Tokens, on: &mut bool, label: &str, caption: Option<&str>) -> Response {
    let width = ui.available_width();
    let cap_h = if caption.is_some() { 16.0 } else { 0.0 };
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 24.0 + cap_h), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let k = ui.ctx().animate_bool_with_time(resp.id, *on, 0.14);
    let p = ui.painter();
    let track = Rect::from_min_size(pos2(rect.right() - 38.0, rect.top() + 2.0), vec2(38.0, 22.0));
    p.rect_filled(track, R_PILL, lerp(t.border_strong, t.accent, k));
    let knob_x = egui::lerp(track.left() + 11.0..=track.right() - 11.0, k);
    p.circle_filled(pos2(knob_x, track.center().y + 0.5), 8.5, Color32::from_black_alpha(30));
    p.circle_filled(pos2(knob_x, track.center().y), 8.5, Color32::WHITE);
    p.text(pos2(rect.left(), rect.top() + 13.0), Align2::LEFT_CENTER, label, theme::medium(13.0), t.text);
    if let Some(c) = caption {
        p.text(pos2(rect.left(), rect.top() + 32.0), Align2::LEFT_CENTER, c, theme::body(11.5), t.text_faint);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Pill-shaped segmented control of icons. Returns true when the selection changed.
pub fn segmented(ui: &mut Ui, t: &Tokens, items: &[(&str, &str)], selected: &mut usize) -> bool {
    let seg = 30.0;
    let (rect, _) = ui.allocate_exact_size(vec2(seg * items.len() as f32 + 6.0, 34.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, R_PILL, t.sunken);
    let mut changed = false;
    for (i, (icon, tip)) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + 3.0 + i as f32 * seg, rect.top() + 3.0), vec2(seg, 28.0));
        let resp = ui.interact(r, ui.scope_id().with(("seg", i)), Sense::click()).on_hover_text(*tip);
        if resp.clicked() && *selected != i {
            *selected = i;
            changed = true;
        }
        let sel = *selected == i;
        if sel {
            p.add(egui::Shadow { offset: [0, 1], blur: 4, spread: 0, color: t.shadow }.as_shape(r, R_PILL));
            p.rect_filled(r, R_PILL, t.surface);
        }
        let c = if sel { t.accent } else if resp.hovered() { t.text } else { t.text_faint };
        theme::paint_icon(ui, icon, Rect::from_center_size(r.center(), Vec2::splat(15.0)), c);
    }
    changed
}

/// Small rounded label.
pub fn badge(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) {
    let g = ui.painter().layout_no_wrap(text.to_string(), theme::semibold(11.0), fg);
    let (rect, _) = ui.allocate_exact_size(vec2(g.size().x + 14.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, R_PILL, bg);
    ui.painter().galley(rect.center() - g.size() / 2.0, g, fg);
}

/// LabelCraft's mark: a coral tile holding a little shipping tag with barcode stripes.
pub fn logo(p: &egui::Painter, rect: Rect, t: &Tokens) {
    let r = rect.height() * 0.28;
    p.rect_filled(rect, r, t.accent);
    let s = rect.height();
    let tag = Rect::from_center_size(rect.center() + vec2(0.0, s * 0.02), vec2(s * 0.56, s * 0.62));
    p.rect_filled(tag, s * 0.08, Color32::WHITE);
    // punch hole
    p.circle_filled(pos2(tag.center().x, tag.top() + s * 0.11), s * 0.045, t.accent);
    // barcode
    let bars = [1.0, 0.5, 1.5, 0.5, 1.0, 1.0, 0.5, 1.5, 0.5, 1.0];
    let total: f32 = bars.iter().sum::<f32>() + (bars.len() as f32 - 1.0) * 0.6;
    let unit = (tag.width() * 0.66) / total;
    let mut x = tag.center().x - tag.width() * 0.33;
    let (y0, y1) = (tag.top() + s * 0.24, tag.bottom() - s * 0.1);
    for (i, w) in bars.iter().enumerate() {
        if i % 2 == 0 {
            p.rect_filled(Rect::from_min_max(pos2(x, y0), pos2(x + w * unit, y1)), 0.0, hex_ink(t));
        }
        x += (w + 0.6) * unit;
    }
}

fn hex_ink(t: &Tokens) -> Color32 {
    if t.text.r() > 128 { Color32::from_rgb(0x2A, 0x25, 0x21) } else { t.text }
}

/// Dotted "cutting mat" background.
pub fn dotted_canvas(p: &egui::Painter, rect: Rect, t: &Tokens) {
    p.rect_filled(rect, 0.0, t.canvas);
    let step = 18.0;
    let mut y = rect.top() + (step / 2.0);
    while y < rect.bottom() {
        let mut x = rect.left() + (step / 2.0);
        while x < rect.right() {
            p.circle_filled(pos2(x, y), 1.0, t.dots);
            x += step;
        }
        y += step;
    }
}

pub fn lerp(a: Color32, b: Color32, k: f32) -> Color32 {
    let k = k.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Color32::from_rgba_premultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}

/// Section caption in small caps style.
pub fn caption(ui: &mut Ui, t: &Tokens, text: &str) {
    ui.label(egui::RichText::new(text.to_uppercase()).font(theme::semibold(10.5)).color(t.text_faint).extra_letter_spacing(0.8));
}

