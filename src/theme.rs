#![allow(non_snake_case)]
//! Design tokens: colours, fonts and global egui style («тёмная сцена»).

use eframe::egui::{self, Color32 as C, CornerRadius, FontFamily, FontId, Stroke, Visuals};
use std::path::PathBuf;
use std::sync::Arc;

/* ---------- colour schemes ---------- */
// Every colour is a function so the whole app can switch schemes on the fly.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

const N_TOK: usize = 31;
/// «Сцена» — the original dark stage with a pink accent.
const STAGE: [C; N_TOK] = [
    C::from_rgb(0x0D, 0x0A, 0x14),
    C::from_rgb(0x08, 0x06, 0x0C),
    C::from_rgb(0x15, 0x11, 0x1D),
    C::from_rgb(0x17, 0x12, 0x1F),
    C::from_rgb(0x1C, 0x16, 0x26),
    C::from_rgb(0x21, 0x1A, 0x2C),
    C::from_rgb(0x26, 0x1F, 0x33),
    C::from_rgb(0x2E, 0x25, 0x40),
    C::from_rgb(0x2A, 0x22, 0x38),
    C::from_rgb(0x34, 0x2A, 0x45),
    C::from_rgb(0x26, 0x1F, 0x33),
    C::from_rgb(0x1A, 0x15, 0x23),
    C::from_rgb(0x3A, 0x2F, 0x4D),
    C::from_rgb(0x3A, 0x30, 0x46),
    C::from_rgb(0xF4, 0xEE, 0xF8),
    C::from_rgb(0xCF, 0xC3, 0xDC),
    C::from_rgb(0xA9, 0x9B, 0xB8),
    C::from_rgb(0x8E, 0x80, 0x9E),
    C::from_rgb(0xFF, 0x4D, 0x8D),
    C::from_rgb(0xFF, 0x6F, 0xA3),
    C::from_rgb(0x1A, 0x07, 0x10),
    C::from_rgb(0x2A, 0x14, 0x20),
    C::from_rgb(0xFF, 0x8F, 0xB6),
    C::from_rgb(0xFF, 0xB5, 0x47),
    C::from_rgb(0x33, 0x24, 0x0C),
    C::from_rgb(0x2A, 0x1C, 0x10),
    C::from_rgb(0xFF, 0xC8, 0x71),
    C::from_rgb(0xFF, 0xD4, 0x9A),
    C::from_rgb(0x6B, 0xD4, 0x7A),
    C::from_rgb(0x9B, 0xE0, 0xA4),
    C::from_rgb(0x1E, 0x2A, 0x1F),
];

pub const SCHEMES: [&str; 4] = ["Сцена", "Неон", "Закат", "Лес"];
static CUR: AtomicUsize = AtomicUsize::new(0);
static TABLES: OnceLock<Vec<[C; N_TOK]>> = OnceLock::new();

