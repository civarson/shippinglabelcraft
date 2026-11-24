//! LabelCraft's look: "kraft paper & stamp ink".
//!
//! Every colour, radius and size the UI paints comes from [`Tokens`]; widgets never
//! hard-code colours. Light ("Paper") and dark ("Night Shift") token sets share one layout.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle,
    Theme, Visuals,
};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct Tokens {
    pub canvas: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub sunken: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub ok: Color32,
    pub ok_soft: Color32,
    pub warn: Color32,
    pub warn_soft: Color32,
    pub danger: Color32,
    pub paper: Color32,
    pub dots: Color32,
    pub shadow: Color32,
}

/// Radii (px).
pub const R_CARD: u8 = 14;
pub const R_CONTROL: u8 = 10;
pub const R_SMALL: u8 = 6;
pub const R_PILL: u8 = 99;

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const PAPER: Tokens = Tokens {
    canvas: hex(0xF2EDE4),
    surface: hex(0xFFFDF8),
    surface_alt: hex(0xF7F2EA),
    sunken: hex(0xEDE6DA),
    border: hex(0xE4DACB),
    border_strong: hex(0xCFC2AF),
    text: hex(0x2A2521),
    text_muted: hex(0x6F655B),
    text_faint: hex(0x9C9186),
    accent: hex(0xE4573A),
    accent_hover: hex(0xCF4729),
    accent_soft: hex(0xFBE4DC),
    on_accent: hex(0xFFFFFF),
    ok: hex(0x23855A),
    ok_soft: hex(0xDCF1E5),
    warn: hex(0xA86C0A),
    warn_soft: hex(0xF8ECD2),
    danger: hex(0xC63A30),
    paper: hex(0xFFFFFF),
    dots: hex(0xE1D8C9),
    shadow: Color32::from_rgba_premultiplied(60, 40, 20, 40),
};

pub const NIGHT: Tokens = Tokens {
    canvas: hex(0x181614),
    surface: hex(0x232020),
    surface_alt: hex(0x2B2725),
    sunken: hex(0x1D1A18),
    border: hex(0x37322E),
    border_strong: hex(0x4A443E),
    text: hex(0xF3EEE7),
    text_muted: hex(0xB3AAA0),
    text_faint: hex(0x80776E),
    accent: hex(0xFF7656),
    accent_hover: hex(0xFF8D71),
    accent_soft: hex(0x41271F),
    on_accent: hex(0x1B1210),
    ok: hex(0x56C996),
    ok_soft: hex(0x1E3429),
    warn: hex(0xEDB653),
    warn_soft: hex(0x3B2F18),
    danger: hex(0xFF6B5E),
    paper: hex(0xFFFFFF),
    dots: hex(0x2C2825),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 110),
};

impl Tokens {
    pub fn get(ctx: &egui::Context) -> Tokens {
        if ctx.theme() == Theme::Dark { NIGHT } else { PAPER }
    }
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}
pub fn body(size: f32) -> FontId {
    FontId::proportional(size)
}
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn card_shadow(t: &Tokens) -> Shadow {
    Shadow { offset: [0, 2], blur: 14, spread: 0, color: t.shadow }
}

/// Install fonts and both themes.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add(&mut fonts, "inter", include_bytes!("../assets/fonts/Inter-Regular.ttf"));
    add(&mut fonts, "inter-medium", include_bytes!("../assets/fonts/Inter-Medium.ttf"));
    add(&mut fonts, "inter-semibold", include_bytes!("../assets/fonts/Inter-SemiBold.ttf"));
    add(&mut fonts, "jbmono", include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"));
    // Keep egui's defaults behind ours as fallbacks (symbols, emoji).
    let fallbacks: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let fam = |first: &str| -> Vec<String> { std::iter::once(first.to_string()).chain(fallbacks.iter().cloned()).collect() };
    fonts.families.insert(FontFamily::Proportional, fam("inter"));
    fonts.families.insert(FontFamily::Name("medium".into()), fam("inter-medium"));
    fonts.families.insert(FontFamily::Name("semibold".into()), fam("inter-semibold"));
    let mut mono_fam = vec!["jbmono".to_string()];
    mono_fam.extend(fallbacks.iter().cloned());
    fonts.families.insert(FontFamily::Monospace, mono_fam);
    ctx.set_fonts(fonts);

    ctx.set_visuals_of(Theme::Light, visuals(&PAPER, false));
    ctx.set_visuals_of(Theme::Dark, visuals(&NIGHT, true));
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, semibold(18.0)),
            (TextStyle::Body, body(13.5)),
            (TextStyle::Button, medium(13.5)),
            (TextStyle::Small, body(11.5)),
            (TextStyle::Monospace, mono(12.5)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 7.0);
        s.spacing.interact_size.y = 32.0;
        s.spacing.slider_rail_height = 6.0;
        s.spacing.combo_height = 260.0;
        s.spacing.menu_margin = Margin::same(8);
    });
}

