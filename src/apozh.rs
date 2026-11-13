//! «АПОЖ» (песня задом наперёд, как в «Хороших шутках»): one player sings a bit of a song,
//! the recording is reversed and cut into pieces, the other player repeats every reversed
//! piece, and then his repeats are glued and reversed again — what song was it?

use super::*;
use crate::{mimic, wave};

/* ---------- pure helpers (tested) ---------- */

/// Reverse a sound.
pub fn reversed(x: &[f32]) -> Vec<f32> { x.iter().rev().copied().collect() }

/// Cut into pieces of about `len` seconds, each cut moved to the quietest spot nearby
/// (so words are cut less often). Returns sample ranges.
pub fn pieces(x: &[f32], sr: u32, len: f32) -> Vec<(usize, usize)> {
    let n = x.len();
    let step = (len * sr as f32) as usize;
    if n == 0 || step == 0 { return vec![]; }
    let win = (sr as f32 * 0.02) as usize;
    let mut out = Vec::new();
    let mut a = 0;
    while a < n {
        let target = a + step;
        if target + step / 3 >= n { out.push((a, n)); break; }
        // look ±30 % around the target for the quietest 20 ms
        let lo = target - step * 3 / 10;
        let hi = (target + step * 3 / 10).min(n - win);
        let cut = (lo..hi).step_by(win.max(1)).min_by(|&i, &j| {
            let e = |k: usize| x[k..k + win].iter().map(|v| v * v).sum::<f32>();
            e(i).total_cmp(&e(j))
        }).unwrap_or(target);
        out.push((a, cut));
        a = cut;
    }
    out
}

/// Cut silence off both ends (a repeat starts after a reaction pause).
pub fn trim_silence(x: &[f32], sr: u32) -> Vec<f32> {
    let win = (sr as f32 * 0.01) as usize;
    if x.len() < win * 4 { return x.to_vec(); }
    let peak = x.iter().fold(0f32, |m, v| m.max(v.abs()));
    let thr = (peak * 0.08).max(0.004);
    let loud = |c: &[f32]| c.iter().any(|v| v.abs() > thr);
    let chunks: Vec<&[f32]> = x.chunks(win).collect();
    let first = chunks.iter().position(|c| loud(c)).unwrap_or(0);
    let last = chunks.iter().rposition(|c| loud(c)).unwrap_or(chunks.len() - 1);
    let a = first.saturating_sub(2) * win;
    let b = ((last + 3) * win).min(x.len());
    x[a..b.max(a)].to_vec()
}

/// Glue pieces with tiny fades so the joins don't click.
pub fn glue(parts: &[Vec<f32>], sr: u32) -> Vec<f32> {
    let fade = (sr as f32 * 0.008) as usize;
    let mut out = Vec::new();
    for p in parts {
        let n = p.len();
        out.extend(p.iter().enumerate().map(|(i, v)| {
            let a = if i < fade { i as f32 / fade as f32 } else { 1.0 };
            let b = if n - i <= fade { (n - i) as f32 / fade as f32 } else { 1.0 };
            v * a * b
        }));
    }
    out
}

/* ---------- state ---------- */

#[derive(Clone, Copy, PartialEq)]
pub enum APhase { Setup, Sing, Repeat, Guess, Final }

pub struct Apozh {
    pub phase: APhase,
    players: Vec<String>,
    points: Vec<u32>,
    /// seconds per piece (difficulty)
    piece_len: f32,
    rounds: usize,
    round: usize,
    singer: usize,
    repeater: usize,
    pub mic: Option<audio::Recorder>,
    rec_since: Option<Instant>,
    heard: bool,
    quiet_since: Option<Instant>,
    /// the singer's recording
    song: Option<(Vec<f32>, u32)>,
    /// reversed song and its pieces
    rev: Vec<f32>,
    cuts: Vec<(usize, usize)>,
    cur: usize,
    /// the repeater's take for every piece
    takes: Vec<Option<Vec<f32>>>,
    take_sr: u32,
    /// the glued and reversed result
    result: Vec<f32>,
    similarity: u32,
    guessed: Option<bool>,
    play_from: Option<(Instant, f32, f32)>, // started, from, length (for the cursor)
    env: wave::Env,
}

