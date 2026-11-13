//! «Бумбокс Бориска» — ворчливый болельщик. Drawn with the egui painter from
//! the same 200×200 geometry as the design canvas; poses blend smoothly.

use crate::theme::*;
use eframe::egui::{self, epaint::QuadraticBezierShape, Color32, CornerRadius, Pos2, Rect, Shape, Stroke, Vec2};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mood { Idle, Shh, Joy, Support, Grumpy }

type Q = [Pos2; 3];
const fn p(x: f32, y: f32) -> Pos2 { Pos2::new(x, y) }
fn line(a: Pos2, b: Pos2) -> Q { [a, p((a.x + b.x) / 2.0, (a.y + b.y) / 2.0), b] }

#[derive(Clone, Copy)]
struct Pose {
    lx: f32, rx: f32, ey: f32, iris: f32, lids: f32,
    brow_l: Q, brow_r: Q,
    mouth_up: Q, mouth_lo: Q, // lower == upper → closed mouth (a stroke)
    arm_l: Q, arm_r: Q,
    notes: f32,
}

fn pose(m: Mood) -> Pose {
    let arms_down = ([p(28.0, 128.0), p(14.0, 142.0), p(16.0, 158.0)], [p(172.0, 128.0), p(186.0, 142.0), p(184.0, 158.0)]);
    match m {
        Mood::Idle => Pose {
            lx: 64.0, rx: 136.0, ey: 124.0, iris: 10.0, lids: 1.0,
            brow_l: line(p(42.0, 98.0), p(84.0, 102.0)), brow_r: line(p(116.0, 102.0), p(158.0, 98.0)),
            mouth_up: [p(86.0, 154.0), p(100.0, 158.0), p(116.0, 150.0)], mouth_lo: [p(86.0, 154.0), p(100.0, 158.0), p(116.0, 150.0)],
            arm_l: arms_down.0, arm_r: arms_down.1, notes: 0.0,
        },
        Mood::Shh => Pose {
            lx: 70.0, rx: 142.0, ey: 122.0, iris: 9.0, lids: 0.35,
            brow_l: line(p(42.0, 92.0), p(84.0, 100.0)), brow_r: line(p(116.0, 100.0), p(158.0, 92.0)),
            mouth_up: line(p(88.0, 156.0), p(112.0, 156.0)), mouth_lo: line(p(88.0, 156.0), p(112.0, 156.0)),
            arm_l: arms_down.0, arm_r: [p(172.0, 126.0), p(190.0, 108.0), p(176.0, 92.0)], notes: 0.0,
        },
        Mood::Joy => Pose {
            lx: 64.0, rx: 136.0, ey: 120.0, iris: 13.0, lids: 0.0,
            brow_l: [p(44.0, 90.0), p(63.0, 82.0), p(84.0, 90.0)], brow_r: [p(116.0, 90.0), p(137.0, 82.0), p(156.0, 90.0)],
            mouth_up: line(p(80.0, 148.0), p(120.0, 148.0)), mouth_lo: [p(80.0, 148.0), p(100.0, 184.0), p(120.0, 148.0)],
            arm_l: [p(28.0, 124.0), p(10.0, 108.0), p(12.0, 88.0)], arm_r: [p(172.0, 124.0), p(190.0, 108.0), p(188.0, 88.0)], notes: 1.0,
        },
        Mood::Support => Pose {
            lx: 62.0, rx: 134.0, ey: 126.0, iris: 9.0, lids: 1.0,
            brow_l: line(p(44.0, 102.0), p(84.0, 94.0)), brow_r: line(p(116.0, 94.0), p(156.0, 102.0)),
            mouth_up: [p(88.0, 154.0), p(100.0, 150.0), p(112.0, 154.0)], mouth_lo: [p(88.0, 154.0), p(100.0, 150.0), p(112.0, 154.0)],
            arm_l: arms_down.0, arm_r: [p(172.0, 128.0), p(190.0, 124.0), p(194.0, 106.0)], notes: 0.0,
        },
        Mood::Grumpy => Pose {
            lx: 66.0, rx: 138.0, ey: 126.0, iris: 8.0, lids: 0.75,
            brow_l: line(p(42.0, 94.0), p(84.0, 104.0)), brow_r: line(p(116.0, 104.0), p(158.0, 94.0)),
            mouth_up: [p(86.0, 158.0), p(100.0, 150.0), p(114.0, 158.0)], mouth_lo: [p(86.0, 158.0), p(100.0, 150.0), p(114.0, 158.0)],
            arm_l: [p(28.0, 130.0), p(46.0, 150.0), p(66.0, 150.0)], arm_r: [p(172.0, 130.0), p(154.0, 150.0), p(134.0, 150.0)], notes: 0.0,
        },
    }
}

