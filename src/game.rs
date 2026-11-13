//! «Пародист»: the judge plays a sound from the chosen packs, everyone repeats it,
//! then all attempts are played back and the closest one wins the round.

use super::*;
use crate::{mimic, sounds, wave};
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum GPhase { Setup, Round, Reveal, RoundDone, Final }

/// A sound being trimmed in the pack editor.
pub(crate) struct TrimEdit { path: PathBuf, x: Vec<f32>, sr: u32, env: wave::Env, t: wave::Trim, play: Option<(Instant, f32)> }

pub(crate) struct Game {
    trim: Option<TrimEdit>,
    // library
    root: PathBuf,
    packs: Vec<sounds::Pack>,
    /// category tab shown in the setup
    cat: usize,
    off_cats: HashSet<usize>,
    off: HashSet<PathBuf>,
    open_pack: Option<PathBuf>,
    last_scan: Instant,
    pack_name: String,
    names: Vec<String>,
    confirm_delete: bool,
    // players
    /// totals: sum of quality (0–100 per attempt), tries: attempts counted; wins: points (1 per round won)
    players: Vec<String>, totals: Vec<u32>, tries: Vec<u32>, wins: Vec<u32>,
    rounds: usize, round: usize,
    deck: Vec<sounds::Sound>, next: usize, cur: Option<sounds::Sound>,
    pub(crate) phase: GPhase,
    reference: Option<(Vec<f32>, u32)>, ref_feat: Option<mimic::Features>,
    pub(crate) mic: Option<audio::Recorder>,
    /// whose turn it is to record (players.len() = everyone has recorded)
    turn: usize,
    rec: Option<usize>, rec_started: Instant, heard: bool, quiet_since: Option<Instant>,
    attempts: Vec<Option<(Vec<f32>, u32, mimic::MimicScore)>>,
    revealed: Vec<bool>,
    reveal_q: Vec<Option<usize>>, reveal_cur: Option<Option<usize>>, reveal_wait: Option<Instant>,
    round_winner: Option<usize>,
}

impl Game {
    pub(crate) fn new(root: PathBuf) -> Self {
        let packs = sounds::packs(&root);
        let mut g = Self {
            trim: None, root, packs, cat: 1, off_cats: HashSet::new(), off: HashSet::new(), open_pack: None, last_scan: Instant::now(), pack_name: String::new(), names: vec![], confirm_delete: false,
            players: vec!["Игрок 1".into(), "Игрок 2".into()], totals: vec![], tries: vec![], wins: vec![], rounds: 5, round: 0,
            deck: vec![], next: 0, cur: None, phase: GPhase::Setup, reference: None, ref_feat: None, mic: None,
            turn: 0, rec: None, rec_started: Instant::now(), heard: false, quiet_since: None, attempts: vec![], revealed: vec![],
            reveal_q: vec![], reveal_cur: None, reveal_wait: None, round_winner: None,
        };
        g.refresh();
        g
    }

    /// Re-read the folder (after adding, renaming, deleting).
    fn refresh(&mut self) {
        self.packs = sounds::packs(&self.root);
        self.last_scan = Instant::now();
        if let Some(n) = &self.open_pack { if !self.packs.iter().any(|p| &p.path == n) { self.open_pack = None; } }
        let open = self.open_pack.clone();
        if let Some(p) = open.and_then(|n| self.packs.iter().find(|p| p.path == n)) {
            self.pack_name = p.name.clone();
            self.names = p.sounds.iter().map(|s| s.name.clone()).collect();
        }
    }

    fn open_pack_ref(&self) -> Option<&sounds::Pack> { self.open_pack.as_ref().and_then(|n| self.packs.iter().find(|p| &p.path == n)) }

    /// Sounds that can come up: from the switched-on packs, 18+ only if allowed.
    fn pool(&self) -> Vec<sounds::Sound> {
        self.packs.iter().filter(|p| !self.off_cats.contains(&p.cat) && !self.off.contains(&p.path)).flat_map(|p| p.sounds.iter()).cloned().collect()
    }

    /// Where dropped/picked files go: the open pack of this tab, else the category folder.
    fn target_pack(&self) -> PathBuf {
        self.open_pack_ref().filter(|p| p.cat == self.cat).map(|p| p.path.clone()).unwrap_or_else(|| sounds::cat_dir(&self.root, self.cat))
    }
}

#[cfg(windows)]
fn pick_files() -> Vec<PathBuf> { rfd::FileDialog::new().set_title("Звуки для набора").add_filter("Звуки", &sounds::EXT).pick_files().unwrap_or_default() }
#[cfg(not(windows))]
fn pick_files() -> Vec<PathBuf> { vec![] }
#[cfg(windows)]
fn pick_folder() -> Option<PathBuf> { rfd::FileDialog::new().set_title("Папка со звуками — станет набором").pick_folder() }
#[cfg(not(windows))]
fn pick_folder() -> Option<PathBuf> { None }

pub(crate) fn sounds_root() -> PathBuf { user_dir("Пародист") }

impl App {
    pub(crate) fn open_game(&mut self) {
        self.cancel();
        self.mic_test = None;
        match &mut self.game { Some(g) => g.refresh(), None => self.game = Some(Game::new(sounds_root())) }
        self.show_game = true;
    }

    pub(crate) fn exit_game(&mut self) {
        self.show_game = false;
        self.game_player = None;
        if let Some(g) = &mut self.game { g.mic = None; g.rec = None; g.phase = GPhase::Setup; }
    }

