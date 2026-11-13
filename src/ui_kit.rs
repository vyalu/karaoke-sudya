//! Reusable widgets for the «тёмная сцена» design: buttons, icons, badges,
//! cards, segmented control, level bars, score ring.

use crate::theme::*;
use eframe::egui::{self, epaint::Mesh, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2};
use std::f32::consts::{PI, TAU};

/* ---------------- icons (line, stroke 2) ---------------- */

#[derive(Clone, Copy, PartialEq)]
pub enum Icon { Mic, Search, Speaker, Headphones, Back, Stop, Repeat, Close, Play, Pause, Trash, Star, User, Folder, Sliders, Warning, External, EyeOff, Eye, Expand }

fn arc(c: Pos2, r: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    let n = ((a1 - a0).abs() / 0.12).ceil().max(4.0) as usize;
    (0..=n).map(|i| c + Vec2::angled(a0 + (a1 - a0) * i as f32 / n as f32) * r).collect()
}

pub fn icon(p: &egui::Painter, ic: Icon, c: Pos2, size: f32, col: Color32) {
    let k = size / 20.0;
    let st = Stroke::new((2.0 * k).max(1.5), col);
    let v = |x: f32, y: f32| c + Vec2::new(x * k, y * k);
    match ic {
        Icon::Mic => {
            p.rect_stroke(Rect::from_min_max(v(-3.5, -9.0), v(3.5, 3.0)), CornerRadius::same((3.5 * k) as u8), st, egui::StrokeKind::Middle);
            p.add(Shape::line(arc(v(0.0, -0.5), 7.0 * k, 0.0, PI), st));
            p.line_segment([v(0.0, 6.5), v(0.0, 9.5)], st);
            p.line_segment([v(-3.5, 9.5), v(3.5, 9.5)], st);
        }
        Icon::Search => {
            p.circle_stroke(v(-2.0, -2.0), 6.0 * k, st);
            p.line_segment([v(2.5, 2.5), v(8.0, 8.0)], st);
        }
        Icon::Speaker => {
            p.add(Shape::closed_line(vec![v(-8.0, -3.0), v(-4.0, -3.0), v(1.0, -7.5), v(1.0, 7.5), v(-4.0, 3.0), v(-8.0, 3.0)], st));
            p.add(Shape::line(arc(v(1.0, 0.0), 4.5 * k, -0.9, 0.9), st));
            p.add(Shape::line(arc(v(1.0, 0.0), 8.0 * k, -0.9, 0.9), st));
        }
        Icon::Headphones => {
            p.add(Shape::line(arc(v(0.0, 2.0), 8.0 * k, PI, TAU), st));
            p.line_segment([v(-8.0, 2.0), v(-8.0, 6.0)], st);
            p.line_segment([v(8.0, 2.0), v(8.0, 6.0)], st);
            p.rect_stroke(Rect::from_min_max(v(-9.0, 2.0), v(-4.5, 9.0)), CornerRadius::same((2.0 * k) as u8), st, egui::StrokeKind::Middle);
            p.rect_stroke(Rect::from_min_max(v(4.5, 2.0), v(9.0, 9.0)), CornerRadius::same((2.0 * k) as u8), st, egui::StrokeKind::Middle);
        }
        Icon::Back => {
            p.line_segment([v(8.0, 0.0), v(-7.0, 0.0)], st);
            p.add(Shape::line(vec![v(-1.0, -6.0), v(-7.0, 0.0), v(-1.0, 6.0)], st));
        }
        Icon::Stop => { p.rect_filled(Rect::from_center_size(c, Vec2::splat(12.0 * k)), CornerRadius::same((2.5 * k) as u8), col); }
        Icon::Star => star(p, c, 9.0 * k, col),
        Icon::User => {
            p.circle_stroke(v(0.0, -4.0), 4.5 * k, st);
            p.add(Shape::line(arc(v(0.0, 10.0), 8.5 * k, PI * 1.05, PI * 1.95), st));
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(v(-6.0, -7.0), v(-1.5, 7.0)), CornerRadius::same((1.5 * k) as u8), col);
            p.rect_filled(Rect::from_min_max(v(1.5, -7.0), v(6.0, 7.0)), CornerRadius::same((1.5 * k) as u8), col);
        }
        Icon::Trash => {
            p.line_segment([v(-8.0, -5.5), v(8.0, -5.5)], st);
            p.add(Shape::line(vec![v(-3.0, -5.5), v(-3.0, -8.0), v(3.0, -8.0), v(3.0, -5.5)], st));
            p.add(Shape::line(vec![v(-6.0, -5.5), v(-5.0, 8.0), v(5.0, 8.0), v(6.0, -5.5)], st));
            p.line_segment([v(-1.5, -2.0), v(-1.5, 5.0)], st);
            p.line_segment([v(1.5, -2.0), v(1.5, 5.0)], st);
        }
        Icon::Play => { p.add(Shape::convex_polygon(vec![v(-4.5, -7.0), v(7.0, 0.0), v(-4.5, 7.0)], col, Stroke::NONE)); }
        Icon::Repeat => {
            p.add(Shape::line(arc(c, 7.0 * k, -PI * 0.35, PI * 1.45), st));
            let tip = c + Vec2::angled(-PI * 0.35) * 7.0 * k;
            p.add(Shape::line(vec![tip + Vec2::new(-4.5 * k, -1.5 * k), tip, tip + Vec2::new(-0.5 * k, 4.5 * k)], st));
        }
        Icon::Close => {
            p.line_segment([v(-6.0, -6.0), v(6.0, 6.0)], st);
            p.line_segment([v(-6.0, 6.0), v(6.0, -6.0)], st);
        }
        Icon::Folder => {
            p.add(Shape::closed_line(vec![v(-9.0, -6.0), v(-3.0, -6.0), v(-1.0, -3.5), v(9.0, -3.5), v(9.0, 7.0), v(-9.0, 7.0)], st));
        }
        Icon::Sliders => {
            for (y, x) in [(-5.5f32, 3.0f32), (0.0, -3.0), (5.5, 1.0)] {
                p.line_segment([v(-9.0, y), v(9.0, y)], st);
                p.circle_filled(v(x, y), 2.6 * k, col);
            }
        }
        Icon::Warning => {
            p.add(Shape::closed_line(vec![v(0.0, -8.5), v(9.0, 7.5), v(-9.0, 7.5)], st));
            p.line_segment([v(0.0, -2.5), v(0.0, 2.0)], st);
            p.circle_filled(v(0.0, 4.8), 1.3 * k, col);
        }
        Icon::External => {
            p.add(Shape::line(vec![v(1.0, -8.0), v(8.0, -8.0), v(8.0, -1.0)], st));
            p.line_segment([v(8.0, -8.0), v(-1.0, 1.0)], st);
            p.add(Shape::line(vec![v(-2.0, -6.0), v(-8.0, -6.0), v(-8.0, 8.0), v(6.0, 8.0), v(6.0, 2.0)], st));
        }
        Icon::Eye | Icon::EyeOff => {
            p.add(Shape::line(arc(v(0.0, 7.0), 12.0 * k, -PI * 0.78, -PI * 0.22), st));
            p.add(Shape::line(arc(v(0.0, -7.0), 12.0 * k, PI * 0.22, PI * 0.78), st));
            p.circle_stroke(c, 2.6 * k, st);
            if ic == Icon::EyeOff { p.line_segment([v(-8.0, -8.0), v(8.0, 8.0)], st); }
        }
        Icon::Expand => {
            for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                p.add(Shape::line(vec![v(8.0 * sx, 3.0 * sy), v(8.0 * sx, 8.0 * sy), v(3.0 * sx, 8.0 * sy)], st));
            }
        }
    }
}