fn mixc(a: C, b: C, t: f32) -> C {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    C::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}
fn lift(bg: C, k: f32) -> C {
    // lighten the background along its own hue (like the stage scheme does)
    let m = bg.r().max(bg.g()).max(bg.b()).max(1) as f32;
    let d = |x: u8| 0.5 + 0.5 * x as f32 / m;
    C::from_rgb((bg.r() as f32 + k * d(bg.r())).min(255.0) as u8, (bg.g() as f32 + k * d(bg.g())).min(255.0) as u8, (bg.b() as f32 + k * d(bg.b())).min(255.0) as u8)
}
/// A scheme from a background, an accent and a second (amber-role) colour.
fn scheme(bg: C, acc: C, amb: C) -> [C; N_TOK] {
    let w = C::from_rgb(0xF6, 0xF4, 0xF8);
    let tint = lift(bg, 160.0);
    let mut t = STAGE;
    let set = |t: &mut [C; N_TOK], name: &str, c: C| { if let Some(i) = NAMES.iter().position(|n| *n == name) { t[i] = c; } };
    for (n, k) in [("BG", 0.0), ("SIDEBAR", 9.0), ("SURFACE", 11.0), ("SURFACE_2", 16.0), ("INPUT", 22.0), ("CHIP", 27.0), ("SELECTED", 37.0),
                   ("BORDER", 32.0), ("BORDER_STRONG", 42.0), ("SIDEBAR_EDGE", 27.0), ("SEGMENT_BG", 13.0), ("DASH", 48.0), ("BAR_OFF", 46.0)] {
        set(&mut t, n, lift(bg, k));
    }
    set(&mut t, "BG_DEEP", mixc(bg, C::BLACK, 0.4));
    set(&mut t, "TEXT", mixc(w, tint, 0.08));
    set(&mut t, "TEXT_2", mixc(w, tint, 0.3));
    set(&mut t, "MUTED", mixc(w, tint, 0.55));
    set(&mut t, "PLACEHOLDER", mixc(w, tint, 0.7));
    set(&mut t, "ACCENT", acc);
    set(&mut t, "ACCENT_TEXT", mixc(acc, w, 0.2));
    set(&mut t, "ON_ACCENT", mixc(acc, C::BLACK, 0.9));
    set(&mut t, "ACCENT_SOFT", mixc(bg, acc, 0.14));
    set(&mut t, "REC_TEXT", mixc(acc, w, 0.4));
    set(&mut t, "AMBER", amb);
    set(&mut t, "AMBER_SOFT", mixc(bg, amb, 0.16));
    set(&mut t, "AMBER_WARN_BG", mixc(bg, amb, 0.1));
    set(&mut t, "AMBER_TEXT", mixc(amb, w, 0.25));
    set(&mut t, "AMBER_WARN_TEXT", mixc(amb, w, 0.45));
    t
}
const NAMES: [&str; N_TOK] = ["BG", "BG_DEEP", "SIDEBAR", "SURFACE", "SURFACE_2", "INPUT", "CHIP", "SELECTED", "BORDER", "BORDER_STRONG", "SIDEBAR_EDGE", "SEGMENT_BG", "DASH", "BAR_OFF", "TEXT", "TEXT_2", "MUTED", "PLACEHOLDER", "ACCENT", "ACCENT_TEXT", "ON_ACCENT", "ACCENT_SOFT", "REC_TEXT", "AMBER", "AMBER_SOFT", "AMBER_WARN_BG", "AMBER_TEXT", "AMBER_WARN_TEXT", "OK", "OK_TEXT", "OK_SOFT"];

fn tables() -> &'static Vec<[C; N_TOK]> {
    TABLES.get_or_init(|| vec![
        STAGE,
        scheme(C::from_rgb(0x07, 0x0B, 0x18), C::from_rgb(0x3D, 0xD6, 0xF5), C::from_rgb(0xFF, 0x4D, 0xD2)),
        scheme(C::from_rgb(0x14, 0x0B, 0x08), C::from_rgb(0xFF, 0x7A, 0x3D), C::from_rgb(0xFF, 0xD1, 0x5C)),
        scheme(C::from_rgb(0x07, 0x12, 0x0E), C::from_rgb(0x3D, 0xE0, 0x8F), C::from_rgb(0xFF, 0xC9, 0x47)),
    ])
}
fn tok(i: usize) -> C { tables()[CUR.load(Ordering::Relaxed).min(SCHEMES.len() - 1)][i] }
pub fn scheme_index() -> usize { CUR.load(Ordering::Relaxed) }
/// Switch the scheme (call `apply` afterwards to restyle egui).
pub fn set_scheme(i: usize) { CUR.store(i.min(SCHEMES.len() - 1), Ordering::Relaxed); }
/// The scheme's accent, for the picker swatches.
pub fn scheme_swatch(i: usize) -> (C, C, C) { let t = &tables()[i]; (t[0], t[18], t[23]) }

pub fn BG() -> C { tok(0) }
pub fn BG_DEEP() -> C { tok(1) }
pub fn SIDEBAR() -> C { tok(2) }
pub fn SURFACE() -> C { tok(3) }
pub fn SURFACE_2() -> C { tok(4) }
pub fn INPUT() -> C { tok(5) }
pub fn CHIP() -> C { tok(6) }
pub fn SELECTED() -> C { tok(7) }
pub fn BORDER() -> C { tok(8) }
pub fn BORDER_STRONG() -> C { tok(9) }
pub fn SIDEBAR_EDGE() -> C { tok(10) }
pub fn SEGMENT_BG() -> C { tok(11) }
pub fn DASH() -> C { tok(12) }
pub fn BAR_OFF() -> C { tok(13) }
pub fn TEXT() -> C { tok(14) }
pub fn TEXT_2() -> C { tok(15) }
pub fn MUTED() -> C { tok(16) }
pub fn PLACEHOLDER() -> C { tok(17) }
pub fn ACCENT() -> C { tok(18) }
pub fn ACCENT_TEXT() -> C { tok(19) }
pub fn ON_ACCENT() -> C { tok(20) }
pub fn ACCENT_SOFT() -> C { tok(21) }
pub fn REC_TEXT() -> C { tok(22) }
pub fn AMBER() -> C { tok(23) }
pub fn AMBER_SOFT() -> C { tok(24) }
pub fn AMBER_WARN_BG() -> C { tok(25) }
pub fn AMBER_TEXT() -> C { tok(26) }
pub fn AMBER_WARN_TEXT() -> C { tok(27) }
pub fn OK() -> C { tok(28) }
pub fn OK_TEXT() -> C { tok(29) }
pub fn OK_SOFT() -> C { tok(30) }