/// Live singing input: how open (loudness 0..1) and how high (−1 low … +1 high, relative to the singer).
#[derive(Clone, Copy, Default)]
pub struct Sing { pub open: f32, pub height: f32 }

/// Boris singing along: face follows the singer's note.
fn sing_pose(base: &Pose, o: f32, h: f32) -> Pose {
    let hu = h.max(0.0);
    let hd = (-h).max(0.0);
    let mw = 26.0 - 6.0 * h;
    let mh = 6.0 + 18.0 * o + 4.0 * hu;
    let down_r: Q = [p(172.0, 128.0), p(186.0, 142.0), p(184.0, 158.0)];
    let up_r: Q = [p(172.0, 124.0), p(194.0, 104.0), p(186.0, 76.0)];
    let chest_r: Q = [p(172.0, 130.0), p(150.0, 150.0), p(130.0, 146.0)];
    let arm_r = lq(lq(down_r, up_r, hu), chest_r, hd);
    Pose {
        lx: 64.0, rx: 136.0, ey: 124.0 - 6.0 * h, iris: 10.0 + 3.5 * hu, lids: 1.1 * hd,
        // neutral → surprised arch (high) → soulful, inner ends raised (low)
        brow_l: lq(lq([p(42.0, 97.0), p(63.0, 95.0), p(84.0, 97.0)], [p(42.0, 90.0), p(63.0, 78.0), p(84.0, 88.0)], hu), [p(42.0, 101.0), p(63.0, 98.0), p(84.0, 92.0)], hd),
        brow_r: lq(lq([p(116.0, 97.0), p(137.0, 95.0), p(158.0, 97.0)], [p(116.0, 88.0), p(137.0, 78.0), p(158.0, 90.0)], hu), [p(116.0, 92.0), p(137.0, 98.0), p(158.0, 101.0)], hd),
        mouth_up: [p(100.0 - mw / 2.0, 153.0), p(100.0, 153.0 - mh * 0.55), p(100.0 + mw / 2.0, 153.0)],
        mouth_lo: [p(100.0 - mw / 2.0, 153.0), p(100.0, 153.0 + mh * 1.15), p(100.0 + mw / 2.0, 153.0)],
        arm_l: base.arm_l, arm_r, notes: (0.35 + o * 0.65).min(1.0),
    }
}

fn lp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }
fn lq(a: Q, b: Q, t: f32) -> Q { [a[0].lerp(b[0], t), a[1].lerp(b[1], t), a[2].lerp(b[2], t)] }
fn blend(a: &Pose, b: &Pose, t: f32) -> Pose {
    Pose {
        lx: lp(a.lx, b.lx, t), rx: lp(a.rx, b.rx, t), ey: lp(a.ey, b.ey, t), iris: lp(a.iris, b.iris, t), lids: lp(a.lids, b.lids, t),
        brow_l: lq(a.brow_l, b.brow_l, t), brow_r: lq(a.brow_r, b.brow_r, t),
        mouth_up: lq(a.mouth_up, b.mouth_up, t), mouth_lo: lq(a.mouth_lo, b.mouth_lo, t),
        arm_l: lq(a.arm_l, b.arm_l, t), arm_r: lq(a.arm_r, b.arm_r, t), notes: lp(a.notes, b.notes, t),
    }
}

