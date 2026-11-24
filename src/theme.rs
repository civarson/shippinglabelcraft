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
    pub name: &'static str,
    pub dark: bool,
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
    name: "Paper",
    dark: false,
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
    name: "Night Shift",
    dark: true,
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

/// Build a palette from a handful of base colours. `soft` variants are mixed toward the surface.
#[allow(clippy::too_many_arguments)]
const fn palette(
    name: &'static str,
    dark: bool,
    [canvas, surface, surface_alt, sunken]: [u32; 4],
    [border, border_strong]: [u32; 2],
    [text, text_muted, text_faint]: [u32; 3],
    [accent, accent_hover, accent_soft, on_accent]: [u32; 4],
    [ok, ok_soft, warn, warn_soft, danger]: [u32; 5],
) -> Tokens {
    Tokens {
        name,
        dark,
        canvas: hex(canvas),
        surface: hex(surface),
        surface_alt: hex(surface_alt),
        sunken: hex(sunken),
        border: hex(border),
        border_strong: hex(border_strong),
        text: hex(text),
        text_muted: hex(text_muted),
        text_faint: hex(text_faint),
        accent: hex(accent),
        accent_hover: hex(accent_hover),
        accent_soft: hex(accent_soft),
        on_accent: hex(on_accent),
        ok: hex(ok),
        ok_soft: hex(ok_soft),
        warn: hex(warn),
        warn_soft: hex(warn_soft),
        danger: hex(danger),
        paper: hex(0xFFFFFF),
        dots: hex(border),
        shadow: if dark {
            Color32::from_rgba_premultiplied(0, 0, 0, 110)
        } else {
            Color32::from_rgba_premultiplied(30, 30, 40, 34)
        },
    }
}