pub fn star(p: &egui::Painter, c: Pos2, r: f32, col: Color32) {
    let pt = |i: usize, rr: f32| c + Vec2::angled(-PI / 2.0 + i as f32 * PI / 5.0) * rr;
    let inner: Vec<Pos2> = (0..5).map(|i| pt(i * 2 + 1, r * 0.42)).collect();
    p.add(Shape::convex_polygon(inner.clone(), col, Stroke::NONE));
    for i in 0..5 {
        let tip = pt(i * 2, r);
        p.add(Shape::convex_polygon(vec![inner[(i + 4) % 5], tip, inner[i]], col, Stroke::NONE));
    }
}

/* ---------------- text helpers ---------------- */

pub fn galley(ui: &Ui, text: &str, font: FontId, col: Color32, wrap: f32, rows: usize) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font, col, wrap);
    if rows > 0 { job.wrap.max_rows = rows; job.wrap.overflow_character = Some('…'); }
    ui.fonts(|f| f.layout_job(job))
}

/// Wrapped label with chosen font / colour / max width.
pub fn text(ui: &mut Ui, s: &str, font: FontId, col: Color32, width: f32) -> Response {
    let g = galley(ui, s, font, col, width, 0);
    let (r, resp) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(r.min, g, col);
    resp
}