fn quad_pts(q: Q, n: usize) -> Vec<Pos2> {
    (0..=n).map(|i| {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        p(u * u * q[0].x + 2.0 * u * t * q[1].x + t * t * q[2].x, u * u * q[0].y + 2.0 * u * t * q[1].y + t * t * q[2].y)
    }).collect()
}

/* ---------------- phrases ---------------- */

/// What is happening right now (decides mood and what Boris says).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Situation { Home, Waiting, Intro, Singing, SingingGood, SingingBad, Silent, ResultTop, ResultGood, ResultMid, ResultLow, ResultZero }

fn phrases(s: Situation) -> &'static [&'static str] {
    match s {
        Situation::Home => &["Ну-ну, посмотрим…", "Выбирай песню. Я подожду.", "Удиви меня. Если сможешь."],
        Situation::Waiting => &["Ну-ну, посмотрим…", "Я готов. А ты?", "Включай уже свою музыку."],
        Situation::Intro => &["Тихо! Это вступление.", "Не спеши. Слушаем музыку.", "Тсс. Ещё рано."],
        Situation::Singing => &["Так-так…", "Продолжай. Я слушаю."],
        Situation::SingingGood => &["Ладно. Признаю. Неплохо.", "Хм. А ты умеешь.", "Вот это нота!", "Динамики аж дрогнули."],
        Situation::SingingBad => &["Это была нота или чих?", "Чуть точнее, маэстро.", "Мимо. Но с душой."],
        Situation::Silent => &["Ну? Пой давай!", "Я жду. Динамики остывают.", "Микрофон не кусается."],
        Situation::ResultTop => &["Ладно. Признаю. Это было сильно.", "Снимаю перед тобой антенну."],
        Situation::ResultGood => &["Ладно. Признаю. Неплохо.", "Хм. Я почти растрогался."],
        Situation::ResultMid => &["Сойдёт. Для начала.", "Неплохо. Но я видал и лучше."],
        Situation::ResultLow => &["Бывало и хуже. Давай ещё.", "Это была разминка, да?"],
        Situation::ResultZero => &["Я ничего не слышал. Микрофон включён?"],
    }
}

fn mood_of(s: Situation) -> Mood {
    match s {
        Situation::Home | Situation::Waiting | Situation::Singing => Mood::Idle,
        Situation::Intro => Mood::Shh,
        Situation::SingingGood | Situation::ResultTop | Situation::ResultGood => Mood::Joy,
        Situation::SingingBad | Situation::Silent | Situation::ResultZero => Mood::Grumpy,
        Situation::ResultMid => Mood::Idle,
        Situation::ResultLow => Mood::Support,
    }
}

fn chatty(s: Situation) -> f32 {
    match s {
        Situation::Singing => 0.0,                                  // stays quiet while you just sing
        Situation::Home => 14.0,
        Situation::SingingGood | Situation::SingingBad => 8.0,
        _ => 10.0,
    }
}

/* ---------------- the character ---------------- */

pub struct Boris {
    from: Pose,
    to: Mood,
    t: f32,
    situation: Option<Situation>,
    said_at: Option<Instant>,
    phrase: Option<(String, Instant)>,
    last_idx: usize,
    seed: u32,
    blink_at: f64,
    start: Instant,
    last_draw: f64,
    sing_amt: f32,
    sing_open: f32,
    sing_h: f32,
    queued: Option<String>,
    /// score sign held over his head (result screen)
    pub sign: Option<u32>,
    sign_t: f32,
}