/// Every built-in theme. Community palettes use their published colours.
pub const THEMES: &[Tokens] = &[
    PAPER,
    NIGHT,
    palette("Catppuccin Latte", false, [0xDCE0E8, 0xEFF1F5, 0xE6E9EF, 0xCCD0DA], [0xCCD0DA, 0xACB0BE], [0x4C4F69, 0x6C6F85, 0x8C8FA1], [0x8839EF, 0x7A2FD6, 0xE7DCFB, 0xFFFFFF], [0x40A02B, 0xDCEFD6, 0xDF8E1D, 0xF8EAD3, 0xD20F39]),
    palette("Catppuccin Mocha", true, [0x11111B, 0x1E1E2E, 0x262637, 0x181825], [0x313244, 0x45475A], [0xCDD6F4, 0xA6ADC8, 0x7F849C], [0xCBA6F7, 0xD9BDF9, 0x3B3150, 0x1E1E2E], [0xA6E3A1, 0x2A3A33, 0xF9E2AF, 0x3D3A2C, 0xF38BA8]),
    palette("Dracula", true, [0x21222C, 0x282A36, 0x303241, 0x1E1F29], [0x44475A, 0x565970], [0xF8F8F2, 0xBDC0D2, 0x6272A4], [0xFF79C6, 0xFF92D0, 0x4A2F43, 0x282A36], [0x50FA7B, 0x24402F, 0xFFB86C, 0x45382B, 0xFF5555]),
    palette("Nord", true, [0x272C36, 0x2E3440, 0x353C4A, 0x292E39], [0x3B4252, 0x4C566A], [0xECEFF4, 0xB6BECC, 0x7B86A0], [0x88C0D0, 0x9FD0DC, 0x2F4450, 0x2E3440], [0xA3BE8C, 0x36423A, 0xEBCB8B, 0x46433A, 0xBF616A]),
    palette("Gruvbox Light", false, [0xF2E5BC, 0xFBF1C7, 0xF6ECC4, 0xEBDBB2], [0xE0CFA5, 0xBDAE93], [0x3C3836, 0x665C54, 0x928374], [0xD65D0E, 0xAF3A03, 0xF6D9B8, 0xFBF1C7], [0x79740E, 0xE6E3B0, 0xB57614, 0xF4DEB0, 0x9D0006]),
    palette("Gruvbox Dark", true, [0x1D2021, 0x282828, 0x32302F, 0x242424], [0x3C3836, 0x504945], [0xEBDBB2, 0xBDAE93, 0x928374], [0xFE8019, 0xFF9A45, 0x4A3424, 0x282828], [0xB8BB26, 0x3A3A20, 0xFABD2F, 0x463A20, 0xFB4934]),
    palette("Solarized Light", false, [0xEEE8D5, 0xFDF6E3, 0xF7F0DC, 0xE9E2CE], [0xE3DCC8, 0x93A1A1], [0x073642, 0x586E75, 0x93A1A1], [0x268BD2, 0x1F74B0, 0xD9E7EF, 0xFDF6E3], [0x859900, 0xE8EBC8, 0xB58900, 0xF2E6BF, 0xDC322F]),
    palette("Solarized Dark", true, [0x00212B, 0x002B36, 0x073642, 0x00252F], [0x0A3D4A, 0x586E75], [0xEEE8D5, 0x93A1A1, 0x657B83], [0x268BD2, 0x4A9FE0, 0x0D3A52, 0xFDF6E3], [0x859900, 0x1D3B26, 0xB58900, 0x2E3A1C, 0xDC322F]),
    palette("Tokyo Night", true, [0x16161E, 0x1A1B26, 0x1F2335, 0x13131A], [0x292E42, 0x414868], [0xC0CAF5, 0xA9B1D6, 0x565F89], [0x7AA2F7, 0x8FB2F9, 0x24304F, 0x1A1B26], [0x9ECE6A, 0x26332A, 0xE0AF68, 0x3A3226, 0xF7768E]),
    palette("Rosé Pine", true, [0x191724, 0x1F1D2E, 0x26233A, 0x16141F], [0x2E2B42, 0x524F67], [0xE0DEF4, 0x908CAA, 0x6E6A86], [0xEBBCBA, 0xF2CFCD, 0x3A2E38, 0x191724], [0x9CCFD8, 0x22333A, 0xF6C177, 0x3B3128, 0xEB6F92]),
    palette("One Dark", true, [0x21252B, 0x282C34, 0x2C313A, 0x1E2227], [0x3A3F4B, 0x4B5263], [0xDCDFE4, 0xABB2BF, 0x5C6370], [0x61AFEF, 0x7BBDF2, 0x263849, 0x21252B], [0x98C379, 0x2B3A2C, 0xE5C07B, 0x3D3828, 0xE06C75]),
    palette("GitHub Light", false, [0xF6F8FA, 0xFFFFFF, 0xF6F8FA, 0xEFF2F5], [0xD0D7DE, 0xAFB8C1], [0x1F2328, 0x59636E, 0x818B98], [0x0969DA, 0x0550AE, 0xDDF4FF, 0xFFFFFF], [0x1A7F37, 0xDAFBE1, 0x9A6700, 0xFFF8C5, 0xD1242F]),
    palette("GitHub Dark", true, [0x010409, 0x0D1117, 0x151B23, 0x010409], [0x30363D, 0x3D444D], [0xF0F6FC, 0x9198A1, 0x656C76], [0x4493F8, 0x6AA8FA, 0x12253F, 0xFFFFFF], [0x3FB950, 0x12261E, 0xD29922, 0x2E2414, 0xF85149]),
];

pub fn find(name: &str) -> Option<Tokens> {
    THEMES.iter().copied().find(|t| t.name == name)
}

static ACTIVE: std::sync::RwLock<(Tokens, Tokens)> = std::sync::RwLock::new((PAPER, NIGHT));

impl Tokens {
    pub fn get(ctx: &egui::Context) -> Tokens {
        let (light, dark) = *ACTIVE.read().unwrap();
        if ctx.theme() == Theme::Dark { dark } else { light }
    }
}

/// Use `light` when egui is in light mode and `dark` in dark mode.
pub fn set_palettes(ctx: &egui::Context, light: &str, dark: &str) {
    let l = find(light).unwrap_or(PAPER);
    let d = find(dark).unwrap_or(NIGHT);
    *ACTIVE.write().unwrap() = (l, d);
    ctx.set_visuals_of(Theme::Light, visuals(&l, l.dark));
    ctx.set_visuals_of(Theme::Dark, visuals(&d, d.dark));
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

    set_palettes(ctx, PAPER.name, NIGHT.name);
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
    "circle-check", "circle-alert", "scissors", "pencil", "undo-2", "lock",
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