    /// Files or folders: folders become packs, files go into the current pack.
    fn game_add_paths(&mut self, paths: Vec<PathBuf>) {
        let Some(g) = &mut self.game else { return };
        let (mut added, mut packs, mut errs) = (0usize, 0usize, Vec::new());
        let target = g.target_pack();
        for p in paths {
            if p.is_dir() {
                match sounds::import_folder(&sounds::cat_dir(&g.root, g.cat), &p) {
                    Ok((d, n)) => { packs += 1; added += n; g.open_pack = Some(d); }
                    Err(e) => errs.push(e),
                }
            } else {
                match sounds::add(&target, &p) { Ok(_) => added += 1, Err(e) => errs.push(e) }
            }
        }
        if g.open_pack.is_none() { g.open_pack = Some(target); }
        g.refresh();
        let cat = sounds::CATEGORIES[g.cat];
        let msg = match (added, packs, errs.first()) {
            (0, _, Some(e)) => e.clone(),
            (n, 0, _) => format!("«{cat}»: добавлено звуков — {n}"),
            (n, k, _) => format!("«{cat}»: новых наборов — {k}, звуков — {n}"),
        };
        self.say(msg);
        if added > 0 { self.boris.queue("О, новые звуки! Сейчас посмеёмся.".into()); }
    }