impl Default for Boris {
    fn default() -> Self {
        Self { from: pose(Mood::Idle), to: Mood::Idle, t: 1.0, situation: None, said_at: None, phrase: None, last_idx: usize::MAX, seed: 0x9E37_79B9, blink_at: 3.0, start: Instant::now(), last_draw: 0.0, sing_amt: 0.0, sing_open: 0.0, sing_h: 0.0, queued: None, sign: None, sign_t: 0.0 }
    }
}

impl Boris {
    fn rand(&mut self) -> u32 { self.seed ^= self.seed << 13; self.seed ^= self.seed >> 17; self.seed ^= self.seed << 5; self.seed }

    fn current(&self) -> Pose { blend(&self.from, &pose(self.to), ease(self.t)) }

    fn say(&mut self, s: Situation) {
        let list = phrases(s);
        let mut i = (self.rand() as usize) % list.len();
        if list.len() > 1 && i == self.last_idx { i = (i + 1) % list.len(); }
        self.last_idx = i;
        self.phrase = Some((list[i].to_string(), Instant::now()));
        self.said_at = Some(Instant::now());
    }

    /// Say this next (memory, prediction…), instead of a stock phrase.
    pub fn queue(&mut self, text: String) { self.queued = Some(text); }

    /// Feed the current situation once per frame.
    pub fn update(&mut self, s: Situation, dt: f32) {
        if let Some(t) = self.queued.take() {
            self.situation = Some(s);
            self.phrase = Some((t, Instant::now()));
            self.said_at = Some(Instant::now());
        }
        let mood = mood_of(s);
        if mood != self.to { self.from = self.current(); self.to = mood; self.t = 0.0; }
        self.t = (self.t + dt / 0.28).min(1.0);
        let since = self.said_at.map(|t| t.elapsed().as_secs_f32()).unwrap_or(999.0);
        if self.situation != Some(s) {
            self.situation = Some(s);
            // react to a new situation, but never chatter more than every 2.5 s
            if since > 2.5 && s != Situation::Singing { self.say(s); }
        } else {
            let gap = chatty(s);
            if gap > 0.0 && since > gap { self.say(s); }
        }
    }

    pub fn phrase(&self) -> Option<(&str, f32)> {
        let (t, at) = self.phrase.as_ref()?;
        let el = at.elapsed().as_secs_f32();
        // longer phrases stay a bit longer
        let life = 4.5 + t.chars().count() as f32 * 0.04;
        if el > life { return None; }
        let a = (el / 0.2).min(1.0) * ((life - el) / 0.5).clamp(0.0, 1.0);
        Some((t.as_str(), a))
    }