impl Apozh {
    pub fn new() -> Self {
        Self {
            phase: APhase::Setup, players: vec![], points: vec![], piece_len: 2.5, rounds: 4, round: 0, singer: 0, repeater: 1,
            mic: None, rec_since: None, heard: false, quiet_since: None, song: None, rev: vec![], cuts: vec![], cur: 0,
            takes: vec![], take_sr: 48000, result: vec![], similarity: 0, guessed: None, play_from: None, env: wave::Env::default(),
        }
    }
    fn sr(&self) -> u32 { self.song.as_ref().map(|s| s.1).unwrap_or(48000) }
    fn piece(&self, k: usize) -> Vec<f32> { self.cuts.get(k).map(|&(a, b)| self.rev[a..b].to_vec()).unwrap_or_default() }
}

impl App {
    pub(crate) fn open_apozh(&mut self) {
        self.cancel();
        self.mic_test = None;
        if self.apozh.is_none() { self.apozh = Some(Apozh::new()); }
        if let Some(a) = &mut self.apozh { a.phase = APhase::Setup; }
        self.show_apozh = true;
    }

    pub(crate) fn exit_apozh(&mut self) {
        self.show_apozh = false;
        self.game_player = None;
        if let Some(a) = &mut self.apozh { a.mic = None; a.rec_since = None; a.phase = APhase::Setup; }
    }

    fn ap_play(&mut self, s: Vec<f32>, sr: u32) {
        let len = s.len() as f32 / sr.max(1) as f32;
        if let Some(a) = &mut self.apozh { a.play_from = Some((Instant::now(), 0.0, len)); }
        self.game_player = audio::Player::from_samples(s, sr, &None, 1.0).ok();
    }

    fn ap_start(&mut self) {
        let players = roster(&self.settings);
        if players.len() < 2 { self.say("Нужно хотя бы два игрока — добавьте в «Играют сегодня»"); return; }
        let Some(a) = &mut self.apozh else { return };
        a.points = vec![0; players.len()];
        a.players = players;
        a.round = 0;
        a.singer = 0;
        a.repeater = 1;
        if a.mic.is_none() {
            match audio::Recorder::mic(&self.settings.input, self.params.clone()) { Ok(m) => a.mic = Some(m), Err(e) => { self.say(e); return; } }
        }
        self.ap_new_round();
    }

    fn ap_new_round(&mut self) {
        let Some(a) = &mut self.apozh else { return };
        a.round += 1;
        a.song = None; a.rev.clear(); a.cuts.clear(); a.takes.clear(); a.result.clear(); a.guessed = None; a.cur = 0;
        a.phase = APhase::Sing;
        let (s, r) = (a.players[a.singer].clone(), a.players[a.repeater].clone());
        self.game_player = None;
        self.boris.queue(format!("Раунд {}! {s} поёт кусочек песни, {r} — не подглядывай… то есть не подслушивай!", self.apozh.as_ref().map(|a| a.round).unwrap_or(1)));
    }

    fn ap_rec_toggle(&mut self) {
        let Some(a) = &mut self.apozh else { return };
        if a.rec_since.take().is_some() {
            let (s, sr) = a.mic.as_ref().map(|m| (m.end(), m.sr)).unwrap_or_default();
            match a.phase {
                APhase::Sing => {
                    let s = trim_silence(&s, sr);
                    if (s.len() as f32) < sr as f32 * 1.0 { self.say("Слишком коротко — спойте хотя бы пару секунд"); return; }
                    a.rev = reversed(&s);
                    a.cuts = pieces(&a.rev, sr, a.piece_len);
                    a.takes = vec![None; a.cuts.len()];
                    a.take_sr = sr;
                    a.song = Some((s, sr));
                }
                APhase::Repeat => {
                    let t = trim_silence(&s, sr);
                    a.take_sr = sr;
                    let k = a.cur;
                    if let Some(slot) = a.takes.get_mut(k) { *slot = Some(t); }
                }
                _ => {}
            }
        } else if let Some(m) = &a.mic {
            m.begin();
            a.rec_since = Some(Instant::now());
            a.heard = false;
            a.quiet_since = None;
            self.game_player = None;
        } else {
            self.say("Микрофон не найден — проверьте его в «Звук»");
        }
    }

