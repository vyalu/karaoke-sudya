//! Waveform pictures: the «окно голоса» look (mirrored magenta envelope on black,
//! cyan frame) and a small trim editor used by «Пародист» and «Озвучка».

use eframe::egui::{self, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Stroke, Vec2};
use crate::theme::*;
use crate::ui_kit::{self as kit, Icon, Kind};

pub const W_OUTER: Color32 = Color32::from_rgb(0x8A, 0x1D, 0x8F);
pub const W_INNER: Color32 = Color32::from_rgb(0xFF, 0x29, 0xF0);
pub const W_FRAME: Color32 = Color32::from_rgb(0x56, 0xCC, 0xF2);
pub const W_PITCH: Color32 = Color32::from_rgb(0xFF, 0x5A, 0x36);
pub const W_HEAD: Color32 = Color32::from_rgb(0xFF, 0x3B, 0x3B);
pub const W_YOU: Color32 = Color32::from_rgb(0x56, 0xCC, 0xF2);

/// Loudness outline of a sound: peak and RMS per bin, 0..1 (relative to the loudest bin).
#[derive(Clone, Default)]
pub struct Env { pub peak: Vec<f32>, pub rms: Vec<f32>, pub secs: f32 }

pub fn envelope(x: &[f32], sr: u32, bins: usize) -> Env {
    let secs = x.len() as f32 / sr.max(1) as f32;
    if x.is_empty() || bins == 0 { return Env { secs, ..Default::default() }; }
    let per = (x.len() as f32 / bins as f32).max(1.0);
    let mut peak = Vec::with_capacity(bins);
    let mut rms = Vec::with_capacity(bins);
    for b in 0..bins {
        let a = (b as f32 * per) as usize;
        let e = (((b + 1) as f32 * per) as usize).min(x.len());
        if a >= e { peak.push(0.0); rms.push(0.0); continue; }
        let c = &x[a..e];
        peak.push(c.iter().fold(0f32, |m, v| m.max(v.abs())));
        rms.push((c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt());
    }
    let m = peak.iter().cloned().fold(1e-6f32, f32::max);
    for v in peak.iter_mut().chain(rms.iter_mut()) { *v = (*v / m).min(1.0); }
    Env { peak, rms, secs }
}

/// Black box with the cyan frame and the white centre line.
pub fn frame(p: &Painter, r: Rect) {
    p.rect_filled(r, CornerRadius::same(10), Color32::BLACK);
    p.line_segment([Pos2::new(r.left() + 4.0, r.center().y), Pos2::new(r.right() - 4.0, r.center().y)], Stroke::new(1.5_f32, Color32::WHITE));
}
pub fn frame_border(p: &Painter, r: Rect) {
    p.rect_stroke(r, CornerRadius::same(10), Stroke::new(3.0_f32, W_FRAME), egui::StrokeKind::Inside);
}

/// Mirrored envelope of `env` between clip times `a`..`b`, drawn into `r`.
pub fn draw_env(p: &Painter, r: Rect, env: &Env, a: f32, b: f32, outer: Color32, inner: Color32) {
    if env.peak.is_empty() || b <= a { return; }
    let n = env.peak.len();
    let cols = (r.width() / 2.0).max(1.0) as usize;
    let h = r.height() / 2.0 - 4.0;
    for c in 0..cols {
        let t = a + (b - a) * c as f32 / cols as f32;
        let i = ((t / env.secs.max(1e-3)) * n as f32) as usize;
        if i >= n { break; }
        let x = r.left() + (c as f32 + 0.5) * r.width() / cols as f32;
        let (pk, rm) = (env.peak[i], (env.rms[i] * 1.6).min(1.0));
        p.line_segment([Pos2::new(x, r.center().y - pk * h), Pos2::new(x, r.center().y + pk * h)], Stroke::new(2.2_f32, outer));
        p.line_segment([Pos2::new(x, r.center().y - rm * h), Pos2::new(x, r.center().y + rm * h)], Stroke::new(2.2_f32, inner));
    }
}

/* ---------- trim editor ---------- */

#[derive(Clone, Copy, PartialEq)]
pub enum TrimAct { None, Play, Overwrite, SaveNew, Cancel }

#[derive(Clone)]
pub struct Trim { pub a: f32, pub b: f32, drag: Option<bool> }
impl Trim { pub fn new(a: f32, b: f32) -> Self { Self { a, b, drag: None } } }

fn ts(t: f32) -> String { format!("{}:{:04.1}", (t / 60.0) as u32, t % 60.0).replace('.', ",") }

/// A stretch `view` of the sound with two draggable edges (`t.a`, `t.b`, in seconds).
/// `lo`..`hi` limit where the edges may go. Returns true while/when they move.
pub fn range_ui(ui: &mut egui::Ui, width: f32, height: f32, env: &Env, view: (f32, f32), lo: f32, hi: f32, t: &mut Trim, head: Option<f32>) -> bool {
    let (v0, v1) = (view.0, view.1.max(view.0 + 0.1));
    let (r, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
    let p = ui.painter();
    frame(p, r);
    let inner = r.shrink2(Vec2::new(6.0, 4.0));
    draw_env(p, inner, env, v0, v1, W_OUTER, W_INNER);
    let x = |s: f32| inner.left() + inner.width() * ((s - v0) / (v1 - v0)).clamp(0.0, 1.0);
    let sx = |px: f32| v0 + ((px - inner.left()) / inner.width()).clamp(0.0, 1.0) * (v1 - v0);
    // dim what is left out
    let dim = Color32::from_black_alpha(170);
    p.rect_filled(Rect::from_min_max(r.min, Pos2::new(x(t.a), r.bottom())), CornerRadius::same(10), dim);
    p.rect_filled(Rect::from_min_max(Pos2::new(x(t.b), r.top()), r.max), CornerRadius::same(10), dim);
    for s in [t.a, t.b] {
        let hx = x(s);
        p.line_segment([Pos2::new(hx, r.top()), Pos2::new(hx, r.bottom())], Stroke::new(3.0_f32, W_FRAME));
        p.rect_filled(Rect::from_center_size(Pos2::new(hx, r.center().y), Vec2::new(10.0, 28.0)), CornerRadius::same(4), W_FRAME);
    }
    // a second ruler: small ticks every second
    let mut k = v0.ceil();
    while k < v1 { let tx = x(k); p.line_segment([Pos2::new(tx, r.bottom() - 7.0), Pos2::new(tx, r.bottom() - 2.0)], Stroke::new(1.0_f32, Color32::from_white_alpha(90))); k += 1.0; }
    if let Some(h) = head { if h >= v0 && h <= v1 { let hx = x(h); p.line_segment([Pos2::new(hx, r.top()), Pos2::new(hx, r.bottom())], Stroke::new(2.0_f32, W_HEAD)); } }
    frame_border(p, r);
    if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal); }
    let mut moved = false;
    if let Some(pos) = resp.interact_pointer_pos() {
        let s = sx(pos.x);
        if resp.drag_started() || resp.clicked() { t.drag = Some((s - t.a).abs() <= (s - t.b).abs()); }
        match t.drag {
            Some(true) => { t.a = s.min(t.b - 0.2).max(lo); moved = true; }
            Some(false) => { t.b = s.max(t.a + 0.2).min(hi); moved = true; }
            None => {}
        }
    }
    if resp.drag_stopped() || resp.clicked() { t.drag = None; }
    moved
}