fn visuals(t: &Tokens, dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.override_text_color = Some(t.text);
    v.panel_fill = t.surface;
    v.window_fill = t.surface;
    v.extreme_bg_color = t.sunken;
    v.faint_bg_color = t.surface_alt;
    v.code_bg_color = t.surface_alt;
    v.hyperlink_color = t.accent;
    v.warn_fg_color = t.warn;
    v.error_fg_color = t.danger;
    v.selection.bg_fill = t.accent;
    v.selection.stroke = Stroke::new(1.0, t.on_accent);
    v.window_stroke = Stroke::new(1.0, t.border);
    v.window_corner_radius = CornerRadius::same(R_CARD);
    v.menu_corner_radius = CornerRadius::same(R_CONTROL);
    v.window_shadow = Shadow { offset: [0, 8], blur: 28, spread: 0, color: t.shadow };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: t.shadow };
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;

    let w = &mut v.widgets;
    let r = CornerRadius::same(R_CONTROL);
    w.noninteractive.bg_fill = t.surface;
    w.noninteractive.weak_bg_fill = t.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text_muted);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = t.sunken;
    w.inactive.weak_bg_fill = t.surface_alt;
    w.inactive.bg_stroke = Stroke::new(1.0, t.border);
    w.inactive.fg_stroke = Stroke::new(1.5, t.text);
    w.inactive.corner_radius = r;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = t.sunken;
    w.hovered.weak_bg_fill = t.sunken;
    w.hovered.bg_stroke = Stroke::new(1.0, t.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.5, t.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = t.accent;
    w.active.weak_bg_fill = t.accent_soft;
    w.active.bg_stroke = Stroke::new(1.0, t.accent);
    w.active.fg_stroke = Stroke::new(1.5, t.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.bg_fill = t.surface_alt;
    w.open.weak_bg_fill = t.surface_alt;
    w.open.bg_stroke = Stroke::new(1.0, t.accent);
    w.open.fg_stroke = Stroke::new(1.5, t.text);
    w.open.corner_radius = r;
    v
}

// ───────────────────────────── icons ─────────────────────────────

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &str)] = &[$(($name, include_str!(concat!("../assets/icons/", $name, ".svg")))),*];
    };
}
icons!(
    "printer", "rotate-ccw", "rotate-cw", "crop", "folder-open", "download", "file-down", "settings-2", "sun", "moon",
    "monitor", "check", "triangle-alert", "scan-barcode", "sparkles", "upload", "refresh-cw", "sticker", "package", "x",
    "circle-check", "circle-alert", "scissors",
);

/// A Lucide icon as a white image source, tinted at paint time.
pub fn icon(name: &str) -> egui::ImageSource<'static> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, &'static [u8]>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut cache = cache.lock().unwrap();
    let bytes: &'static [u8] = cache.entry(name.to_string()).or_insert_with(|| {
        let svg = ICONS.iter().find(|(n, _)| *n == name).map(|(_, s)| *s).unwrap_or(ICONS[0].1);
        // Render white so `tint` gives the exact colour; draw a touch thinner than Lucide's 2px.
        let svg = svg.replace("currentColor", "#ffffff").replace("stroke-width=\"2\"", "stroke-width=\"1.9\"");
        Box::leak(svg.into_bytes().into_boxed_slice())
    });
    egui::ImageSource::Bytes { uri: format!("bytes://icon/{name}.svg").into(), bytes: egui::load::Bytes::Static(bytes) }
}

pub fn paint_icon(ui: &egui::Ui, name: &str, rect: egui::Rect, color: Color32) {
    egui::Image::new(icon(name)).tint(color).paint_at(ui, rect);
}