    fn game_start(&mut self) {
        let Some(g) = &mut self.game else { return };
        g.refresh();
        let mut deck = g.pool();
        if deck.is_empty() { self.say("В выбранных наборах нет звуков — добавьте свои"); return; }
        g.players = roster(&self.settings);
        g.totals = vec![0; g.players.len()];
        g.tries = vec![0; g.players.len()];
        g.wins = vec![0; g.players.len()];
        let mut seed = audio::stamp().bytes().fold(17u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32)) | 1;
        for i in (1..deck.len()).rev() { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; deck.swap(i, seed as usize % (i + 1)); }
        g.deck = deck;
        g.next = 0;
        g.round = 0;
        if g.mic.is_none() {
            match audio::Recorder::mic(&self.settings.input, self.params.clone()) {
                Ok(m) => g.mic = Some(m),
                Err(e) => { self.say(e); return; }
            }
        }
        self.game_next_round();
    }

    fn game_rounds(g: &Game) -> usize { g.rounds.min(g.deck.len()).max(1) }

    fn game_next_round(&mut self) {
        let mut tries = 0;
        loop {
            let Some(g) = &mut self.game else { return };
            if g.round >= Self::game_rounds(g) || g.next >= g.deck.len() { g.phase = GPhase::Final; self.game_final(); return; }
            let s = g.deck[g.next].clone();
            g.next += 1;
            match audio::load_mono(&s.path, 20.0) {
                Ok((smp, sr)) => {
                    let f = mimic::features(&smp, sr);
                    g.round += 1;
                    g.cur = Some(s.clone());
                    g.reference = Some((smp.clone(), sr));
                    g.ref_feat = Some(f);
                    g.attempts = vec![None; g.players.len()];
                    g.revealed = vec![false; g.players.len()];
                    g.phase = GPhase::Round;
                    g.rec = None;
                    g.turn = 0;
                    g.round_winner = None;
                    g.reveal_cur = None;
                    let n = g.round;
                    self.game_player = audio::Player::from_samples(smp, sr, &None, 1.0).ok();
                    let short: String = s.name.chars().take(40).collect();
                    self.boris.queue(match s.cat {
                        0 => format!("Раунд {n}. Музыка: «{short}». Слушаем — и напеваем!"),
                        2 => format!("Раунд {n}. Зверь: «{short}». Кто изобразит похоже?"),
                        _ => format!("Раунд {n}. Слушаем: «{short}». Кто повторит?"),
                    });
                    return;
                }
                Err(e) => {
                    tries += 1;
                    let name = s.name.clone();
                    self.say(format!("«{name}» не читается: {e}"));
                    if tries > 20 { if let Some(g) = &mut self.game { g.phase = GPhase::Setup; } return; }
                }
            }
        }
    }

    fn game_final(&mut self) {
        let Some(g) = &self.game else { return };
        if let Some(w) = (0..g.players.len()).max_by_key(|&i| (g.wins[i], avg_quality(g, i))) {
            let name = g.players[w].clone();
            self.boris.queue(format!("Пародист вечера — {name}! Признаю, похоже было."));
            if self.settings.crowd_sfx { self.sfx = audio::Player::from_samples(sfx::for_score(95, 3).unwrap_or_default(), sfx::SR, &None, 0.8).ok(); }
        }
    }

    /// Start/stop a player's attempt.
    fn game_toggle_rec(&mut self, i: usize) {
        let Some(g) = &mut self.game else { return };
        if let Some(cur) = g.rec.take() {
            let sr = g.mic.as_ref().map(|r| r.sr).unwrap_or(48000);
            let s = g.mic.as_ref().map(|r| r.end()).unwrap_or_default();
            let f = mimic::features(&s, sr);
            let sc = g.ref_feat.as_ref().map(|r| mimic::compare(r, &f)).unwrap_or_default();
            g.attempts[cur] = Some((s, sr, sc));
            let n = g.players.len();
            g.turn = (1..=n).map(|k| (cur + k) % n).find(|&j| g.attempts[j].is_none()).unwrap_or(n);
        } else if let Some(r) = &g.mic {
            r.begin();
            g.rec = Some(i);
            g.rec_started = Instant::now();
            g.heard = false;
            g.quiet_since = None;
            self.game_player = None; // the original must not get into the mic
        } else {
            self.say("Микрофон не найден — проверьте его в «Звук»");
        }
    }

    fn game_tick(&mut self) {
        // auto-stop an attempt after the player has spoken and gone quiet
        let mut stop = None;
        if let Some(g) = &mut self.game {
            if let Some(i) = g.rec {
                let lvl = g.mic.as_ref().map(|m| m.level()).unwrap_or(0.0);
                let el = g.rec_started.elapsed().as_secs_f32();
                // stop only after a real pause, and not before most of the phrase could have been said
                let ref_secs = g.ref_feat.as_ref().map(|f| f.seconds()).unwrap_or(3.0);
                if lvl > 0.025 { g.heard = true; g.quiet_since = None; }
                else if g.heard && lvl < 0.008 && g.quiet_since.is_none() { g.quiet_since = Some(Instant::now()); }
                else if lvl >= 0.008 { g.quiet_since = None; }
                let max = (ref_secs * 1.6 + 4.0).max(8.0);
                let min = (ref_secs * 0.6).max(1.5);
                let silent = g.quiet_since.map(|t| t.elapsed().as_secs_f32() > 2.0).unwrap_or(false);
                if (silent && el > min) || el > max { stop = Some(i); }
            }
        }
        if let Some(i) = stop { self.game_toggle_rec(i); }
        // round reveal: original → each attempt → score → … → winner
        let mut say: Option<String> = None;
        let mut play: Option<(Vec<f32>, u32)> = None;
        let mut applause = false;
        if let Some(g) = &mut self.game {
            if g.phase == GPhase::Reveal {
                let playing = self.game_player.as_ref().map(|p| !p.finished() && !p.paused()).unwrap_or(false);
                if !playing {
                    if g.reveal_wait.is_none() { g.reveal_wait = Some(Instant::now()); }
                    if g.reveal_wait.map(|t| t.elapsed().as_secs_f32() > 0.8).unwrap_or(false) {
                        g.reveal_wait = None;
                        if let Some(Some(i)) = g.reveal_cur {
                            g.revealed[i] = true;
                            let sc = g.attempts[i].as_ref().map(|a| a.2.total).unwrap_or(0);
                            say = Some(meme_reaction(sc, i as u32 + g.round as u32 * 7).to_string());
                        }
                        if g.reveal_q.is_empty() {
                            g.reveal_cur = None;
                            let best = (0..g.players.len()).filter(|&i| g.attempts[i].is_some()).max_by_key(|&i| g.attempts[i].as_ref().map(|a| a.2.total).unwrap_or(0));
                            for i in 0..g.players.len() { if let Some(a) = &g.attempts[i] { g.totals[i] += a.2.total; g.tries[i] += 1; } }
                            if let Some(b) = best {
                                g.wins[b] += 1; g.round_winner = Some(b);
                                let sc = g.attempts[b].as_ref().map(|a| a.2.total).unwrap_or(0);
                                let tail = ["Это шедевр!", "Красавчик!", "Так и запишем.", "Учитесь, студенты!"][g.round % 4];
                                say = Some(format!("Очко получает {}! Качество {sc} из 100. {tail}", g.players[b]));
                                applause = true;
                            }
                            g.phase = GPhase::RoundDone;
                        } else {
                            let nx = g.reveal_q.remove(0);
                            g.reveal_cur = Some(nx);
                            play = match nx { None => g.reference.clone(), Some(i) => g.attempts[i].as_ref().map(|a| (a.0.clone(), a.1)) };
                        }
                    }
                }
            }
        }
        if let Some(t) = say { self.boris.queue(t); }
        if let Some((s, sr)) = play { self.game_player = audio::Player::from_samples(s, sr, &None, 1.0).ok(); }
        if applause && self.settings.crowd_sfx { self.sfx = audio::Player::from_samples(sfx::applause(2.5, 20, 9), sfx::SR, &None, 0.7).ok(); }
    }

    fn game_begin_reveal(&mut self) {
        let Some(g) = &mut self.game else { return };
        if g.rec.take().is_some() { if let Some(m) = &g.mic { m.end(); } }
        let mut q: Vec<Option<usize>> = vec![None];
        for i in 0..g.players.len() { if g.attempts[i].is_some() { q.push(Some(i)); } }
        g.reveal_q = q;
        g.reveal_cur = None;
        g.reveal_wait = Some(Instant::now() - Duration::from_secs(2));
        g.round_winner = None;
        g.phase = GPhase::Reveal;
        self.game_player = None;
        self.boris.queue("Итак, слушаем! Сначала оригинал…".into());
    }

    pub(crate) fn header_game(&mut self, ui: &mut egui::Ui) {
        let sub = self.game.as_ref().map(|g| match g.phase { GPhase::Setup => "мини-игра".to_string(), GPhase::Final => "итоги игры".into(), _ => format!("раунд {} из {}", g.round, Self::game_rounds(g)) }).unwrap_or_default();
        ui.horizontal_centered(|ui| {
            if kit::ghost(ui, "Назад", Some(Icon::Back)).clicked() { self.exit_game(); return; }
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new("Пародист").font(display_bold(18.0)));
                ui.label(RichText::new(sub).font(body(13.0)).color(MUTED()));
            });
        });
    }

    pub(crate) fn game_screen(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.video_rect = None;
        self.game_tick();
        ctx.request_repaint_after(Duration::from_millis(40));
        let phase = self.game.as_ref().map(|g| g.phase).unwrap_or(GPhase::Setup);
        if phase == GPhase::Setup {
            let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
            if !dropped.is_empty() { self.game_add_paths(dropped); }
            let typing = ctx.memory(|m| m.focused().is_some());
            if let Some(g) = &mut self.game { if !typing && g.last_scan.elapsed().as_secs_f32() > 2.0 { g.refresh(); } }
            return self.game_setup(ui, ctx);
        }
        if phase == GPhase::Final { return self.game_final_view(ui); }
        self.game_round_view(ui);
    }

    fn game_round_view(&mut self, ui: &mut egui::Ui) {
        let col_w = 400.0;
        let gap = 24.0;
        let avail = ui.available_size();
        let left_w = (avail.x - col_w - gap).max(320.0);
        let mut act: Option<u8> = None;
        let mut rec_toggle: Option<usize> = None;
        let mut listen: Option<(Vec<f32>, u32)> = None;
        let playing = self.game_player.as_ref().map(|p| !p.finished() && !p.paused()).unwrap_or(false);
        let t = ui.ctx().input(|i| i.time) as f32;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            // ---- left: the sound ----
            ui.allocate_ui_with_layout(Vec2::new(left_w, avail.y), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.set_width(left_w);
                let g = self.game.as_ref().unwrap();
                kit::section_label(ui, match g.cur.as_ref().map(|s| s.cat) { Some(0) => "МУЗЫКА — НАПОЙТЕ КАК В ОРИГИНАЛЕ", Some(2) => "ЖИВОТНЫЕ — ИЗОБРАЗИТЕ ЭТОТ ЗВУК", _ => "МЕМЫ — ПОВТОРИТЕ КАК В ОРИГИНАЛЕ" });
                ui.add_space(10.0);
                let h = (avail.y - 300.0).clamp(160.0, 300.0);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(left_w, h), egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, CornerRadius::same(20), SURFACE());
                // a simple "sound" picture: bars that move while something plays
                let bars = 28;
                let bw = (rect.width() - 80.0) / bars as f32;
                for k in 0..bars {
                    let ph = k as f32 * 0.55;
                    let a = if playing { 0.25 + 0.75 * ((t * 7.0 + ph).sin() * 0.5 + 0.5) * ((t * 3.1 + ph * 1.7).sin() * 0.5 + 0.5) } else { 0.12 + 0.05 * (ph * 2.3).sin().abs() };
                    let bh = (h - 90.0) * a;
                    let x = rect.left() + 40.0 + k as f32 * bw;
                    let r = Rect::from_center_size(Pos2::new(x + bw / 2.0, rect.center().y - 14.0), Vec2::new(bw * 0.55, bh.max(4.0)));
                    p.rect_filled(r, CornerRadius::same(3), if playing { ACCENT() } else { BAR_OFF() });
                }
                let (cap, col) = match (g.phase, g.reveal_cur) {
                    (GPhase::Reveal, Some(None)) => ("Оригинал".to_string(), ACCENT_TEXT()),
                    (GPhase::Reveal, Some(Some(i))) => (g.players[i].clone(), ACCENT_TEXT()),
                    (GPhase::Reveal, None) => ("Слушаем попытки…".to_string(), MUTED()),
                    (GPhase::RoundDone, _) => (g.round_winner.map(|w| format!("Лучший: {}", g.players[w])).unwrap_or("Раунд окончен".into()), AMBER()),
                    _ if playing => ("Звучит оригинал…".to_string(), ACCENT_TEXT()),
                    _ if g.rec.is_some() => (format!("Записываю: {}…", g.players[g.rec.unwrap()]), REC_TEXT()),
                    _ => (g.ref_feat.as_ref().map(|f| format!("Образец: {:.1} с", f.seconds()).replace('.', ",")).unwrap_or_default(), MUTED()),
                };
                p.text(Pos2::new(rect.center().x, rect.bottom() - 30.0), Align2::CENTER_CENTER, cap, semibold(17.0), col);
                ui.add_space(12.0);
                if g.phase == GPhase::Round {
                    ui.vertical_centered(|ui| {
                        ui.add_space(6.0);
                        let big = Vec2::new((left_w * 0.6).clamp(320.0, 520.0), 76.0);
                        if let Some(i) = g.rec {
                            if kit::button_ex(ui, Kind::Primary, &format!("Стоп — {}", g.players[i]), Some(Icon::Stop), big, bold(22.0), 20).clicked() { rec_toggle = Some(i); }
                        } else if g.turn < g.players.len() {
                            if kit::button_ex(ui, Kind::Primary, &format!("Записать — {}", g.players[g.turn]), Some(Icon::Mic), big, bold(22.0), 20).clicked() { rec_toggle = Some(g.turn); }
                        } else if kit::button_ex(ui, Kind::Primary, "Итоги раунда", Some(Icon::Play), big, bold(22.0), 20).clicked() { act = Some(1); }
                        ui.add_space(10.0);
                        let hint = if g.rec.is_some() { "Говорите. Запись остановится сама после паузы — или нажмите «Стоп».".to_string() }
                            else if g.turn < g.players.len() { format!("Очередь: {}. Нажмите и повторите звук.", g.players[g.turn]) }
                            else { "Все записались — жмите «Итоги раунда». Перезаписать можно справа.".to_string() };
                        ui.label(RichText::new(hint).font(body(14.0)).color(MUTED()));
                        if g.rec.is_none() {
                            ui.add_space(12.0);
                            // two equal buttons, centered as a pair
                            let small = Vec2::new(200.0, 42.0);
                            ui.allocate_ui_with_layout(Vec2::new(410.0, 44.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                if kit::button_ex(ui, Kind::Ghost, "Послушать заново", Some(Icon::Play), small, semibold(14.0), 12).clicked() { listen = g.reference.clone(); }
                                if kit::button_ex(ui, Kind::Ghost, "Другой звук", Some(Icon::Repeat), small, semibold(14.0), 12).clicked() { act = Some(6); }
                            });
                        }
                    });
                }
            });
            // ---- right: players ----
            ui.allocate_ui_with_layout(Vec2::new(col_w, avail.y), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.set_width(col_w);
                let g = self.game.as_ref().unwrap();
                kit::card(ui, SURFACE(), |ui| {
                    ui.set_width(col_w - 40.0);
                    ui.horizontal(|ui| {
                        kit::section_label(ui, "ИГРОКИ");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { ui.label(RichText::new("очки").font(body(12.0)).color(MUTED())); });
                    });
                    ui.add_space(6.0);
                    for i in 0..g.players.len() {
                        let recording = g.rec == Some(i);
                        let att = g.attempts[i].as_ref();
                        let show_score = g.revealed[i];
                        let playing_now = g.reveal_cur == Some(Some(i)) || (g.phase == GPhase::Round && (g.rec == Some(i) || (g.rec.is_none() && g.turn == i)));
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(col_w - 40.0, 52.0), egui::Sense::hover());
                        let p = ui.painter();
                        if playing_now || g.round_winner == Some(i) { p.rect_filled(rect.expand2(Vec2::new(8.0, 0.0)), CornerRadius::same(12), ACCENT_SOFT()); }
                        p.text(Pos2::new(rect.left(), rect.center().y - 8.0), Align2::LEFT_CENTER, &g.players[i], semibold(15.0), TEXT());
                        let sub = if recording { "говорите…".to_string() }
                            else if g.phase == GPhase::Round && g.rec.is_none() && g.turn == i && att.is_none() { "следующий".to_string() }
                            else if let Some(a) = att { if show_score { format!("интонация {} · ритм {} · темп {}", a.2.intonation, a.2.rhythm, a.2.tempo) } else { "записано".into() } }
                            else { "ещё не пробовал".into() };
                        p.text(Pos2::new(rect.left(), rect.center().y + 10.0), Align2::LEFT_CENTER, sub, body(12.0), if recording { ACCENT_TEXT() } else { MUTED() });
                        p.text(rect.right_center(), Align2::RIGHT_CENTER, g.wins[i].to_string(), display_bold(20.0), if g.wins[i] > 0 { ACCENT_TEXT() } else { MUTED() });
                        if let (Some(a), true) = (att, show_score) {
                            p.text(rect.right_center() - Vec2::new(44.0, 0.0), Align2::RIGHT_CENTER, format!("{}/100", a.2.total), semibold(15.0), if g.round_winner == Some(i) { AMBER() } else { TEXT_2() });
                        }
                        if g.phase == GPhase::Round && g.rec.is_none() && att.is_some() {
                            let br = Rect::from_min_size(Pos2::new(rect.right() - 136.0, rect.center().y - 18.0), Vec2::new(84.0, 36.0));
                            let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(br).layout(egui::Layout::right_to_left(egui::Align::Center)));
                            cu.spacing_mut().item_spacing.x = 6.0;
                            if kit::icon_button(&mut cu, Icon::Mic, 36.0).on_hover_text("Перезаписать").clicked() { rec_toggle = Some(i); }
                            if kit::icon_button(&mut cu, Icon::Play, 36.0).on_hover_text("Послушать").clicked() { listen = att.map(|a| (a.0.clone(), a.1)); }
                        }
                    }
                });
                ui.add_space(12.0);
                let w = col_w;
                let any = g.attempts.iter().any(|a| a.is_some());
                let last = g.round >= Self::game_rounds(g);
                match g.phase {
                    GPhase::Round => {
                        if any && g.rec.is_none() && g.turn < g.players.len() && kit::button_ex(ui, Kind::Ghost, "Итоги раунда (не все записались)", None, Vec2::new(w, 48.0), semibold(14.0), 14).clicked() { act = Some(1); }
                    }
                    GPhase::Reveal => {
                        if kit::button_ex(ui, Kind::Ghost, "Пропустить прослушивание", None, Vec2::new(w, 48.0), semibold(14.0), 14).clicked() { act = Some(2); }
                    }
                    GPhase::RoundDone => {
                        if kit::button_ex(ui, Kind::Primary, if last { "Итоги игры" } else { "Следующий звук" }, None, Vec2::new(w, 60.0), bold(16.0), 16).clicked() { act = Some(3); }
                    }
                    _ => {}
                }
                ui.add_space(8.0);
                self.boris_below(ui, w);
            });
        });
        if let Some(i) = rec_toggle { self.game_toggle_rec(i); }
        if let Some((s, sr)) = listen { self.game_player = audio::Player::from_samples(s, sr, &None, 1.0).ok(); }
        match act {
            Some(1) => self.game_begin_reveal(),
            Some(2) => {
                self.game_player = None;
                if let Some(g) = &mut self.game {
                    for x in g.reveal_q.drain(..).flatten().collect::<Vec<_>>() { g.revealed[x] = true; }
                    if let Some(Some(i)) = g.reveal_cur { g.revealed[i] = true; }
                    g.reveal_cur = None;
                    g.reveal_wait = Some(Instant::now() - Duration::from_secs(2));
                }
            }
            Some(3) => {
                let last = self.game.as_ref().map(|g| g.round >= Self::game_rounds(g)).unwrap_or(true);
                if last { if let Some(g) = &mut self.game { g.phase = GPhase::Final; } self.game_final(); } else { self.game_next_round(); }
            }
            Some(6) => {
                // swap the sound: this round doesn't count
                if let Some(g) = &mut self.game { g.round -= 1; if g.next >= g.deck.len() { g.next = 0; } }
                self.game_next_round();
            }
            _ => {}
        }
    }

    fn game_setup(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut start = false;
        let mut roster_changed = false;
        let mut add_files = false;
        let mut add_folder = false;
        let mut new_pack = false;
        let mut open_folder: Option<PathBuf> = None;
        let mut play: Option<PathBuf> = None;
        let mut rename: Option<(PathBuf, String)> = None;
        let mut remove: Option<PathBuf> = None;
        let mut trim_open: Option<PathBuf> = None;
        let mut trim_act = wave::TrimAct::None;
        let mut rename_pack: Option<(PathBuf, String)> = None;
        let mut delete_pack: Option<PathBuf> = None;
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| ui.vertical_centered(|ui| {
            ui.add_space(16.0);
            ui.allocate_ui_with_layout(Vec2::new(720.0, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.label(RichText::new("Кто лучше спародирует?").font(display(40.0)));
                ui.add_space(6.0);
                kit::text(ui, "Судья включает случайный звук — песню, мем или зверя, — каждый повторяет его в микрофон. В конце раунда звучат все попытки, и судья выбирает, чья ближе к оригиналу.", body(16.0), TEXT_2(), 720.0);
                ui.add_space(16.0);
                let g = self.game.as_mut().unwrap();
                // ---- categories and packs ----
                egui::Frame::new().fill(if hovering { SELECTED() } else { SURFACE() }).stroke(Stroke::new(1.0_f32, if hovering { ACCENT() } else { BORDER() })).corner_radius(20.0).inner_margin(24.0).show(ui, |ui| {
                    ui.set_width(672.0);
                    kit::section_label(ui, "КАТЕГОРИИ В ИГРЕ");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for c in 0..sounds::CATEGORIES.len() {
                            let mut on = !g.off_cats.contains(&c);
                            let n: usize = g.packs.iter().filter(|p| p.cat == c && !g.off.contains(&p.path)).map(|p| p.sounds.len()).sum();
                            let txt = RichText::new(format!("{}  {}  · {n}", if on { "✔" } else { "  " }, sounds::CATEGORIES[c])).font(bold(15.0)).color(if on { ON_ACCENT() } else { MUTED() });
                            if ui.add(egui::Button::new(txt).fill(if on { ACCENT() } else { CHIP() }).corner_radius(12.0).min_size(Vec2::new(160.0, 44.0))).clicked() { on = !on; if on { g.off_cats.remove(&c); } else { g.off_cats.insert(c); } }
                        }
                    });
                    kit::text(ui, "Можно играть сразу во все три — звуки пойдут вперемешку.", body(13.0), MUTED(), 672.0);
                    ui.add_space(16.0);
                    kit::section_label(ui, "РЕДАКТИРОВАТЬ ЗВУКИ");
                    ui.add_space(8.0);
                    // category tabs
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for c in 0..sounds::CATEGORIES.len() {
                            let r = kit::button_ex(ui, if g.cat == c { Kind::Primary } else { Kind::Ghost }, sounds::CATEGORIES[c], None, Vec2::new(150.0, 40.0), bold(14.0), 12);
                            if r.clicked() && g.cat != c { g.cat = c; g.confirm_delete = false; if g.open_pack_ref().map(|p| p.cat != c).unwrap_or(false) { g.open_pack = None; } }
                        }
                    });
                    ui.add_space(10.0);
                    let c = g.cat;
                    let cat_on = !g.off_cats.contains(&c);
                    kit::text(ui, "Перетащите в окно mp3-файлы (попадут в выбранный набор) или целую папку (станет новым набором). Можно и через проводник — просто кладите файлы в папку категории.", body(13.0), MUTED(), 672.0);
                    ui.horizontal(|ui| {
                        if kit::link(ui, &format!("Открыть папку «{}»", sounds::CATEGORIES[c])).clicked() { open_folder = Some(sounds::cat_dir(&g.root, c)); }
                        ui.add_space(12.0);
                        if kit::link(ui, "+ Новый набор").clicked() { new_pack = true; }
                        ui.add_space(12.0);
                        if kit::link(ui, "Добавить папку как набор").clicked() { add_folder = true; }
                    });
                    ui.add_space(8.0);
                    let packs: Vec<sounds::Pack> = g.packs.iter().filter(|p| p.cat == c).cloned().collect();
                    for p in &packs {
                        if p.loose && p.sounds.is_empty() && packs.len() > 1 && g.open_pack.as_ref() != Some(&p.path) { continue; }
                        let mut on = !g.off.contains(&p.path);
                        let is_open = g.open_pack.as_ref() == Some(&p.path);
                        ui.horizontal(|ui| {
                            if ui.add_enabled(cat_on, egui::Checkbox::without_text(&mut on)).changed() { if on { g.off.remove(&p.path); } else { g.off.insert(p.path.clone()); } }
                            let label = format!("{} · {} зв.", p.name, p.sounds.len());
                            let toggle = ui.add(egui::Label::new(RichText::new(label).font(semibold(15.0)).color(if is_open { ACCENT_TEXT() } else { TEXT() })).sense(egui::Sense::click())).clicked();
                            let toggle = toggle | ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| kit::link(ui, if is_open { "Свернуть" } else { "Изменить" }).clicked()).inner;
                            if toggle {
                                g.open_pack = if is_open { None } else { Some(p.path.clone()) };
                                g.confirm_delete = false;
                                if !is_open { g.pack_name = p.name.clone(); g.names = p.sounds.iter().map(|s| s.name.clone()).collect(); }
                            }
                        });
                        if is_open {
                            egui::Frame::new().fill(SURFACE_2()).corner_radius(14.0).inner_margin(14.0).show(ui, |ui| {
                                ui.set_width(644.0);
                                if !p.loose {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("Название").font(body(13.0)).color(MUTED()));
                                        let r = ui.add(egui::TextEdit::singleline(&mut g.pack_name).font(semibold(15.0)).margin(Vec2::new(10.0, 8.0)).desired_width(300.0));
                                        if r.lost_focus() && g.pack_name.trim() != p.name && !g.pack_name.trim().is_empty() { rename_pack = Some((p.path.clone(), g.pack_name.clone())); }
                                    });
                                    ui.add_space(8.0);
                                }
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    if kit::button_ex(ui, Kind::Primary, "Добавить звуки", Some(Icon::Folder), Vec2::new(0.0, 40.0), bold(14.0), 10).clicked() { add_files = true; }
                                    if kit::ghost(ui, "Открыть папку", Some(Icon::External)).clicked() { open_folder = Some(p.path.clone()); }
                                    if !p.loose {
                                        if g.confirm_delete {
                                            if kit::button_ex(ui, Kind::Primary, "Да, удалить набор", Some(Icon::Trash), Vec2::new(0.0, 40.0), bold(14.0), 10).clicked() { delete_pack = Some(p.path.clone()); }
                                            if kit::ghost(ui, "Отмена", None).clicked() { g.confirm_delete = false; }
                                        } else if kit::ghost(ui, "Удалить набор", Some(Icon::Trash)).clicked() { g.confirm_delete = true; }
                                    }
                                });
                                ui.add_space(8.0);
                                if p.sounds.is_empty() {
                                    kit::text(ui, "Пусто. Нажмите «Добавить звуки» или перетащите файлы в окно. Название звука — это имя файла, его можно поправить здесь.", body(13.0), MUTED(), 644.0);
                                }
                                if g.names.len() != p.sounds.len() { g.names = p.sounds.iter().map(|s| s.name.clone()).collect(); }
                                for (k, s) in p.sounds.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        if kit::icon_button(ui, Icon::Play, 34.0).on_hover_text("Послушать").clicked() { play = Some(s.path.clone()); }
                                        let r = ui.add(egui::TextEdit::singleline(&mut g.names[k]).font(body(14.0)).margin(Vec2::new(8.0, 6.0)).desired_width(500.0));
                                        if r.lost_focus() && g.names[k].trim() != s.name && !g.names[k].trim().is_empty() { rename = Some((s.path.clone(), g.names[k].clone())); }
                                        // the scissors stay small and quiet until needed
                                        let open = g.trim.as_ref().map(|t| t.path == s.path).unwrap_or(false);
                                        if ui.add(egui::Button::new(RichText::new("✂").font(body(15.0)).color(if open { ACCENT_TEXT() } else { MUTED() })).frame(false)).on_hover_text("Обрезать").clicked() { trim_open = Some(s.path.clone()); }
                                        if kit::icon_button(ui, Icon::Trash, 34.0).on_hover_text("Удалить").clicked() { remove = Some(s.path.clone()); }
                                    });
                                    if let Some(te) = g.trim.as_mut().filter(|t| t.path == s.path) {
                                        let head = te.play.map(|(t0, a)| a + t0.elapsed().as_secs_f32()).filter(|h| *h < te.t.b);
                                        ui.add_space(4.0);
                                        trim_act = wave::trim_ui(ui, 620.0, &te.env, &mut te.t, head);
                                        ui.add_space(8.0);
                                    }
                                }
                            });
                            ui.add_space(6.0);
                        }
                    }
                    if packs.iter().all(|p| p.sounds.is_empty()) {
                        ui.add_space(4.0);
                        kit::text(ui, "В этой категории пока нет звуков.", body(13.0), MUTED(), 672.0);
                    }
                });
                ui.add_space(14.0);
                // ---- players: today's list, shared by all games ----
                let players = &mut self.settings.players;
                egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(20.0).inner_margin(24.0).show(ui, |ui| {
                    ui.set_width(672.0);
                    kit::section_label(ui, "ИГРАЮТ СЕГОДНЯ");
                    ui.add_space(6.0);
                    roster_changed = roster_editor(ui, players, 672.0);
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Раундов").font(semibold(15.0)));
                        for n in [3usize, 5, 10] {
                            let on = g.rounds == n;
                            if kit::button_ex(ui, if on { Kind::Primary } else { Kind::Ghost }, &n.to_string(), None, Vec2::new(56.0, 40.0), bold(14.0), 10).clicked() { g.rounds = n; }
                        }
                    });
                });
                ui.add_space(16.0);
                let n = g.pool().len();
                let label = if n == 0 { "Добавьте звуки, чтобы начать".to_string() } else { format!("Начать игру · {n} зв.") };
                if kit::button_ex(ui, Kind::Primary, &label, Some(Icon::Play), Vec2::new(720.0, 60.0), bold(17.0), 16).clicked() && n > 0 { start = true; }
                ui.add_space(16.0);
            });
        }));
        // apply actions (outside the borrow of the game)
        if let Some(p) = trim_open {
            let again = self.game.as_ref().and_then(|g| g.trim.as_ref()).map(|t| t.path == p).unwrap_or(false);
            if again { if let Some(g) = &mut self.game { g.trim = None; } } else {
                match audio::load_mono(&p, 120.0) {
                    Ok((x, sr)) => if let Some(g) = &mut self.game {
                        let env = wave::envelope(&x, sr, 600);
                        let t = wave::Trim::new(0.0, env.secs);
                        g.trim = Some(TrimEdit { path: p, x, sr, env, t, play: None });
                    },
                    Err(e) => self.say(e),
                }
            }
        }
        match trim_act {
            wave::TrimAct::Play => {
                let piece = self.game.as_mut().and_then(|g| g.trim.as_mut()).map(|te| {
                    te.play = Some((Instant::now(), te.t.a));
                    let (i0, i1) = ((te.t.a * te.sr as f32) as usize, ((te.t.b * te.sr as f32) as usize).min(te.x.len()));
                    (te.x[i0.min(i1)..i1].to_vec(), te.sr)
                });
                if let Some((s, sr)) = piece { self.game_player = audio::Player::from_samples(s, sr, &None, 1.0).ok(); }
            }
            wave::TrimAct::Overwrite | wave::TrimAct::SaveNew => {
                let over = trim_act == wave::TrimAct::Overwrite;
                let r = self.game.as_ref().and_then(|g| g.trim.as_ref()).map(|te| sounds::save_trimmed(&te.path, &te.x, te.sr, te.t.a, te.t.b, over));
                self.game_player = None;
                match r {
                    Some(Ok(p)) => {
                        if let Some(g) = &mut self.game { g.trim = None; g.refresh(); }
                        let n = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                        self.say(if over { format!("Обрезано: «{n}»") } else { format!("Сохранено как «{n}»") });
                    }
                    Some(Err(e)) => self.say(e),
                    None => {}
                }
            }
            wave::TrimAct::Cancel => { self.game_player = None; if let Some(g) = &mut self.game { g.trim = None; } }
            wave::TrimAct::None => {}
        }
        if let Some(p) = play { match audio::load_mono(&p, 20.0) { Ok((s, sr)) => self.game_player = audio::Player::from_samples(s, sr, &None, 1.0).ok(), Err(e) => self.say(e) } }
        if let Some(d) = open_folder { open(d); }
        let mut err: Option<String> = None;
        if let Some(g) = &mut self.game {
            let mut changed = false;
            if let Some((p, name)) = rename { if let Err(e) = sounds::rename(&p, &name) { err = Some(e); } changed = true; }
            if let Some(p) = remove { if let Err(e) = sounds::remove(&p) { err = Some(e); } changed = true; }
            if let Some((p, name)) = rename_pack {
                match sounds::rename_pack(&p, &name) {
                    Ok(d) => { if g.off.remove(&p) { g.off.insert(d.clone()); } g.open_pack = Some(d); }
                    Err(e) => err = Some(e),
                }
                changed = true;
            }
            if let Some(p) = delete_pack { if let Err(e) = sounds::delete_pack(&p) { err = Some(e); } g.open_pack = None; g.confirm_delete = false; changed = true; }
            if new_pack {
                match sounds::create_pack(&sounds::cat_dir(&g.root, g.cat), "Новый набор") { Ok(d) => { g.pack_name = d.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(); g.open_pack = Some(d); } Err(e) => err = Some(e) }
                changed = true;
            }
            if changed { g.refresh(); }
        }
        if let Some(e) = err { self.say(e); }
        if add_files { let f = pick_files(); if !f.is_empty() { self.game_add_paths(f); } }
        if add_folder { if let Some(d) = pick_folder() { self.game_add_paths(vec![d]); } }
        if roster_changed { self.settings.save(); }
        if start { self.game_start(); }
    }

    fn game_final_view(&mut self, ui: &mut egui::Ui) {
        let mut act = None;
        ui.vertical_centered(|ui| {
            ui.add_space(10.0);
            let g = self.game.as_ref().unwrap();
            let mut order: Vec<usize> = (0..g.players.len()).collect();
            order.sort_by_key(|&i| std::cmp::Reverse((g.wins[i], avg_quality(g, i))));
            ui.label(RichText::new("Пародист вечера").font(semibold(16.0)).color(MUTED()));
            ui.label(RichText::new(&g.players[order[0]]).font(display(52.0)).color(ACCENT()));
            ui.add_space(14.0);
            ui.allocate_ui_with_layout(Vec2::new(560.0, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(20.0).inner_margin(22.0).show(ui, |ui| {
                    ui.set_width(516.0);
                    for (place, &i) in order.iter().enumerate() {
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(516.0, 44.0), egui::Sense::hover());
                        let p = ui.painter();
                        if place == 0 { p.rect_filled(rect.expand2(Vec2::new(8.0, 0.0)), CornerRadius::same(12), AMBER_SOFT()); }
                        p.text(Pos2::new(rect.left() + 10.0, rect.center().y), Align2::LEFT_CENTER, format!("{}", place + 1), display_bold(18.0), if place == 0 { AMBER() } else { MUTED() });
                        p.text(Pos2::new(rect.left() + 44.0, rect.center().y), Align2::LEFT_CENTER, &g.players[i], semibold(16.0), TEXT());
                        p.text(Pos2::new(rect.right() - 110.0, rect.center().y), Align2::RIGHT_CENTER, format!("качество {}/100", avg_quality(g, i)), body(13.0), MUTED());
                        p.text(rect.right_center(), Align2::RIGHT_CENTER, format!("{} очк.", g.wins[i]), display_bold(18.0), TEXT());
                    }
                    ui.label(RichText::new("Очко — за победу в раунде. Качество — среднее по всем попыткам, из 100.").font(body(12.0)).color(MUTED()));
                });
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    if kit::button_ex(ui, Kind::Primary, "Ещё игра", Some(Icon::Repeat), Vec2::new(0.0, 56.0), bold(16.0), 14).clicked() { act = Some(1); }
                    if kit::button_ex(ui, Kind::Ghost, "Выйти", None, Vec2::new(0.0, 56.0), semibold(15.0), 14).clicked() { act = Some(2); }
                });
            });
            ui.add_space(10.0);
            self.boris_column(ui, 170.0, 360.0);
        });
        match act { Some(1) => { if let Some(g) = &mut self.game { g.phase = GPhase::Setup; g.refresh(); } } Some(2) => self.exit_game(), _ => {} }
    }
}