/// Upper-case section caption with letter spacing (12 px, muted).
pub fn section_label(ui: &mut Ui, s: &str) {
    let mut job = egui::text::LayoutJob::default();
    job.append(s, 0.0, egui::TextFormat { font_id: bold(12.0), color: MUTED(), extra_letter_spacing: 1.4, ..Default::default() });
    let g = ui.fonts(|f| f.layout_job(job));
    let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(r.min, g, MUTED());
}

/* ---------------- buttons ---------------- */

#[derive(Clone, Copy, PartialEq)]
pub enum Kind { Primary, Ghost }

pub fn button_ex(ui: &mut Ui, kind: Kind, label: &str, ic: Option<Icon>, min: Vec2, font: FontId, radius: u8) -> Response {
    let g = galley(ui, label, font, Color32::WHITE, f32::INFINITY, 1);
    let icon_w = if ic.is_some() { 26.0 } else { 0.0 };
    let w = (g.size().x + icon_w + 32.0).max(min.x);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, min.y.max(44.0)), Sense::click());
    let hov = resp.hovered();
    let p = ui.painter();
    let (fill, fg, stroke) = match kind {
        Kind::Primary => (if hov { mix(ACCENT(), Color32::WHITE, 0.10) } else { ACCENT() }, ON_ACCENT(), Stroke::NONE),
        Kind::Ghost => (if hov { SELECTED() } else { Color32::TRANSPARENT }, TEXT(), Stroke::new(1.0_f32, BORDER_STRONG())),
    };
    let fill = if resp.is_pointer_button_down_on() { mix(fill, Color32::BLACK, 0.08) } else { fill };
    p.rect(rect, CornerRadius::same(radius), fill, stroke, egui::StrokeKind::Inside);
    let total = g.size().x + icon_w;
    let mut x = rect.center().x - total / 2.0;
    if let Some(ic) = ic { icon(p, ic, Pos2::new(x + 9.0, rect.center().y), 18.0, fg); x += icon_w; }
    p.galley(Pos2::new(x, rect.center().y - g.size().y / 2.0), g, fg);
    if hov { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
    resp
}
pub fn primary(ui: &mut Ui, label: &str, ic: Option<Icon>) -> Response { button_ex(ui, Kind::Primary, label, ic, Vec2::new(0.0, 44.0), bold(15.0), 12) }
pub fn ghost(ui: &mut Ui, label: &str, ic: Option<Icon>) -> Response { button_ex(ui, Kind::Ghost, label, ic, Vec2::new(0.0, 44.0), semibold(14.0), 12) }

/// Square icon-only button (e.g. close 44×44).
pub fn icon_button(ui: &mut Ui, ic: Icon, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(12), if resp.hovered() { SELECTED() } else { Color32::TRANSPARENT }, Stroke::new(1.0_f32, BORDER_STRONG()), egui::StrokeKind::Inside);
    icon(p, ic, rect.center(), 18.0, TEXT());
    if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
    resp
}