/* ---------- fonts ---------- */

pub fn display(size: f32) -> FontId { FontId::new(size, FontFamily::Name("display".into())) }
pub fn display_bold(size: f32) -> FontId { FontId::new(size, FontFamily::Name("display7".into())) }
pub fn body(size: f32) -> FontId { FontId::proportional(size) }
pub fn semibold(size: f32) -> FontId { FontId::new(size, FontFamily::Name("semibold".into())) }
pub fn bold(size: f32) -> FontId { FontId::new(size, FontFamily::Name("bold".into())) }

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let add = |fonts: &mut egui::FontDefinitions, name: &str, bytes: Vec<u8>| {
        fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
    };
    add(&mut fonts, "unbounded800", include_bytes!("../assets/fonts/Unbounded-ExtraBold.ttf").to_vec());
    add(&mut fonts, "unbounded700", include_bytes!("../assets/fonts/Unbounded-Bold.ttf").to_vec());
    add(&mut fonts, "manrope400", include_bytes!("../assets/fonts/Manrope-Regular.ttf").to_vec());
    add(&mut fonts, "manrope600", include_bytes!("../assets/fonts/Manrope-SemiBold.ttf").to_vec());
    add(&mut fonts, "manrope700", include_bytes!("../assets/fonts/Manrope-Bold.ttf").to_vec());

    // Segoe UI (+ symbols) as fallback for anything the bundled fonts lack.
    let dir = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
    let mut fallback: Vec<String> = Vec::new();
    for (name, file) in [("segoe", "segoeui.ttf"), ("segoe_sym", "seguisym.ttf")] {
        if let Ok(b) = std::fs::read(dir.join(file)) { add(&mut fonts, name, b); fallback.push(name.into()); }
    }
    let defaults = fonts.families[&FontFamily::Proportional].clone();
    let chain = |first: &str| -> Vec<String> {
        let mut v = vec![first.to_string()];
        v.extend(fallback.iter().cloned());
        v.extend(defaults.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, chain("manrope400"));
    fonts.families.insert(FontFamily::Name("semibold".into()), chain("manrope600"));
    fonts.families.insert(FontFamily::Name("bold".into()), chain("manrope700"));
    fonts.families.insert(FontFamily::Name("display".into()), chain("unbounded800"));
    fonts.families.insert(FontFamily::Name("display7".into()), chain("unbounded700"));
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    let mut v = Visuals::dark();
    v.panel_fill = BG();
    v.window_fill = SIDEBAR();
    v.extreme_bg_color = INPUT();
    v.faint_bg_color = SURFACE();
    v.override_text_color = Some(TEXT());
    v.selection.bg_fill = ACCENT();
    v.selection.stroke = Stroke::new(1.0_f32, ON_ACCENT());
    v.hyperlink_color = ACCENT_TEXT();
    v.window_corner_radius = CornerRadius::same(28);
    v.window_stroke = Stroke::new(1.0_f32, BORDER());
    v.menu_corner_radius = CornerRadius::same(12);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(12);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, SIDEBAR_EDGE());
    v.widgets.inactive.weak_bg_fill = INPUT();
    v.widgets.inactive.bg_fill = INPUT();
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER_STRONG());
    v.widgets.hovered.weak_bg_fill = SELECTED();
    v.widgets.hovered.bg_fill = SELECTED();
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, BORDER_STRONG());
    v.widgets.active.weak_bg_fill = SELECTED();
    v.widgets.active.bg_fill = SELECTED();
    v.widgets.open.weak_bg_fill = INPUT();
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT());
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT());
    v.slider_trailing_fill = true;
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: C::from_black_alpha(120) };
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.button_padding = egui::vec2(16.0, 12.0);
        s.spacing.interact_size.y = 44.0;
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.slider_width = 220.0;
        s.spacing.combo_width = 300.0;
        s.text_styles.insert(egui::TextStyle::Body, body(15.0));
        s.text_styles.insert(egui::TextStyle::Button, semibold(14.0));
        s.text_styles.insert(egui::TextStyle::Small, body(13.0));
    });
}