/// The whole sound with two handles; returns what the user pressed.
/// `head` — playback position (seconds) to show, if something plays.
pub fn trim_ui(ui: &mut egui::Ui, width: f32, env: &Env, t: &mut Trim, head: Option<f32>) -> TrimAct {
    let mut act = TrimAct::None;
    let dur = env.secs.max(0.01);
    range_ui(ui, width, 96.0, env, (0.0, dur), 0.0, dur, t, head);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{} — {}  ·  {:.1} с", ts(t.a), ts(t.b), t.b - t.a).replace('.', ",")).font(semibold(13.0)).color(TEXT_2()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if kit::link(ui, "Отмена").clicked() { act = TrimAct::Cancel; }
        });
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if kit::button_ex(ui, Kind::Ghost, "Прослушать", Some(Icon::Play), Vec2::new(0.0, 38.0), semibold(13.0), 10).clicked() { act = TrimAct::Play; }
        if kit::button_ex(ui, Kind::Primary, "Перезаписать", None, Vec2::new(0.0, 38.0), bold(13.0), 10).on_hover_text("Заменить этот файл обрезанным").clicked() { act = TrimAct::Overwrite; }
        if kit::button_ex(ui, Kind::Ghost, "Сохранить как новый", Some(Icon::Folder), Vec2::new(0.0, 38.0), semibold(13.0), 10).on_hover_text("Оригинал останется, обрезка — отдельным файлом").clicked() { act = TrimAct::SaveNew; }
    });
    act
}

pub fn time_label(t: f32) -> String { ts(t) }

#[cfg(test)]
mod tests {
    #[test]
    fn envelope_shape() {
        let sr = 1000;
        let mut x = vec![0.0f32; 1000];
        x.extend((0..1000).map(|i| if i % 2 == 0 { 0.5 } else { -0.5 }));
        let e = super::envelope(&x, sr, 10);
        assert_eq!(e.peak.len(), 10);
        assert!((e.secs - 2.0).abs() < 1e-3);
        assert!(e.peak[2] < 0.01 && (e.peak[7] - 1.0).abs() < 1e-3 && e.rms[7] > 0.9);
    }
}