/// Boris's verdict on one attempt — in the language of memes.
fn meme_reaction(score: u32, seed: u32) -> &'static str {
    let pick = |v: &[&'static str]| v[seed as usize % v.len()];
    match score {
        90.. => pick(&["Это шедевр!", "Один в один! Я аж вздрогнул.", "Сильно! Мощно!", "Вот это поворот — от оригинала не отличить!"]),
        75.. => pick(&["Красиво! Лепота!", "Похоже! Я так и знал.", "Чётко! Мне нравится.", "А что, так можно было?"]),
        55.. => pick(&["Ну… что-то общее есть.", "Пойдёт. Но я бы не стал.", "Близко, но не то, братан.", "Терпимо. Ставлю «зачёт с натяжкой»."]),
        1.. => pick(&["Это фиаско, братан.", "Ты адекватный?", "Можно. А зачем?", "Ничего не понятно, но очень интересно."]),
        _ => pick(&["Я ничего не услышал. Ты где?", "Тишина в эфире…"]),
    }
}

/// Average quality of a player's attempts, 0–100.
fn avg_quality(g: &Game, i: usize) -> u32 { g.totals[i] / g.tries[i].max(1) }

/* ---------- demo stand for README screenshots (Linux only) ---------- */
#[cfg(not(windows))]
impl Game {
    pub(crate) fn demo(players: Vec<String>) -> Self {
        let mut g = Game::new(std::env::temp_dir().join("kj_demo_sounds"));
        let sr = 16000u32;
        let voice = crate::dub::demo_voice(2.4, sr, 3);
        g.players = players.clone();
        g.totals = vec![161, 148, 97];
        g.tries = vec![2, 2, 2];
        g.wins = vec![1, 1, 0];
        g.rounds = 5;
        g.round = 3;
        g.cur = Some(sounds::Sound { path: PathBuf::from("Это фиаско, братан.wav"), name: "Это фиаско, братан".into(), cat: 1 });
        g.ref_feat = Some(mimic::features(&voice, sr));
        g.reference = Some((voice.clone(), sr));
        let sc = |t, i, r, m| Some((voice.clone(), sr, mimic::MimicScore { total: t, intonation: i, rhythm: r, tempo: m }));
        g.attempts = vec![sc(86, 89, 82, 91), sc(71, 64, 78, 80), sc(43, 38, 52, 47)];
        g.revealed = vec![true, true, true];
        g.round_winner = Some(0);
        g.phase = GPhase::RoundDone;
        g
    }
}