/// Text link in accent colour.
pub fn link(ui: &mut Ui, label: &str) -> Response {
    let g = galley(ui, label, semibold(13.0), ACCENT_TEXT(), f32::INFINITY, 1);
    let (r, resp) = ui.allocate_exact_size(g.size() + Vec2::new(0.0, 4.0), Sense::click());
    let col = if resp.hovered() { mix(ACCENT_TEXT(), Color32::WHITE, 0.25) } else { ACCENT_TEXT() };
    ui.painter().galley(r.min, g, col);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0_f32, col));
    }
    resp
}

/* ---------------- containers ---------------- */

pub fn card<R>(ui: &mut Ui, fill: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new().fill(fill).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(18.0).inner_margin(egui::Margin::symmetric(20, 16))
        .show(ui, |ui| ui.with_layout(egui::Layout::top_down(egui::Align::Min), add).inner).inner
}

/// Pill badge with optional dot or icon.
pub fn badge(ui: &mut Ui, label: &str, bg: Color32, fg: Color32, dot: Option<Color32>, ic: Option<Icon>) -> Response {
    let g = galley(ui, label, semibold(13.0), fg, f32::INFINITY, 1);
    let lead = if dot.is_some() || ic.is_some() { 20.0 } else { 0.0 };
    let (r, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + lead + 28.0, 32.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(16), bg);
    let mut x = r.left() + 14.0;
    if let Some(d) = dot { p.circle_filled(Pos2::new(x + 4.0, r.center().y), 4.0, d); x += lead; }
    if let Some(ic) = ic { icon(p, ic, Pos2::new(x + 6.0, r.center().y), 14.0, fg); x += lead; }
    p.galley(Pos2::new(x, r.center().y - g.size().y / 2.0), g, fg);
    resp
}

/// Amber warning plaque.
pub fn warning(ui: &mut Ui, msg: &str, width: f32) {
    egui::Frame::new().fill(AMBER_WARN_BG()).corner_radius(14.0).inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
        ui.set_width(width - 28.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (r, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
            icon(ui.painter(), Icon::Warning, r.center(), 18.0, AMBER());
            text(ui, msg, body(13.0), AMBER_WARN_TEXT(), width - 62.0);
        });
    });
}

/// Rounded rect with dashed border.
pub fn dashed_rect(p: &egui::Painter, r: Rect, col: Color32) {
    let pts = vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for s in Shape::dashed_line(&pts, Stroke::new(1.0_f32, col), 6.0, 5.0) { p.add(s); }
}

/* ---------------- segmented: Колонки / Наушники ---------------- */

/// Returns true if the value changed.
pub fn segmented(ui: &mut Ui, speakers: &mut bool, enabled: bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(300.0, 44.0), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, CornerRadius::same(12), SEGMENT_BG());
    let halves = [Rect::from_min_max(rect.min, rect.center_bottom()), Rect::from_min_max(rect.center_top(), rect.max)];
    let mut changed = false;
    for (i, (label, ic, val)) in [("Колонки", Icon::Speaker, true), ("Наушники", Icon::Headphones, false)].into_iter().enumerate() {
        let r = halves[i].shrink(4.0);
        let resp = ui.interact(r, ui.id().with(("seg", i)), if enabled { Sense::click() } else { Sense::hover() });
        let on = *speakers == val;
        if on { p.rect_filled(r, CornerRadius::same(9), SELECTED()); }
        else if resp.hovered() && enabled { p.rect_filled(r, CornerRadius::same(9), Color32::from_white_alpha(6)); }
        let col = if on { TEXT() } else { MUTED() };
        let g = galley(ui, label, semibold(14.0), col, f32::INFINITY, 1);
        let total = 24.0 + g.size().x;
        let x = r.center().x - total / 2.0;
        icon(&p, ic, Pos2::new(x + 8.0, r.center().y), 16.0, col);
        p.galley(Pos2::new(x + 24.0, r.center().y - g.size().y / 2.0), g, col);
        if resp.clicked() && !on { *speakers = val; changed = true; }
        if resp.hovered() && enabled && !on { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
    }
    changed
}