    /// Move to piece `k` and play it at once.
    fn ap_goto(&mut self, k: usize) {
        let Some(a) = &mut self.apozh else { return };
        a.cur = k.min(a.cuts.len());
        if a.cur < a.cuts.len() {
            let p = a.piece(a.cur);
            a.env = wave::envelope(&p, a.sr(), 300);
            let sr = a.sr();
            self.ap_play(p, sr);
        }
    }

    fn ap_finish_repeats(&mut self) {
        let Some(a) = &mut self.apozh else { return };
        let parts: Vec<Vec<f32>> = a.takes.iter().map(|t| t.clone().unwrap_or_default()).collect();
        let glued = glue(&parts, a.take_sr);
        a.result = reversed(&glued);
        // how close it came out to the original (intonation and rhythm)
        a.similarity = a.song.as_ref().map(|(s, sr)| mimic::compare(&mimic::features(s, *sr), &mimic::features(&a.result, a.take_sr)).total).unwrap_or(0);
        a.phase = APhase::Guess;
        let (r, sr) = (a.result.clone(), a.take_sr);
        let who = a.players[a.repeater].clone();
        self.ap_play(r, sr);
        self.boris.queue(format!("{who}, слушай, что у тебя получилось. Что это за песня?"));
    }

    fn ap_judge(&mut self, ok: bool) {
        let Some(a) = &mut self.apozh else { return };
        a.guessed = Some(ok);
        let (s, r) = (a.singer, a.repeater);
        if ok { a.points[r] += 1; a.points[s] += 1; }
        let sim = a.similarity;
        let line = if ok { format!("Угадал! Очко и тому, кто пел, и тому, кто повторял. Похожесть {sim} из 100.") } else { format!("Мимо! А ведь похожесть была {sim} из 100…") };
        self.boris.queue(line);
        if ok && self.settings.crowd_sfx { self.sfx = audio::Player::from_samples(sfx::applause(2.5, 20, 5), sfx::SR, &None, 0.7).ok(); }
    }

    fn ap_next(&mut self) {
        let Some(a) = &mut self.apozh else { return };
        let n = a.players.len();
        if a.round >= a.rounds.max(1) {
            a.phase = APhase::Final;
            let best = (0..n).max_by_key(|&i| a.points[i]).unwrap_or(0);
            let name = a.players[best].clone();
            self.boris.queue(format!("Главный по песням задом наперёд — {name}! Это шедевр!"));
            return;
        }
        // roles move on: the repeater sings next, the next player repeats
        a.singer = a.repeater;
        a.repeater = (a.singer + 1) % n;
        self.ap_new_round();
    }

    fn ap_tick(&mut self) {
        let mut stop = false;
        if let Some(a) = &mut self.apozh {
            if let Some(t0) = a.rec_since {
                let lvl = a.mic.as_ref().map(|m| m.level()).unwrap_or(0.0);
                if lvl > 0.025 { a.heard = true; a.quiet_since = None; }
                else if a.heard && lvl < 0.008 && a.quiet_since.is_none() { a.quiet_since = Some(Instant::now()); }
                else if lvl >= 0.008 { a.quiet_since = None; }
                let el = t0.elapsed().as_secs_f32();
                // the song itself: up to 20 s, stopped by hand; a repeat: stops itself after a pause
                let (max, pause) = match a.phase { APhase::Sing => (20.0, f32::MAX), _ => (a.piece_len * 2.5 + 3.0, 1.2) };
                if el > max || (a.quiet_since.map(|t| t.elapsed().as_secs_f32() > pause).unwrap_or(false) && el > 0.6) { stop = true; }
            }
            if a.play_from.map(|(t0, _, l)| t0.elapsed().as_secs_f32() > l + 0.1).unwrap_or(false) { a.play_from = None; }
        }
        if stop { self.ap_rec_toggle(); }
    }