    /// Draw Boris inside `rect` (square). `level` = voice level 0..1 (speakers pulse).
    pub fn draw(&mut self, painter: &egui::Painter, rect: Rect, level: f32, sing: Option<Sing>, music: f32) {
        let time = self.start.elapsed().as_secs_f64();
        let dt = ((time - self.last_draw) as f32).clamp(0.0, 0.1);
        self.last_draw = time;
        // smooth the singing input so the face moves fluidly
        let k = (dt * 10.0).min(1.0);
        let target = if sing.is_some() { 1.0 } else { 0.0 };
        self.sing_amt += (target - self.sing_amt) * (dt * if target > self.sing_amt { 9.0 } else { 3.0 }).min(1.0);
        if let Some(sg) = sing {
            self.sing_open += (sg.open.clamp(0.0, 1.0) - self.sing_open) * (dt * 18.0).min(1.0);
            self.sing_h += (sg.height.clamp(-1.0, 1.0) - self.sing_h) * k;
        } else {
            self.sing_open *= 1.0 - k;
        }
        self.sign_t = if self.sign.is_some() { (self.sign_t + dt / 0.45).min(1.0) } else { 0.0 };
        let mut ps = self.current();
        if self.sign.is_some() {
            let k = ease(self.sign_t);
            ps.arm_l = lq(ps.arm_l, [p(28.0, 124.0), p(2.0, 70.0), p(46.0, 6.0)], k);
            ps.arm_r = lq(ps.arm_r, [p(172.0, 124.0), p(198.0, 70.0), p(154.0, 6.0)], k);
        }
        if self.sing_amt > 0.01 {
            let sp = sing_pose(&ps, self.sing_open, self.sing_h);
            ps = blend(&ps, &sp, ease(self.sing_amt));
        }
        // blinking
        if time > self.blink_at {
            let k = ((time - self.blink_at) / 0.16) as f32;
            if k >= 1.0 { self.blink_at = time + 2.5 + (self.rand() % 3000) as f64 / 1000.0; }
            else { let close = 1.0 - (2.0 * k - 1.0).abs(); ps.lids = ps.lids.max(close * 1.95); }
        }
        let tf = time as f32;
        // sways to the music; bounces when happy
        let groove = music.clamp(0.0, 1.0) * (1.0 - self.sing_amt * 0.5);
        let bob = (tf * 2.2).sin() * (1.6 + groove * 3.5) + if self.to == Mood::Joy { (tf * 9.0).sin().abs() * -3.0 } else { 0.0 };
        // stretch up on high notes, squash on low ones (feet stay on the floor)
        let stretch = 1.0 + 0.07 * self.sing_h * self.sing_amt;
        let sway = if self.to == Mood::Joy { (tf * 8.0).sin() * 5.0 } else { (tf * 1.7).sin() * 1.2 };

        // with a sign the drawing spans y = −70…200 instead of 0…200
        let top = if self.sign.is_some() { -70.0 } else { 0.0 };
        let s = (rect.width() / 200.0).min(rect.height() / (200.0 - top));
        let o = Pos2::new(rect.center().x - 100.0 * s, rect.top() - top * s);
        let tp = |q: Pos2, dy: f32| o + Vec2::new(q.x * s, (186.0 - (186.0 - q.y) * stretch + dy) * s);
        let w = |x: f32| x * s;
        let white = TEXT();
        let st = |px: f32| Stroke::new(w(px), white);

        let quad = |q: Q, dy: f32, stroke: Stroke| {
            Shape::QuadraticBezier(QuadraticBezierShape::from_points_stroke([tp(q[0], dy), tp(q[1], dy), tp(q[2], dy)], false, Color32::TRANSPARENT, stroke))
        };
        // arms (behind the body)
        let mut al = ps.arm_l; al[2].y += sway; al[2].x -= sway * 0.3;
        let mut ar = ps.arm_r; ar[2].y -= sway; ar[2].x += sway * 0.3;
        painter.add(quad(al, bob, st(6.0)));
        painter.add(quad(ar, bob, st(6.0)));
        for e in [al[2], ar[2]] { painter.circle_filled(tp(e, bob), w(4.0), white); }
        // antenna + handle
        painter.line_segment([tp(p(150.0, 66.0), bob), tp(p(166.0 + sway * 0.4, 30.0), bob)], st(4.0));
        painter.circle_filled(tp(p(166.0 + sway * 0.4, 28.0), bob), w(5.0), AMBER());
        painter.add(Shape::line(vec![tp(p(62.0, 66.0), bob), tp(p(62.0, 54.0), bob), tp(p(66.0, 47.0), bob), tp(p(70.0, 46.0), bob), tp(p(130.0, 46.0), bob), tp(p(134.0, 47.0), bob), tp(p(138.0, 54.0), bob), tp(p(138.0, 66.0), bob)], st(6.0)));
        // legs + feet (do not bob)
        painter.line_segment([tp(p(76.0, 170.0), bob * 0.5), tp(p(74.0, 182.0), 0.0)], st(6.0));
        painter.line_segment([tp(p(124.0, 170.0), bob * 0.5), tp(p(126.0, 182.0), 0.0)], st(6.0));
        painter.rect_filled(Rect::from_min_size(tp(p(60.0, 180.0), 0.0), Vec2::new(w(26.0), w(9.0))), CornerRadius::same(w(4.5) as u8), AMBER());
        painter.rect_filled(Rect::from_min_size(tp(p(114.0, 180.0), 0.0), Vec2::new(w(26.0), w(9.0))), CornerRadius::same(w(4.5) as u8), AMBER());
        // body
        let body = Rect::from_min_size(tp(p(26.0, 66.0), bob), Vec2::new(w(148.0), w(106.0)));
        painter.rect(body, CornerRadius::same(w(16.0) as u8), BORDER(), st(4.0), egui::StrokeKind::Middle);
        painter.rect_filled(Rect::from_min_size(tp(p(42.0, 76.0), bob), Vec2::new(w(116.0), w(10.0))), CornerRadius::same(w(5.0) as u8), BORDER_STRONG());
        painter.circle_filled(tp(p(54.0, 81.0), bob), w(2.5), if level > 0.15 { ACCENT() } else { ACCENT().gamma_multiply(0.35) });
        painter.circle_filled(tp(p(64.0, 81.0), bob), w(2.5), MUTED());
        // speakers = eyes; cones pulse with the voice
        let pulse = level.clamp(0.0, 1.0) * 3.5;
        for (cx, ix) in [(64.0, ps.lx), (136.0, ps.rx)] {
            let c = tp(p(cx, 122.0), bob);
            painter.circle(c, w(26.0 + pulse * 0.3), SURFACE(), st(3.0));
            painter.circle_filled(tp(p(ix, ps.ey), bob), w(ps.iris + pulse), ACCENT());
            painter.circle_filled(tp(p(ix, ps.ey), bob), w(4.0), ON_ACCENT());
            // lids: cover the speaker from the top down to a chord
            let amt = ps.lids.clamp(0.0, 1.95) / 2.0; // 0.5 = half, ~1 = closed
            if amt > 0.02 {
                let r = 26.5f32;
                let dy = (-r + 2.0 * r * amt).clamp(-r + 0.01, r - 0.01);
                let a1 = (dy / r).asin();
                let a2 = std::f32::consts::PI - a1;
                let n = 40;
                let pts: Vec<Pos2> = (0..=n).map(|i| {
                    let a = a2 + (a1 + std::f32::consts::TAU - a2) * i as f32 / n as f32;
                    tp(p(cx + a.cos() * r, 122.0 + a.sin() * r), bob)
                }).collect();
                let hx = (r * r - dy * dy).max(0.0).sqrt();
                painter.add(Shape::convex_polygon(pts, BORDER(), Stroke::NONE));
                painter.line_segment([tp(p(cx - hx, 122.0 + dy), bob), tp(p(cx + hx, 122.0 + dy), bob)], st(3.0));
            }
        }
        // brows
        painter.add(quad(ps.brow_l, bob, st(6.0)));
        painter.add(quad(ps.brow_r, bob, st(6.0)));
        // mouth
        let open = ps.mouth_up.iter().zip(ps.mouth_lo.iter()).map(|(a, b)| (a.y - b.y).abs()).fold(0.0, f32::max);
        if open > 1.5 {
            let mut pts: Vec<Pos2> = quad_pts(ps.mouth_up, 12).into_iter().map(|q| tp(q, bob)).collect();
            let mut lo: Vec<Pos2> = quad_pts(ps.mouth_lo, 16).into_iter().map(|q| tp(q, bob)).collect();
            lo.reverse();
            pts.extend(lo);
            painter.add(Shape::convex_polygon(pts.clone(), ACCENT(), Stroke::NONE));
            painter.add(Shape::closed_line(pts, st(4.0)));
        } else {
            painter.add(quad(ps.mouth_up, bob, st(4.0)));
        }
        // the score sign, raised from behind his back
        if let Some(score) = self.sign {
            let k = ease(self.sign_t);
            let lift = (1.0 - k) * 60.0;
            let wob = (tf * 3.0).sin() * 1.5 * k;
            let board = Rect::from_min_max(tp(p(30.0, -64.0 + lift + wob), 0.0), tp(p(170.0, 6.0 + lift + wob), 0.0));
            painter.rect(board, CornerRadius::same(w(10.0) as u8), TEXT(), Stroke::new(w(4.0), BORDER()), egui::StrokeKind::Middle);
            let txt = if score == 0 { "?".to_string() } else { score.to_string() };
            painter.text(board.center() + Vec2::new(0.0, w(2.0)), egui::Align2::CENTER_CENTER, txt, display(w(46.0)), ON_ACCENT());
            // hands on the board
            for e in [p(46.0, 6.0 + lift + wob), p(154.0, 6.0 + lift + wob)] { painter.circle_filled(tp(e, 0.0), w(5.0), white); }
        }
        // music notes when happy
        if ps.notes > 0.02 {
            let a = (ps.notes * 255.0) as u8;
            let col = Color32::from_rgba_unmultiplied(AMBER().r(), AMBER().g(), AMBER().b(), a);
            for (i, (x, y)) in [(176.0f32, 52.0f32), (18.0, 60.0)].into_iter().enumerate() {
                let fl = ((tf * 1.3 + i as f32 * 0.7) % 1.0) * 10.0;
                let head = tp(p(x, y - fl), 0.0);
                painter.circle_filled(head, w(5.0), col);
                painter.line_segment([head + Vec2::new(w(4.5), 0.0), head + Vec2::new(w(4.5), -w(18.0))], Stroke::new(w(3.0), col));
                painter.line_segment([head + Vec2::new(w(4.5), -w(18.0)), head + Vec2::new(w(11.0), -w(12.0))], Stroke::new(w(3.0), col));
            }
        }
    }
}