/* ---------------- switch ---------------- */

/// Toggle row: title + description on the left, switch on the right. Returns true if toggled.
pub fn switch_row(ui: &mut Ui, title: &str, desc: &str, on: &mut bool, enabled: bool) -> bool {
    let w = ui.available_width();
    let g1 = galley(ui, title, semibold(15.0), TEXT(), w - 70.0, 0);
    let g2 = galley(ui, desc, body(13.0), MUTED(), w - 70.0, 0);
    let h = (g1.size().y + 2.0 + g2.size().y).max(44.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    let fade = if enabled { 1.0 } else { 0.45 };
    let h1 = g1.size().y;
    p.galley(rect.left_top(), g1, TEXT().gamma_multiply(fade));
    p.galley(rect.left_top() + Vec2::new(0.0, h1 + 2.0), g2, MUTED().gamma_multiply(fade));
    let track = Rect::from_min_size(Pos2::new(rect.right() - 48.0, rect.top() + 2.0), Vec2::new(48.0, 28.0));
    p.rect_filled(track, CornerRadius::same(14), if *on { ACCENT().gamma_multiply(fade) } else { SELECTED() });
    let kx = if *on { track.right() - 14.0 } else { track.left() + 14.0 };
    p.circle_filled(Pos2::new(kx, track.center().y), 10.0, if *on { ON_ACCENT() } else { MUTED() });
    if enabled && resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
    if enabled && resp.clicked() { *on = !*on; return true; }
    false
}

/* ---------------- level bars & score ring ---------------- */

/// Vertical bars (history of the voice level). `hist` = (level 0..1, voice detected).
pub fn level_bars(ui: &mut Ui, hist: &[(f32, bool)], n: usize, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let p = ui.painter();
    let gap = 4.0;
    let bw = ((rect.width() - gap * (n as f32 - 1.0)) / n as f32).max(2.0);
    let start = hist.len().saturating_sub(n);
    for i in 0..n {
        let (lv, on) = hist.get(start + i).copied().unwrap_or((0.0, false));
        let h = (lv.clamp(0.0, 1.0) * height).max(6.0);
        let x = rect.left() + i as f32 * (bw + gap);
        let r = Rect::from_min_max(Pos2::new(x, rect.bottom() - h), Pos2::new(x + bw, rect.bottom()));
        p.rect_filled(r, CornerRadius::same(3), if on { ACCENT() } else { BAR_OFF() });
    }
}

/// Horizontal segmented meter (mic test): 0..1.
pub fn segment_meter(ui: &mut Ui, level: f32, n: usize, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let p = ui.painter();
    let gap = 3.0;
    let w = (rect.width() - gap * (n as f32 - 1.0)) / n as f32;
    let lit = (level.clamp(0.0, 1.0) * n as f32).round() as usize;
    for i in 0..n {
        let x = rect.left() + i as f32 * (w + gap);
        let r = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(w, height));
        let col = if i < lit { if i >= n * 3 / 4 { AMBER() } else { OK() } } else { SELECTED() };
        p.rect_filled(r, CornerRadius::same(3), col);
    }
}


/// Elliptical glow (wider than tall).
pub fn glow_ellipse(p: &egui::Painter, c: Pos2, rx: f32, ry: f32, col: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(c, col);
    let n = 64;
    for i in 0..=n {
        let a = i as f32 / n as f32 * TAU;
        m.colored_vertex(c + Vec2::new(a.cos() * rx, a.sin() * ry), Color32::TRANSPARENT);
    }
    for i in 1..=n as u32 { m.add_triangle(0, i, i + 1); }
    p.add(Shape::mesh(m));
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn progress(ui: &mut Ui, frac: f32, height: f32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same((height / 2.0) as u8), SELECTED());
    let mut f = rect; f.set_width(rect.width() * frac.clamp(0.0, 1.0));
    if f.width() > 1.0 { p.rect_filled(f, CornerRadius::same((height / 2.0) as u8), ACCENT()); }
    rect
}