    pub(crate) fn header_apozh(&mut self, ui: &mut egui::Ui) {
        let sub = self.apozh.as_ref().map(|a| match a.phase { APhase::Setup => "песня задом наперёд".to_string(), APhase::Final => "итоги".into(), _ => format!("раунд {} из {}", a.round, a.rounds) }).unwrap_or_default();
        ui.horizontal_centered(|ui| {
            if kit::ghost(ui, "Назад", Some(Icon::Back)).clicked() { self.exit_apozh(); return; }
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new("АПОЖ").font(display_bold(18.0)));
                ui.label(RichText::new(sub).font(body(13.0)).color(MUTED()));
            });
        });
    }

    pub(crate) fn apozh_screen(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.video_rect = None;
        self.ap_tick();
        ctx.request_repaint_after(Duration::from_millis(40));
        let ph = self.apozh.as_ref().map(|a| a.phase).unwrap_or(APhase::Setup);
        let mut act: Option<(u8, usize)> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| ui.vertical_centered(|ui| {
            ui.set_max_width(760.0);
            let a = self.apozh.as_mut().unwrap();
            let recording = a.rec_since.is_some();
            let big = Vec2::new(460.0, 72.0);
            match ph {
                APhase::Setup => {
                    ui.add_space(16.0);
                    ui.label(RichText::new("Песня задом наперёд").font(display(40.0)));
                    ui.add_space(6.0);
                    kit::text(ui, "Один поёт кусочек песни — без музыки. Программа переворачивает запись и режет на кусочки. Второй повторяет каждый перевёрнутый кусочек, а потом его повторы склеиваются и переворачиваются обратно. Что это была за песня?", body(16.0), TEXT_2(), 720.0);
                    ui.add_space(16.0);
                    egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(20.0).inner_margin(24.0).show(ui, |ui| {
                        ui.set_width(672.0);
                        kit::section_label(ui, "ИГРАЮТ СЕГОДНЯ");
                        ui.add_space(6.0);
                        if roster_editor(ui, &mut self.settings.players, 672.0) { act = Some((9, 0)); }
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Кусочки").font(semibold(15.0)));
                            for (lab, v) in [("Легко · 1,5 с", 1.5f32), ("Нормально · 2,5 с", 2.5), ("Сложно · 4 с", 4.0)] {
                                let on = (a.piece_len - v).abs() < 0.01;
                                if kit::button_ex(ui, if on { Kind::Primary } else { Kind::Ghost }, lab, None, Vec2::new(0.0, 40.0), bold(13.0), 10).clicked() { a.piece_len = v; }
                            }
                        });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Раундов").font(semibold(15.0)));
                            for n in [2usize, 4, 6] {
                                let on = a.rounds == n;
                                if kit::button_ex(ui, if on { Kind::Primary } else { Kind::Ghost }, &n.to_string(), None, Vec2::new(56.0, 40.0), bold(14.0), 10).clicked() { a.rounds = n; }
                            }
                        });
                    });
                    ui.add_space(16.0);
                    if kit::button_ex(ui, Kind::Primary, "Начать игру", Some(Icon::Play), Vec2::new(720.0, 60.0), bold(17.0), 16).clicked() { act = Some((1, 0)); }
                }
                APhase::Sing => {
                    let (s, r) = (a.players[a.singer].clone(), a.players[a.repeater].clone());
                    ui.add_space(24.0);
                    ui.label(RichText::new(format!("Поёт {s}")).font(display(38.0)).color(player_color(a.singer)));
                    ui.add_space(6.0);
                    kit::text(ui, &format!("Спойте 5–15 секунд любой известной песни — без музыки. {r} пока может слушать: всё равно потом всё будет задом наперёд."), body(16.0), TEXT_2(), 640.0);
                    ui.add_space(24.0);
                    if recording {
                        let lvl = a.mic.as_ref().map(|m| m.level()).unwrap_or(0.0);
                        let el = a.rec_since.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
                        ui.label(RichText::new(format!("● Запись {:.0} с", el)).font(bold(18.0)).color(REC_TEXT()));
                        let (lr, _) = ui.allocate_exact_size(Vec2::new(460.0, 10.0), egui::Sense::hover());
                        ui.painter().rect_filled(lr, CornerRadius::same(5), SURFACE_2());
                        ui.painter().rect_filled(Rect::from_min_size(lr.min, Vec2::new(lr.width() * (lvl * 4.0).min(1.0), lr.height())), CornerRadius::same(5), ACCENT());
                        ui.add_space(12.0);
                        if kit::button_ex(ui, Kind::Primary, "Стоп", Some(Icon::Stop), big, bold(21.0), 20).clicked() { act = Some((2, 0)); }
                    } else if a.song.is_some() {
                        ui.label(RichText::new(format!("Записано {:.1} с · кусочков: {}", a.song.as_ref().map(|s| s.0.len() as f32 / s.1 as f32).unwrap_or(0.0), a.cuts.len()).replace('.', ",")).font(semibold(15.0)).color(OK_TEXT()));
                        ui.add_space(12.0);
                        if kit::button_ex(ui, Kind::Primary, &format!("Перевернуть — повторяет {r}"), Some(Icon::Repeat), big, bold(19.0), 20).clicked() { act = Some((3, 0)); }
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(((ui.available_width() - 420.0) / 2.0).max(0.0));
                            if kit::button_ex(ui, Kind::Ghost, "Послушать", Some(Icon::Play), Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some((4, 0)); }
                            if kit::button_ex(ui, Kind::Ghost, "Перепеть", Some(Icon::Mic), Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some((2, 0)); }
                        });
                    } else if kit::button_ex(ui, Kind::Primary, &format!("Записать — {s}"), Some(Icon::Mic), big, bold(21.0), 20).clicked() { act = Some((2, 0)); }
                }
                APhase::Repeat => {
                    let r = a.players[a.repeater].clone();
                    let n = a.cuts.len();
                    ui.add_space(16.0);
                    ui.label(RichText::new(format!("Повторяет {r}")).font(display(34.0)).color(player_color(a.repeater)));
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("Кусочек {} из {}", (a.cur + 1).min(n), n)).font(semibold(16.0)).color(TEXT_2()));
                    ui.add_space(10.0);
                    // pieces strip
                    let (sr_, _) = ui.allocate_exact_size(Vec2::new(640.0, 14.0), egui::Sense::hover());
                    for k in 0..n {
                        let w = sr_.width() / n as f32;
                        let r = Rect::from_min_size(Pos2::new(sr_.left() + k as f32 * w + 2.0, sr_.top()), Vec2::new(w - 4.0, sr_.height()));
                        let c = if a.takes.get(k).map(|t| t.is_some()).unwrap_or(false) { player_color(a.repeater) } else if k == a.cur { ACCENT_SOFT() } else { SURFACE_2() };
                        ui.painter().rect_filled(r, CornerRadius::same(4), c);
                        if k == a.cur { ui.painter().rect_stroke(r, CornerRadius::same(4), Stroke::new(1.5_f32, TEXT()), egui::StrokeKind::Outside); }
                    }
                    ui.add_space(12.0);
                    // the reversed piece as a waveform
                    let (wr, _) = ui.allocate_exact_size(Vec2::new(640.0, 110.0), egui::Sense::hover());
                    wave::frame(ui.painter(), wr);
                    wave::draw_env(ui.painter(), wr.shrink2(Vec2::new(8.0, 6.0)), &a.env, 0.0, a.env.secs, wave::W_OUTER, wave::W_INNER);
                    if let Some((t0, _, l)) = a.play_from {
                        let x = wr.left() + 8.0 + (wr.width() - 16.0) * (t0.elapsed().as_secs_f32() / l.max(0.01)).min(1.0);
                        ui.painter().line_segment([Pos2::new(x, wr.top() + 3.0), Pos2::new(x, wr.bottom() - 3.0)], Stroke::new(2.5_f32, wave::W_HEAD));
                    }
                    wave::frame_border(ui.painter(), wr);
                    ui.add_space(18.0);
                    let has = a.takes.get(a.cur).map(|t| t.is_some()).unwrap_or(false);
                    if recording {
                        if kit::button_ex(ui, Kind::Primary, "Стоп", Some(Icon::Stop), big, bold(21.0), 20).clicked() { act = Some((2, 0)); }
                        ui.add_space(6.0);
                        ui.label(RichText::new("Повторяй как слышишь — остановится само").font(body(14.0)).color(REC_TEXT()));
                    } else if has {
                        let last = a.cur + 1 >= n;
                        if kit::button_ex(ui, Kind::Primary, if last { "Склеить и перевернуть!" } else { "Следующий кусочек" }, Some(if last { Icon::Repeat } else { Icon::Play }), big, bold(20.0), 20).clicked() { act = Some(if last { (6, 0) } else { (5, a.cur + 1) }); }
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(((ui.available_width() - 630.0) / 2.0).max(0.0));
                            if kit::button_ex(ui, Kind::Ghost, "Ещё раз кусочек", Some(Icon::Play), Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some((5, a.cur)); }
                            if kit::button_ex(ui, Kind::Ghost, "Мой повтор", Some(Icon::Headphones), Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some((7, a.cur)); }
                            if kit::button_ex(ui, Kind::Ghost, "Перезаписать", Some(Icon::Mic), Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some((2, 0)); }
                        });
                    } else {
                        if kit::button_ex(ui, Kind::Primary, &format!("Записать — {r}"), Some(Icon::Mic), big, bold(21.0), 20).clicked() { act = Some((2, 0)); }
                        ui.add_space(10.0);
                        if kit::button_ex(ui, Kind::Ghost, "Ещё раз кусочек", Some(Icon::Play), Vec2::new(220.0, 42.0), semibold(14.0), 12).clicked() { act = Some((5, a.cur)); }
                    }
                }
                APhase::Guess => {
                    let (s, r) = (a.players[a.singer].clone(), a.players[a.repeater].clone());
                    ui.add_space(20.0);
                    ui.label(RichText::new("Что это за песня?").font(display(40.0)));
                    ui.add_space(6.0);
                    kit::text(ui, &format!("Так её «спел» {r}. Угадывает {r}, а {s} говорит — правильно или нет."), body(16.0), TEXT_2(), 640.0);
                    ui.add_space(20.0);
                    if kit::button_ex(ui, Kind::Primary, "Послушать ещё раз", Some(Icon::Play), big, bold(20.0), 20).clicked() { act = Some((10, 0)); }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space(((ui.available_width() - 420.0) / 2.0).max(0.0));
                        if kit::button_ex(ui, Kind::Ghost, "Задом наперёд", Some(Icon::Repeat), Vec2::new(200.0, 42.0), semibold(14.0), 12).on_hover_text("Перевёрнутый оригинал — то, что слышал повторяющий").clicked() { act = Some((11, 0)); }
                        if kit::button_ex(ui, Kind::Ghost, "Оригинал", Some(Icon::Mic), Vec2::new(200.0, 42.0), semibold(14.0), 12).on_hover_text("Как пел первый игрок").clicked() { act = Some((4, 0)); }
                    });
                    ui.add_space(18.0);
                    match a.guessed {
                        None => {
                            ui.horizontal(|ui| {
                                ui.add_space(((ui.available_width() - 470.0) / 2.0).max(0.0));
                                if ui.add(egui::Button::new(RichText::new("✔ Угадал").font(bold(18.0)).color(Color32::BLACK)).fill(Color32::from_rgb(111, 220, 140)).corner_radius(16.0).min_size(Vec2::new(225.0, 56.0))).clicked() { act = Some((12, 1)); }
                                if ui.add(egui::Button::new(RichText::new("✖ Не угадал").font(bold(18.0)).color(TEXT())).fill(CHIP()).corner_radius(16.0).min_size(Vec2::new(225.0, 56.0))).clicked() { act = Some((12, 0)); }
                            });
                        }
                        Some(ok) => {
                            ui.label(RichText::new(if ok { "Угадал! +1 обоим" } else { "Не угадал" }).font(bold(18.0)).color(if ok { OK_TEXT() } else { MUTED() }));
                            ui.add_space(10.0);
                            let last = a.round >= a.rounds;
                            if kit::button_ex(ui, Kind::Primary, if last { "Итоги игры" } else { "Следующий раунд" }, Some(Icon::Play), big, bold(20.0), 20).clicked() { act = Some((13, 0)); }
                        }
                    }
                    ui.add_space(14.0);
                    ui.label(RichText::new(format!("Похожесть на оригинал: {} из 100", a.similarity)).font(semibold(14.0)).color(MUTED()));
                }
                APhase::Final => {
                    ui.add_space(20.0);
                    let mut order: Vec<usize> = (0..a.players.len()).collect();
                    order.sort_by_key(|&i| std::cmp::Reverse(a.points[i]));
                    ui.label(RichText::new("Главный по песням задом наперёд").font(semibold(16.0)).color(MUTED()));
                    ui.label(RichText::new(&a.players[order[0]]).font(display(52.0)).color(ACCENT()));
                    ui.add_space(14.0);
                    egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(20.0).inner_margin(22.0).show(ui, |ui| {
                        ui.set_width(460.0);
                        for (place, &i) in order.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("{}", place + 1)).font(display_bold(18.0)).color(if place == 0 { AMBER() } else { MUTED() }));
                                ui.add_space(8.0);
                                ui.label(RichText::new(&a.players[i]).font(semibold(16.0)));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { ui.label(RichText::new(format!("{} очк.", a.points[i])).font(display_bold(18.0))); });
                            });
                        }
                    });
                    ui.add_space(14.0);
                    if kit::button_ex(ui, Kind::Primary, "Ещё игра", Some(Icon::Repeat), Vec2::new(300.0, 56.0), bold(16.0), 14).clicked() { act = Some((14, 0)); }
                }
            }
            ui.add_space(16.0);
            if ph != APhase::Setup {
                // scores and Boris
                let a = self.apozh.as_ref().unwrap();
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(((ui.available_width() - 120.0 * a.players.len() as f32) / 2.0).max(0.0));
                    for (i, p) in a.players.iter().enumerate() {
                        ui.label(RichText::new(format!("{p}: {}", a.points[i])).font(semibold(14.0)).color(player_color(i)));
                        ui.add_space(14.0);
                    }
                });
                ui.add_space(6.0);
                self.boris_below(ui, 420.0);
            }
        }));
        let Some((c, i)) = act else { return };
        match c {
            1 => self.ap_start(),
            2 => self.ap_rec_toggle(),
            3 => {
                if let Some(a) = &mut self.apozh { a.phase = APhase::Repeat; }
                let r = self.apozh.as_ref().map(|a| a.players[a.repeater].clone()).unwrap_or_default();
                self.boris.queue(format!("Перевернул! {r}, слушай и повторяй как слышишь — даже если это бред."));
                self.ap_goto(0);
            }
            4 => { let s = self.apozh.as_ref().and_then(|a| a.song.clone()); if let Some((s, sr)) = s { self.ap_play(s, sr); } }
            5 => self.ap_goto(i),
            6 => self.ap_finish_repeats(),
            7 => { let t = self.apozh.as_ref().and_then(|a| a.takes.get(i).cloned().flatten().map(|t| (t, a.take_sr))); if let Some((t, sr)) = t { self.ap_play(t, sr); } }
            9 => self.settings.save(),
            10 => { let r = self.apozh.as_ref().map(|a| (a.result.clone(), a.take_sr)); if let Some((s, sr)) = r { self.ap_play(s, sr); } }
            11 => { let r = self.apozh.as_ref().map(|a| (a.rev.clone(), a.sr())); if let Some((s, sr)) = r { self.ap_play(s, sr); } }
            12 => self.ap_judge(i == 1),
            13 => self.ap_next(),
            14 => { if let Some(a) = &mut self.apozh { a.phase = APhase::Setup; } }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversing_twice_gives_back_the_song() {
        let x: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        assert_eq!(reversed(&reversed(&x)), x);
    }

    #[test]
    fn pieces_cover_everything_and_cut_at_pauses() {
        let sr = 16000;
        // 2.2 s sound, 0.2 s pause, 2.6 s sound, 0.2 s pause, 2 s sound
        let tone = |s: f32| (0..(s * sr as f32) as usize).map(|i| (i as f32 * 0.1).sin() * 0.4).collect::<Vec<f32>>();
        let mut x = tone(2.2);
        x.extend(vec![0.0; (0.2 * sr as f32) as usize]);
        x.extend(tone(2.6));
        x.extend(vec![0.0; (0.2 * sr as f32) as usize]);
        x.extend(tone(2.0));
        let p = pieces(&x, sr, 2.5);
        assert_eq!(p.first().unwrap().0, 0);
        assert_eq!(p.last().unwrap().1, x.len());
        for w in p.windows(2) { assert_eq!(w[0].1, w[1].0); }
        // the first cut lands in the first pause (2.2–2.4 s)
        let c = p[0].1 as f32 / sr as f32;
        assert!(c > 2.15 && c < 2.45, "{c}");
    }

    #[test]
    fn silence_is_trimmed_and_glue_keeps_length() {
        let sr = 16000;
        let mut x = vec![0.0f32; 8000];
        x.extend((0..16000).map(|i| (i as f32 * 0.1).sin() * 0.5));
        x.extend(vec![0.0f32; 8000]);
        let t = trim_silence(&x, sr);
        assert!(t.len() > 16000 && t.len() < 17500, "{}", t.len());
        let g = glue(&[t.clone(), t.clone()], sr);
        assert_eq!(g.len(), t.len() * 2);
    }
}

/* ---------- demo stand for README screenshots (Linux only) ---------- */
#[cfg(not(windows))]
impl Apozh {
    pub(crate) fn demo(players: Vec<String>) -> Self {
        let mut a = Apozh::new();
        let sr = 16000u32;
        let song = crate::dub::demo_voice(9.0, sr, 4);
        a.points = vec![2, 1, 1];
        a.players = players;
        a.round = 3; a.rounds = 4; a.singer = 2; a.repeater = 0;
        a.rev = reversed(&song);
        a.cuts = pieces(&a.rev, sr, 2.5);
        a.takes = (0..a.cuts.len()).map(|k| (k < 2).then(|| crate::dub::demo_voice(2.3, sr, 10 + k as u32))).collect();
        a.take_sr = sr;
        a.song = Some((song, sr));
        a.cur = 2;
        let p = a.piece(2);
        a.env = wave::envelope(&p, sr, 300);
        a.play_from = Some((Instant::now() - Duration::from_millis(900), 0.0, p.len() as f32 / sr as f32));
        a.phase = APhase::Repeat;
        a
    }
}