fn ease(t: f32) -> f32 { let t = t.clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) }

/// Speech bubble with a tail pointing at `tail_to`. Text wraps to `max_w`.
/// Returns the bubble's size (for layout).
pub fn bubble_size(ui: &egui::Ui, text: &str, max_w: f32) -> Vec2 {
    crate::ui_kit::galley(ui, text, semibold(15.0), TEXT(), max_w - 28.0, 0).size() + Vec2::new(28.0, 22.0)
}

pub fn bubble(ui: &egui::Ui, painter: &egui::Painter, anchor: Pos2, tail_to: Pos2, text: &str, alpha: f32, max_w: f32) {
    let g = crate::ui_kit::galley(ui, text, semibold(15.0), TEXT(), max_w - 28.0, 0);
    let size = g.size() + Vec2::new(28.0, 22.0);
    let r = Rect::from_min_size(anchor, size);
    let fade = |c: Color32| c.gamma_multiply(alpha);
    // tail from the side facing Boris
    let (b1, b2) = if tail_to.x < r.left() {
        (p(r.left() + 1.0, r.center().y - 9.0), p(r.left() + 1.0, r.center().y + 9.0))
    } else {
        let cx = tail_to.x.clamp(r.left() + 20.0, r.right() - 20.0);
        let y = if tail_to.y < r.top() { r.top() + 1.0 } else { r.bottom() - 1.0 };
        (p(cx - 9.0, y), p(cx + 9.0, y))
    };
    painter.add(Shape::convex_polygon(vec![b1, b2, tail_to], fade(SURFACE_2()), Stroke::NONE));
    painter.line_segment([b1, tail_to], Stroke::new(1.0_f32, fade(BORDER_STRONG())));
    painter.line_segment([b2, tail_to], Stroke::new(1.0_f32, fade(BORDER_STRONG())));
    painter.rect(r, CornerRadius::same(14), fade(SURFACE_2()), Stroke::new(1.0_f32, fade(BORDER_STRONG())), egui::StrokeKind::Inside);
    painter.galley(r.min + Vec2::new(14.0, 11.0), g, fade(TEXT()));
}
