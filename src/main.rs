#![windows_subsystem = "windows"]
//! Караоке-судья — поиск караоке на YouTube, голос в колонки и оценка пения.

mod audio;
mod avatar;
mod cleanup;
mod ff;
mod apozh;
mod dsp;
mod dub;
mod separate;
mod speech;
mod game;
mod mimic;
mod sounds;
mod wave;
// YouTube search and the embedded player are Windows-only; the tests use the parser
#[cfg_attr(not(windows), allow(dead_code))]
mod search;
mod sfx;
mod theme;
mod ui_kit;

use audio::{Engine, EngineConfig, Outcome, Params, Phase};
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Stroke, Vec2};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use theme::*;
use ui_kit::{self as kit, Icon, Kind};

/* ---------- small Windows helpers ---------- */

#[cfg(windows)]
#[repr(C)]
struct SystemTime16([u16; 8]);
#[cfg(windows)]
#[repr(C)]
struct TimeZoneInfo { bias: i32, sn: [u16; 32], sd: SystemTime16, sb: i32, dn: [u16; 32], dd: SystemTime16, db: i32 }
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" { fn GetTimeZoneInformation(tz: *mut TimeZoneInfo) -> u32; }

pub fn utc_offset_secs() -> i64 {
    #[cfg(windows)]
    unsafe {
        let mut tz: TimeZoneInfo = std::mem::zeroed();
        let r = GetTimeZoneInformation(&mut tz);
        let extra = if r == 2 { tz.db } else { tz.sb };
        return -((tz.bias + extra) as i64) * 60;
    }
    #[allow(unreachable_code)]
    0
}

#[cfg(windows)]
#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(hwnd: *mut std::ffi::c_void, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> isize;
}

/// Open a URL, file or folder with its default app (browser, player, Explorer).
fn open(target: impl AsRef<std::ffi::OsStr>) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let op = wide(std::ffi::OsStr::new("open"));
        let file = wide(target.as_ref());
        let r = unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
        if r <= 32 {
            // fallback
            let _ = std::process::Command::new("cmd").args(["/C", "start", ""]).arg(target.as_ref()).spawn();
        }
    }
    #[cfg(not(windows))]
    { let _ = std::process::Command::new("xdg-open").arg(target).spawn(); }
}
fn home() -> PathBuf { std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")) }
/* ---------- where things live (portable: next to the program) ---------- */

/// Old places (before the portable layout).
fn old_data_dir() -> PathBuf { std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home).join("KaraokeJudge") }
fn old_record_dir() -> PathBuf { home().join("Music").join("Karaoke") }

/// The program's folder if we may write there, else the old per-user folder.
fn app_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
        if let Some(d) = exe_dir {
            let probe = d.join(".kj_write_test");
            if std::fs::write(&probe, b"ok").is_ok() { let _ = std::fs::remove_file(&probe); return d; }
        }
        old_data_dir()
    }).clone()
}
fn portable() -> bool { app_dir() != old_data_dir() }
fn record_dir() -> PathBuf { if portable() { app_dir().join("Записи") } else { old_record_dir() } }
fn data_dir() -> PathBuf { let d = if portable() { app_dir().join("Данные") } else { old_data_dir() }; let _ = std::fs::create_dir_all(&d); d }
fn data_file() -> PathBuf { data_dir().join("settings.txt") }
/// «Озвучка», «Пародист» — visible folders next to the program.
fn user_dir(name: &str) -> PathBuf { let d = app_dir().join(name); let _ = std::fs::create_dir_all(&d); d }

/// Move a file or a folder; across drives — copy, then remove.
fn move_any(from: &std::path::Path, to: &std::path::Path) -> bool {
    if std::fs::rename(from, to).is_ok() { return true; }
    fn copy_all(a: &std::path::Path, b: &std::path::Path) -> std::io::Result<()> {
        if a.is_dir() {
            std::fs::create_dir_all(b)?;
            for e in std::fs::read_dir(a)?.flatten() { copy_all(&e.path(), &b.join(e.file_name()))?; }
            Ok(())
        } else { std::fs::copy(a, b).map(|_| ()) }
    }
    if copy_all(from, to).is_ok() {
        let _ = if from.is_dir() { std::fs::remove_dir_all(from) } else { std::fs::remove_file(from) };
        true
    } else { false }
}

/// First start of the portable version: bring settings, recordings and clips over.
fn migrate_to_portable() {
    if !portable() || data_file().exists() { return; }
    let old = old_data_dir();
    for f in ["settings.txt", "highlights.txt"] {
        let (a, b) = (old.join(f), data_dir().join(f));
        if a.exists() && !b.exists() { move_any(&a, &b); }
    }
    for dname in ["Озвучка", "Пародист"] {
        let a = old.join(dname);
        if !a.exists() { continue; }
        let b = app_dir().join(dname);
        if !b.exists() { move_any(&a, &b); } else if let Ok(rd) = std::fs::read_dir(&a) {
            for e in rd.flatten() { let t = b.join(e.file_name()); if !t.exists() { move_any(&e.path(), &t); } }
        }
    }
    let (a, b) = (old_record_dir(), record_dir());
    if let Ok(rd) = std::fs::read_dir(&a) {
        let _ = std::fs::create_dir_all(&b);
        for e in rd.flatten() { let t = b.join(e.file_name()); if e.path().is_file() && !t.exists() { move_any(&e.path(), &t); } }
        let _ = std::fs::remove_dir(&a); // only if it is empty now
    }
}

/* ---------- settings & history ---------- */

#[derive(Clone)]
struct HistoryItem { score: u32, title: String, when: String }

struct Settings {
    speakers: bool, monitor: f32, reverb: f32, echo: f32, aec: bool, denoise: bool, mic_gain: f32,
    input: Option<String>, best: u32, history: Vec<HistoryItem>,
    voice_low: f32, voice_high: f32, crowd_sfx: bool, songs_total: u32, longest_best: f32,
    achs: Vec<(String, String)>, // (id, when)
    /// «Играют сегодня»: one list of names for karaoke and all games
    players: Vec<String>,
    /// who sings next in karaoke (index in `players`)
    singer: usize,
    /// colour scheme (theme::SCHEMES)
    scheme: usize,
    /// auto-tune for the monitor: 0 off, 1 soft, 2 «робот»
    autotune: u8,
}
impl Default for Settings {
    fn default() -> Self { Self { speakers: true, monitor: 0.4, reverb: 0.3, echo: 0.1, aec: true, denoise: false, mic_gain: 1.0, input: None, best: 0, history: vec![],
        voice_low: 0.0, voice_high: 0.0, crowd_sfx: true, songs_total: 0, longest_best: 0.0, achs: vec![], players: vec![], singer: 0, scheme: 0, autotune: 0 } }
}
impl Settings {
    fn has(&self, id: &str) -> bool { self.achs.iter().any(|(a, _)| a == id) }
    /// Grant an achievement; returns its title if it is new.
    fn grant(&mut self, id: &str) -> Option<&'static str> {
        if self.has(id) { return None; }
        let a = ACHS.iter().find(|a| a.id == id)?;
        self.achs.push((id.to_string(), audio::stamp()));
        Some(a.title)
    }
    fn load() -> Self {
        let mut s = Self::default();
        if let Ok(t) = std::fs::read_to_string(data_file()) {
            for line in t.lines() {
                let Some((k, v)) = line.split_once('=') else { continue };
                match k {
                    "speakers" => s.speakers = v == "1",
                    "monitor" => s.monitor = v.parse().unwrap_or(s.monitor),
                    "reverb" => s.reverb = v.parse().unwrap_or(s.reverb),
                    "echo" => s.echo = v.parse().unwrap_or(s.echo),
                    "aec" => s.aec = v == "1",
                    "denoise" => s.denoise = v == "1",
                    "mic_gain" => s.mic_gain = v.parse::<f32>().unwrap_or(1.0).clamp(0.25, 4.0),
                    "input" if !v.is_empty() => s.input = Some(v.to_string()),
                    "best" => s.best = v.parse().unwrap_or(0),
                    "voice_low" => s.voice_low = v.parse().unwrap_or(0.0),
                    "voice_high" => s.voice_high = v.parse().unwrap_or(0.0),
                    "crowd_sfx" => s.crowd_sfx = v == "1",
                    "songs_total" => s.songs_total = v.parse().unwrap_or(0),
                    "longest_best" => s.longest_best = v.parse().unwrap_or(0.0),
                    "player" if !v.trim().is_empty() => s.players.push(v.trim().to_string()),
                    "singer" => s.singer = v.parse().unwrap_or(0),
                    "scheme" => s.scheme = v.parse().unwrap_or(0),
                    "autotune" => s.autotune = v.parse::<u8>().unwrap_or(0).min(2),
                    "ach" => { if let Some((id, w)) = v.split_once('\t') { s.achs.push((id.into(), w.into())); } }
                    "h" => {
                        let p: Vec<&str> = v.splitn(3, '\t').collect();
                        if p.len() == 3 { s.history.push(HistoryItem { score: p[0].parse().unwrap_or(0), title: p[1].into(), when: p[2].into() }); }
                    }
                    _ => {}
                }
            }
        }
        s
    }
    fn save(&self) {
        // (the sound always goes to the Windows default output)
        let mut t = format!("speakers={}\nmonitor={}\nreverb={}\necho={}\naec={}\ndenoise={}\nmic_gain={}\ninput={}\nbest={}\n",
            self.speakers as u8, self.monitor, self.reverb, self.echo, self.aec as u8, self.denoise as u8, self.mic_gain,
            self.input.clone().unwrap_or_default(), self.best);
        t.push_str(&format!("voice_low={}\nvoice_high={}\ncrowd_sfx={}\nsongs_total={}\nlongest_best={}\n",
            self.voice_low, self.voice_high, self.crowd_sfx as u8, self.songs_total, self.longest_best));
        for (id, w) in &self.achs { t.push_str(&format!("ach={id}\t{w}\n")); }
        for p in &self.players { t.push_str(&format!("player={}\n", p.replace('\n', " "))); }
        t.push_str(&format!("singer={}\nscheme={}\nautotune={}\n", self.singer, self.scheme, self.autotune));
        for h in self.history.iter().take(30) {
            t.push_str(&format!("h={}\t{}\t{}\n", h.score, h.title.replace(['\t', '\n'], " "), h.when));
        }
        let _ = std::fs::write(data_file(), t);
    }
}

/* ---------- achievements ---------- */

struct Ach { id: &'static str, title: &'static str, desc: &'static str }
const ACHS: [Ach; 10] = [
    Ach { id: "first", title: "Первая песня", desc: "Спеть первую песню до конца" },
    Ach { id: "s60", title: "Неплохо!", desc: "Набрать 60 баллов" },
    Ach { id: "s80", title: "Отличник", desc: "Набрать 80 баллов" },
    Ach { id: "s90", title: "Звезда сцены", desc: "Набрать 90 баллов" },
    Ach { id: "hold5", title: "Длинная нота", desc: "Удержать одну ноту 5 секунд" },
    Ach { id: "marathon", title: "Марафон", desc: "Спеть 10 песен за один вечер" },
    Ach { id: "fan", title: "Меломан", desc: "Спеть 25 песен" },
    Ach { id: "record", title: "Растёшь!", desc: "Побить свой рекорд" },
    Ach { id: "voice", title: "Знаю свой голос", desc: "Пройти тест «Узнай свой голос»" },
    Ach { id: "boris", title: "Удивил Бориску", desc: "Набрать на 10 баллов больше его прогноза" },
];

/* ---------- voice types & note names ---------- */

/// (name, typical low, typical high) in MIDI.
const VOICES: [(&str, f32, f32); 6] = [
    ("Бас", 40.0, 64.0), ("Баритон", 45.0, 69.0), ("Тенор", 48.0, 72.0),
    ("Контральто", 53.0, 77.0), ("Меццо-сопрано", 57.0, 81.0), ("Сопрано", 60.0, 84.0),
];

fn voice_type(low: f32, high: f32) -> &'static str {
    let c = (low + high) / 2.0;
    VOICES.iter().min_by(|a, b| {
        let da = (c - (a.1 + a.2) / 2.0).abs() + 0.5 * (low - a.1).abs();
        let db = (c - (b.1 + b.2) / 2.0).abs() + 0.5 * (low - b.1).abs();
        da.total_cmp(&db)
    }).map(|v| v.0).unwrap_or("Баритон")
}

/// Songs that usually sit well for each voice (suggestions, click to search).
fn songs_for(voice: &str) -> &'static [&'static str] {
    match voice {
        "Бас" | "Баритон" => &["Кино — Группа крови", "Сплин — Выхода нет", "Ленинград — Экспонат", "Frank Sinatra — My Way", "Johnny Cash — Hurt"],
        "Тенор" => &["Король и Шут — Лесник", "Би-2 — Полковнику никто не пишет", "Мумий Тролль — Владивосток 2000", "Bon Jovi — It's My Life", "Queen — Bohemian Rhapsody"],
        "Контральто" | "Меццо-сопрано" => &["Земфира — Хочешь?", "Ночные Снайперы — 31 весна", "Adele — Rolling in the Deep", "Amy Winehouse — Back to Black", "Duffy — Mercy"],
        _ => &["ABBA — Dancing Queen", "Ёлка — Прованс", "Celine Dion — My Heart Will Go On", "Мумий Тролль — Утекай", "Kelly Clarkson — Since U Been Gone"],
    }
}

const NOTE_RU: [&str; 12] = ["До", "До♯", "Ре", "Ре♯", "Ми", "Фа", "Фа♯", "Соль", "Соль♯", "Ля", "Ля♯", "Си"];
/// MIDI → «Ля малой октавы».
fn note_name(m: f32) -> String {
    let r = m.round() as i32;
    let n = NOTE_RU[r.rem_euclid(12) as usize];
    let oct = match r.div_euclid(12) - 1 { 0 => "субконтроктавы", 1 => "контроктавы", 2 => "большой октавы", 3 => "малой октавы", 4 => "первой октавы", 5 => "второй октавы", 6 => "третьей октавы", _ => "четвёртой октавы" };
    format!("{n} {oct}")
}

/* ---------- best moments (index next to settings) ---------- */

fn highlights_file() -> PathBuf { data_file().with_file_name("highlights.txt") }
fn save_highlight(p: &std::path::Path, start: f32) {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let mut t = std::fs::read_to_string(highlights_file()).unwrap_or_default();
    t.push_str(&format!("{name}\t{start:.1}\n"));
    let _ = std::fs::write(highlights_file(), t);
}
fn load_highlights() -> HashMap<String, f32> {
    std::fs::read_to_string(highlights_file()).unwrap_or_default().lines()
        .filter_map(|l| l.split_once('\t')).filter_map(|(n, v)| v.parse().ok().map(|v| (n.to_string(), v))).collect()
}

/* ---------- embedded video (WebView2) ---------- */

/// Makes the YouTube watch page show only the player, filling the view;
/// prevents autoplay of the next video; reports progress and fullscreen
/// requests back to the app.
#[cfg_attr(not(windows), allow(dead_code))]
const YT_JS: &str = r#"
(function(){
  // a local clip for «Озвучка»: plain video, always silent (sound is played by the app); kj(t, play) seeks/plays, kj(-1,false) pauses
  if (location.protocol === 'file:') {
    function vid(){ return document.querySelector('video'); }
    function prep(){
      var v = vid(); if (!v) return;
      v.muted = true; v.controls = false; v.autoplay = false; v.loop = false;
      document.documentElement.style.background = '#000';
      if (document.body) { document.body.style.margin = '0'; document.body.style.background = '#000'; }
      v.style.cssText = 'position:fixed;left:0;top:0;width:100vw;height:100vh;object-fit:contain;background:#000';
    }
    window.kj = function(t, play){
      var v = vid(); if (!v) return; prep();
      // repeated calls are harmless: only jump if really off
      if (t >= 0 && (Math.abs(v.currentTime - t) > 0.3 || !play)) v.currentTime = t;
      if (play) { if (v.paused) v.play(); } else { v.pause(); }
    };
    document.addEventListener('DOMContentLoaded', function(){ prep(); var v = vid(); if (v) { v.pause(); v.addEventListener('play', prep); } });
    setTimeout(function(){ prep(); var v = vid(); if (v && !window.kjStarted) { v.pause(); v.currentTime = 0; } }, 300);
    return;
  }
  if (!/youtube\.com$/.test(location.hostname)) return;
  var css = 'html,body{overflow:hidden!important;background:#000!important}' +
    '#masthead-container,ytd-masthead,#secondary,#below,#comments,#guide,tp-yt-app-drawer,ytd-mini-guide-renderer{display:none!important}' +
    '#movie_player{position:fixed!important;top:0!important;left:0!important;width:100vw!important;height:100vh!important;z-index:2147483647!important;background:#000!important}' +
    '#movie_player .html5-video-container{position:absolute!important;top:0!important;left:0!important;width:100%!important;height:100%!important}' +
    '#movie_player video.html5-main-video{position:absolute!important;width:100%!important;height:100%!important;left:0!important;top:0!important;object-fit:contain!important}' +
    '.ytp-caption-window-container,.ytp-ce-element,.ytp-pause-overlay{display:none!important}';
  var first = null, lastW = 0, lastH = 0;
  function post(m){ try { window.ipc.postMessage(m); } catch(e){} }
  function tick(){
    if (!document.getElementById('kj-style') && (document.head || document.documentElement)) {
      var st = document.createElement('style'); st.id = 'kj-style'; st.textContent = css;
      (document.head || document.documentElement).appendChild(st);
    }
    if (innerWidth !== lastW || innerHeight !== lastH) { lastW = innerWidth; lastH = innerHeight; dispatchEvent(new Event('resize')); }
    var v = new URLSearchParams(location.search).get('v');
    if (first === null && v) first = v;
    var cancel = document.querySelector('.ytp-autonav-endscreen-upnext-cancel-button');
    if (cancel && cancel.offsetParent) cancel.click();
    if (first && v && v !== first) document.querySelectorAll('video').forEach(function(x){ x.pause(); });
    var vid = document.querySelector('video.html5-main-video');
    var ad = !!document.querySelector('.ad-showing');
    if (vid && vid.duration && isFinite(vid.duration)) post('t:' + vid.currentTime + ':' + vid.duration + ':' + (ad ? 1 : 0));
  }
  // our own fullscreen: the whole app window goes fullscreen
  document.addEventListener('click', function(e){
    var b = e.target && e.target.closest && e.target.closest('.ytp-fullscreen-button');
    if (b) { e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation(); post('fs'); }
  }, true);
  document.addEventListener('dblclick', function(e){
    if (e.target && e.target.closest && e.target.closest('#movie_player') && !e.target.closest('.ytp-chrome-bottom')) {
      e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation(); post('fs');
    }
  }, true);
  document.addEventListener('keydown', function(e){
    var t = e.target && e.target.tagName;
    if (t === 'INPUT()' || t === 'TEXTAREA') return;
    if (e.key === 'f' || e.key === 'F' || e.key === 'а' || e.key === 'А') { e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation(); post('fs'); }
    if (e.key === 'Escape') post('fsx');
  }, true);
  document.addEventListener('DOMContentLoaded', function(){ tick(); setTimeout(function(){ dispatchEvent(new Event('resize')); }, 800); });
  setInterval(tick, 500);
})();
"#;

#[derive(Default)]
struct VideoShared { cur: f32, dur: f32, ad: bool, fs_toggle: bool, fs_exit: bool }

#[cfg(windows)]
struct VideoView { wv: wry::WebView, _ctx: wry::WebContext, bounds: (i32, i32, u32, u32), visible: bool }
#[cfg(not(windows))]
struct VideoView;

#[cfg(windows)]
fn make_video(frame: &eframe::Frame, url: &str, shared: Arc<Mutex<VideoShared>>, ectx: egui::Context) -> Result<VideoView, String> {
    let dir = data_dir().join("webview");
    let mut wctx = wry::WebContext::new(Some(dir));
    let wv = wry::WebViewBuilder::new_with_web_context(&mut wctx)
        .with_url(url)
        .with_autoplay(true)
        .with_initialization_script(YT_JS)
        .with_background_color((0, 0, 0, 255))
        .with_ipc_handler(move |req: wry::http::Request<String>| {
            let m = req.body();
            if let Ok(mut s) = shared.lock() {
                if m == "fs" { s.fs_toggle = true; }
                else if m == "fsx" { s.fs_exit = true; }
                else if let Some(rest) = m.strip_prefix("t:") {
                    let p: Vec<&str> = rest.split(':').collect();
                    if p.len() == 3 { s.cur = p[0].parse().unwrap_or(0.0); s.dur = p[1].parse().unwrap_or(0.0); s.ad = p[2] == "1"; }
                }
            }
            ectx.request_repaint();
        })
        .with_bounds(wry::Rect { position: wry::dpi::PhysicalPosition::new(0, 0).into(), size: wry::dpi::PhysicalSize::new(1u32, 1u32).into() })
        .build_as_child(frame)
        .map_err(|e| e.to_string())?;
    Ok(VideoView { wv, _ctx: wctx, bounds: (0, 0, 1, 1), visible: true })
}
#[cfg(not(windows))]
fn make_video(_f: &eframe::Frame, _u: &str, _s: Arc<Mutex<VideoShared>>, _c: egui::Context) -> Result<VideoView, String> { Ok(VideoView) }

impl VideoView {
    #[cfg(windows)]
    fn load(&self, url: &str) { let _ = self.wv.load_url(url); }
    #[cfg(not(windows))]
    fn load(&self, _url: &str) {}
    #[cfg(windows)]
    fn place(&mut self, r: Option<Rect>, ppp: f32, show: bool) {
        let show = show && r.is_some();
        if let Some(r) = r {
            let b = ((r.min.x * ppp).round() as i32, (r.min.y * ppp).round() as i32,
                     (r.width() * ppp).round().max(1.0) as u32, (r.height() * ppp).round().max(1.0) as u32);
            if b != self.bounds {
                self.bounds = b;
                let _ = self.wv.set_bounds(wry::Rect {
                    position: wry::dpi::PhysicalPosition::new(b.0, b.1).into(),
                    size: wry::dpi::PhysicalSize::new(b.2, b.3).into(),
                });
            }
        }
        if show != self.visible { self.visible = show; let _ = self.wv.set_visible(show); }
    }
    #[cfg(not(windows))]
    fn place(&mut self, _r: Option<Rect>, _ppp: f32, _show: bool) {}
}

/* ---------- «Играют сегодня» ---------- */

/// Edit the list of today's players. Returns true if something changed.
fn roster_editor(ui: &mut egui::Ui, players: &mut Vec<String>, width: f32) -> bool {
    let mut changed = false;
    let mut rm = None;
    for (i, name) in players.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 6.0, player_color(i));
            if ui.add(egui::TextEdit::singleline(name).font(body(16.0)).margin(Vec2::new(12.0, 10.0)).desired_width(width - 80.0)).changed() { changed = true; }
            if kit::icon_button(ui, Icon::Close, 40.0).on_hover_text("Убрать").clicked() { rm = Some(i); }
        });
    }
    if let Some(i) = rm { players.remove(i); changed = true; }
    if players.len() < 10 && kit::link(ui, "+ Добавить игрока").clicked() { let n = players.len() + 1; players.push(format!("Игрок {n}")); changed = true; }
    changed
}

const PLAYER_COLORS: [Color32; 6] = [
    Color32::from_rgb(255, 77, 141), Color32::from_rgb(255, 184, 77), Color32::from_rgb(61, 214, 198),
    Color32::from_rgb(155, 140, 255), Color32::from_rgb(111, 220, 140), Color32::from_rgb(90, 169, 255),
];
fn player_color(i: usize) -> Color32 { PLAYER_COLORS[i % PLAYER_COLORS.len()] }

/// Today's players for a game (at least one).
fn roster(s: &Settings) -> Vec<String> {
    let v: Vec<String> = s.players.iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
    if v.is_empty() { vec!["Игрок 1".into(), "Игрок 2".into()] } else { v }
}

/* ---------- helpers ---------- */

fn fmt_time(s: f32) -> String { let s = s.max(0.0) as u32; format!("{}:{:02}", s / 60, s % 60) }
fn pretty_when(stamp: &str) -> String {
    // a damaged settings file must not crash the program: only well-formed stamps are reformatted
    match (stamp.get(8..10), stamp.get(5..7), stamp.get(11..13), stamp.get(14..16)) {
        (Some(d), Some(m), Some(h), Some(mi)) if stamp.is_ascii() => format!("{d}.{m} {h}:{mi}"),
        _ => stamp.to_string(),
    }
}
/// "Исполнитель - Песня (Караоке)" → ("Песня (Караоке)", "Исполнитель")
fn split_title(title: &str, fallback_artist: &str) -> (String, String) {
    for sep in [" — ", " – ", " - ", " | "] {
        if let Some((a, b)) = title.split_once(sep) {
            if !a.trim().is_empty() && !b.trim().is_empty() { return (b.trim().to_string(), a.trim().to_string()); }
        }
    }
    (title.trim().to_string(), fallback_artist.to_string())
}

const VERDICTS: [(u32, &str); 6] = [(90, "Звезда сцены!"), (80, "Отлично!"), (70, "Очень хорошо!"), (60, "Хорошо!"), (45, "Неплохо!"), (1, "Попробуйте ещё!")];
fn verdict_index(score: u32) -> Option<usize> { if score == 0 { None } else { VERDICTS.iter().position(|(min, _)| score >= *min) } }

const SUGGEST: [&str; 8] = [
    "Кино — Группа крови", "Король и Шут — Лесник", "Земфира — Хочешь?", "Сплин — Выхода нет",
    "Мумий Тролль — Владивосток 2000", "Ленинград — Экспонат", "Queen — Bohemian Rhapsody", "ABBA — Dancing Queen",
];

/* ---------- recordings on disk ---------- */

struct RecItem { path: PathBuf, title: String, when: String, secs: f32, score: Option<u32>, sort: String, best: Option<f32> }

const MONTHS: [&str; 12] = ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"];

/// "YYYY-MM-DD", "HH", "MM" → "11 ноября · 00:41" (year shown if not the current one)
fn human_when(date: &str, hh: &str, mm: &str) -> String {
    let p: Vec<&str> = date.split('-').collect();
    if p.len() != 3 { return format!("{date} · {hh}:{mm}"); }
    let m = p[1].parse::<usize>().unwrap_or(1).clamp(1, 12);
    let d = p[2].trim_start_matches('0');
    let this_year = audio::stamp().get(0..4).map(|y| y == p[0]).unwrap_or(true);
    if this_year { format!("{d} {} · {hh}:{mm}", MONTHS[m - 1]) } else { format!("{d} {} {} · {hh}:{mm}", MONTHS[m - 1], p[0]) }
}

/// Make a Windows-safe file name part.
fn safe_name(s: &str) -> String {
    let t: String = s.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { ' ' } else { c }).collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(90).collect();
    t.trim_matches(|c: char| c == '.' || c == ' ').to_string()
}

/// Rename a finished recording to "YYYY-MM-DD HH-MM — Песня — 87.wav".
fn name_recording(p: &std::path::Path, title: &str, score: u32) -> PathBuf {
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    // the engine names files "karaoke_YYYY-MM-DD_HH-MM-SS"
    let st = stem.strip_prefix("karaoke_").unwrap_or(stem);
    let (date, time) = st.split_once('_').unwrap_or((st, "00-00-00"));
    let hm: String = time.chars().take(5).collect();
    let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let base = format!("{date} {hm} — {} — {score}", safe_name(title));
    let mut target = dir.join(format!("{base}.wav"));
    let mut k = 2;
    while target.exists() { target = dir.join(format!("{base} ({k}).wav")); k += 1; }
    match std::fs::rename(p, &target) { Ok(_) => target, Err(_) => p.to_path_buf() }
}

fn parse_recording(path: &std::path::Path) -> Option<RecItem> {
    let stem = path.file_stem()?.to_str()?.to_string();
    let secs = audio::wav_seconds(path).unwrap_or(0.0);
    if let Some(st) = stem.strip_prefix("karaoke_") {
        // old naming: karaoke_YYYY-MM-DD_HH-MM-SS
        let (date, time) = st.split_once('_')?;
        let (hh, mm) = (time.get(0..2)?, time.get(3..5)?);
        return Some(RecItem { path: path.to_path_buf(), title: "Без названия".into(), when: human_when(date, hh, mm), secs, score: None, sort: format!("{date} {hh}-{mm}"), best: None });
    }
    let parts: Vec<&str> = stem.split(" — ").collect();
    if parts.len() >= 3 {
        let dt = parts[0];
        let (date, hm) = dt.split_once(' ')?;
        let (hh, mm) = (hm.get(0..2)?, hm.get(3..5)?);
        let last = parts[parts.len() - 1];
        let score = last.split_whitespace().next().and_then(|x| x.parse().ok());
        let title = parts[1..parts.len() - 1].join(" — ");
        return Some(RecItem { path: path.to_path_buf(), title, when: human_when(date, hh, mm), secs, score, sort: dt.to_string(), best: None });
    }
    Some(RecItem { path: path.to_path_buf(), title: stem.clone(), when: String::new(), secs, score: None, sort: String::new(), best: None })
}

fn load_recordings() -> Vec<RecItem> {
    let mut v: Vec<RecItem> = std::fs::read_dir(record_dir()).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("wav")).unwrap_or(false))
        .filter_map(|p| parse_recording(&p)).collect()).unwrap_or_default();
    v.sort_by(|a, b| b.sort.cmp(&a.sort));
    let hl = load_highlights();
    for it in &mut v {
        if let Some(n) = it.path.file_name().and_then(|n| n.to_str()) { it.best = hl.get(n).copied(); }
    }
    v
}

fn show_in_folder(p: &std::path::Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", p.display())).spawn();
    }
    #[cfg(not(windows))]
    { if let Some(d) = p.parent() { open(d); } }
}

/* ---------- app state ---------- */

enum SearchMsg { Results(u64, Result<Vec<search::Video>, String>), Thumb(String, egui::ColorImage) }

struct ResultView { score: u32, recording: Option<PathBuf>, new_best: bool, shown: Instant, title: Option<String>, artist: String,
    best: Option<f32>, new_achs: Vec<&'static str> }

#[derive(Clone)]
struct Song { id: String, title: String, channel: String }

#[derive(PartialEq, Clone, Copy)]
enum Screen { Home, Session, Result, Recordings, Profile, Game, Dub, Apozh }

struct App {
    params: Arc<Params>,
    settings: Settings,
    engine: Option<Engine>,
    result: Option<ResultView>,
    toast: Option<(String, Instant)>,
    note: Option<(String, Instant)>, // engine warnings shown in the session column
    show_settings: bool,
    devices: (Vec<String>, Vec<String>),
    // search
    query: String,
    searching: bool,
    search_gen: u64,
    last_query: String,
    results: Vec<search::Video>,
    search_err: Option<String>,
    thumbs: HashMap<String, egui::TextureHandle>,
    tx: mpsc::Sender<SearchMsg>,
    rx: mpsc::Receiver<SearchMsg>,
    // session
    in_session: bool,
    song: Option<Song>,
    video: Option<VideoView>,
    video_hidden: bool,
    video_rect: Option<Rect>,
    video_shared: Arc<Mutex<VideoShared>>,
    pending_video: Option<String>,
    close_video: bool,
    fullscreen: bool,
    voice_hist: VecDeque<(f32, bool)>,
    last_bar: Instant,
    // microphone check
    mic_test: Option<audio::MicTest>,
    mic_test_user: bool,
    mic_err: Option<String>,
    mic_heard: Option<Instant>,
    // recordings
    show_recs: bool,
    recs: Vec<RecItem>,
    player: Option<(PathBuf, audio::Player)>,
    confirm_delete: Option<PathBuf>,
    // Бумбокс Бориска
    boris: avatar::Boris,
    last_frame: Instant,
    voice_level: f32,
    sing: Option<avatar::Sing>,
    pitch_center: f32,
    music_level: f32,
    default_out: Option<String>,
    prediction: Option<u32>,
    sfx: Option<audio::Player>,
    show_profile: bool,
    vt: Option<VoiceTest>,
    game: Option<game::Game>,
    show_game: bool,
    game_player: Option<audio::Player>,
    dub: Option<dub::Dub>,
    show_dub: bool,
    show_roster: bool,
    show_games: bool,
    apozh: Option<apozh::Apozh>,
    show_apozh: bool,
}

/// State of the «Узнай свой голос» test.
struct VoiceTest { mon: audio::PitchMonitor, step: u8, low: Option<f32>, high: Option<f32>, recent: VecDeque<(Instant, f32)>, cur: Option<f32> }

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        migrate_to_portable();
        let settings = Settings::load();
        theme::set_scheme(settings.scheme);
        theme::apply(&cc.egui_ctx);
        let params = Arc::new(Params::default());
        params.monitor.set(settings.monitor);
        params.reverb.set(settings.reverb);
        params.echo.set(settings.echo);
        params.aec.store(settings.aec, std::sync::atomic::Ordering::Relaxed);
        params.denoise.store(settings.denoise, std::sync::atomic::Ordering::Relaxed);
        params.autotune.store(settings.autotune, std::sync::atomic::Ordering::Relaxed);
        params.mic_gain.set(settings.mic_gain);
        let (tx, rx) = mpsc::channel();
        #[allow(unused_mut)]
        let mut app = Self {
            params, settings, engine: None, result: None, toast: None, note: None, show_settings: false, devices: audio::list_devices(),
            query: String::new(), searching: false, search_gen: 0, last_query: String::new(), results: vec![], search_err: None,
            thumbs: HashMap::new(), tx, rx,
            in_session: false, song: None, video: None, video_hidden: false, video_rect: None,
            video_shared: Arc::new(Mutex::new(VideoShared::default())), pending_video: None, close_video: false, fullscreen: false,
            voice_hist: VecDeque::new(), last_bar: Instant::now(),
            mic_test: None, mic_test_user: false, mic_err: None, mic_heard: None,
            show_recs: false, recs: vec![], player: None, confirm_delete: None,
            boris: avatar::Boris::default(), last_frame: Instant::now(), voice_level: 0.0,
            sing: None, pitch_center: 0.0, music_level: 0.0, default_out: audio::default_output_name(),
            prediction: None, sfx: None, show_profile: false, vt: None, game: None, show_game: false, game_player: None, dub: None, show_dub: false, show_roster: false, show_games: false, apozh: None, show_apozh: false,
        };
        // demo stand for README screenshots (Linux only, never in the Windows program)
        #[cfg(not(windows))]
        if let Ok(which) = std::env::var("KJ_DEMO") {
            let players: Vec<String> = ["Макс", "Слава", "Аня"].iter().map(|s| s.to_string()).collect();
            app.settings.players = players.clone();
            app.settings.singer = 1;
            app.settings.history = vec![
                HistoryItem { score: 91, title: "Группа крови · Макс".into(), when: "2026-11-13 21:40:00".into() },
                HistoryItem { score: 84, title: "Лесник · Аня".into(), when: "2026-11-13 21:32:00".into() },
                HistoryItem { score: 77, title: "Хочешь? · Слава".into(), when: "2026-11-13 21:25:00".into() },
            ];
            app.mic_heard = Some(Instant::now());
            match which.as_str() {
                "games" => app.show_games = true,
                "parodist" => { app.game = Some(game::Game::demo(players.clone())); app.show_game = true; app.boris.queue("Очко получает Макс! Качество 86 из 100. Это шедевр!".into()); }
                "dub" => { app.dub = Some(dub::Dub::demo(players.clone(), false)); app.show_dub = true; app.boris.queue("Микрофон у Макса! Не подведи.".into()); }
                "dubsetup" => app.open_dub(),
                "editor" => { app.dub = Some(dub::Dub::demo(players.clone(), true)); app.show_dub = true; }
                "apozh" => { app.apozh = Some(apozh::Apozh::demo(players.clone())); app.show_apozh = true; app.boris.queue("Перевернул! Макс, слушай и повторяй как слышишь — даже если это бред.".into()); }
                "result" => {
                    app.result = Some(ResultView { score: 91, recording: Some(PathBuf::from("x")), new_best: true, shown: Instant::now() - Duration::from_secs(3),
                        title: Some("Группа крови · Макс".into()), artist: "Кино".into(), best: Some(42.0), new_achs: vec!["Отличник"] });
                }
                _ => {}
            }
        }
        app
    }

    fn screen(&self) -> Screen {
        if self.show_apozh { Screen::Apozh } else if self.show_dub { Screen::Dub } else if self.show_game { Screen::Game } else if self.show_recs { Screen::Recordings } else if self.show_profile { Screen::Profile } else if self.result.is_some() { Screen::Result } else if self.in_session { Screen::Session } else { Screen::Home }
    }

    fn say(&mut self, s: impl Into<String>) { self.toast = Some((s.into(), Instant::now())); }

    fn start(&mut self, auto: bool) {
        self.player = None;
        self.sfx = None;
        self.vt = None;
        self.show_recs = false;
        self.show_profile = false;
        self.show_game = false;
        self.show_dub = false;
        self.show_apozh = false;
        if let Some(a) = &mut self.apozh { a.mic = None; a.phase = apozh::APhase::Setup; }
        self.game_player = None;
        if let Some(d) = &mut self.dub { d.mic = None; d.phase = dub::DPhase::Setup; }
        if let Some(g) = &mut self.game { g.mic = None; g.phase = game::GPhase::Setup; }
        self.boris_intro();
        self.mic_test = None;
        self.mic_test_user = false;
        self.voice_hist.clear();
        let cfg = EngineConfig {
            speakers_mode: self.settings.speakers,
            input_name: self.settings.input.clone(),
            output_name: None,
            record_dir: record_dir(),
            auto,
        };
        match Engine::start(cfg, self.params.clone()) {
            Ok(e) => { self.engine = Some(e); self.in_session = true; }
            Err(e) => self.say(e),
        }
    }

    /// Stop the session. Shows the result if the singing part had begun.
    fn stop(&mut self) {
        if let Some(e) = self.engine.take() {
            let singing = e.status.lock().map(|s| s.phase == Phase::Singing).unwrap_or(false);
            let Outcome { score, recording, longest_note, best_start, .. } = e.finish();
            if !singing {
                if let Some(p) = recording { let _ = std::fs::remove_file(p); }
                return;
            }
            let prev_best = self.settings.best;
            let new_best = score > prev_best;
            if new_best { self.settings.best = score; }
            let singer = self.current_singer();
            let title = self.song.as_ref().map(|s| s.title.clone()).map(|t| match &singer { Some(n) => format!("{t} · {n}"), None => t });
            let free = match &singer { Some(n) => format!("Свободное пение · {n}"), None => "Свободное пение".into() };
            let recording = recording.map(|p| name_recording(&p, title.as_deref().unwrap_or(&free), score));
            if !self.settings.players.is_empty() { self.settings.singer = (self.settings.singer + 1) % self.settings.players.len(); }
            let best = if score > 0 { best_start } else { None };
            if let (Some(p), Some(b)) = (&recording, best) { save_highlight(p, b); }
            self.settings.longest_best = self.settings.longest_best.max(longest_note);
            let mut new_achs = Vec::new();
            if score > 0 {
                self.settings.history.insert(0, HistoryItem {
                    score, title: title.clone().unwrap_or_else(|| "Без названия".into()), when: audio::stamp(),
                });
                self.settings.songs_total += 1;
                let today = audio::stamp()[..10].to_string();
                let tonight = self.settings.history.iter().filter(|h| h.when.starts_with(&today)).count();
                let mut ids: Vec<&str> = vec!["first"];
                if score >= 60 { ids.push("s60"); }
                if score >= 80 { ids.push("s80"); }
                if score >= 90 { ids.push("s90"); }
                if longest_note >= 5.0 { ids.push("hold5"); }
                if tonight >= 10 { ids.push("marathon"); }
                if self.settings.songs_total >= 25 { ids.push("fan"); }
                if new_best && prev_best > 0 { ids.push("record"); }
                if self.prediction.map(|p| score >= p + 10).unwrap_or(false) { ids.push("boris"); }
                for id in ids { if let Some(t) = self.settings.grant(id) { new_achs.push(t); } }
            }
            self.settings.save();
            // Boris compares the score with his prediction
            if let (Some(p), true) = (self.prediction, score > 0) {
                let d = score as i32 - p as i32;
                let txt = if d >= 10 { format!("Ставил на {p}… Ладно, ты меня удивил!") }
                    else if d <= -10 { format!("Ставил на {p}. Переоценил я тебя.") }
                    else { format!("Ставил на {p}. Я гений, признай.") };
                self.boris.queue(txt);
            }
            // the hall reacts
            if self.settings.crowd_sfx {
                let seed = audio::stamp().bytes().fold(score.wrapping_add(1), |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
                if let Some(v) = sfx::for_score(score, seed) {
                    self.sfx = audio::Player::from_samples(v, sfx::SR, &None, 0.8).ok();
                }
            }
            let (t, artist) = match &self.song {
                Some(s) => { let (a, b) = split_title(&s.title, &s.channel); (Some(a), b) }
                None => (None, String::new()),
            };
            self.result = Some(ResultView { score, recording, new_best, shown: Instant::now(), title: t, artist, best, new_achs });
            self.fullscreen = false;
        }
    }

    /// Drop the current session without a result.
    fn cancel(&mut self) {
        if let Some(e) = self.engine.take() { if let Some(p) = e.finish().recording { let _ = std::fs::remove_file(p); } }
    }

    fn go_home(&mut self) {
        self.cancel();
        self.sfx = None;
        self.result = None;
        self.in_session = false;
        self.close_video = true;
        self.fullscreen = false;
        self.note = None;
    }

    fn do_search(&mut self, ctx: &egui::Context, q: String) {
        let q = q.trim().to_string();
        if q.is_empty() { return; }
        self.query = q.clone();
        self.last_query = q.clone();
        self.search_gen += 1;
        self.searching = true;
        self.search_err = None;
        self.results.clear();
        let (tx, gen, ctx) = (self.tx.clone(), self.search_gen, ctx.clone());
        let have: Vec<String> = self.thumbs.keys().cloned().collect();
        std::thread::spawn(move || {
            let res = search::search(&q);
            let ids: Vec<String> = res.as_ref().map(|v| v.iter().map(|x| x.id.clone()).collect()).unwrap_or_default();
            let _ = tx.send(SearchMsg::Results(gen, res));
            ctx.request_repaint();
            for id in ids {
                if have.contains(&id) { continue; }
                if let Ok(bytes) = search::thumbnail(&id) {
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        let img = img.to_rgba8();
                        let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], &img);
                        let _ = tx.send(SearchMsg::Thumb(id, ci));
                        ctx.request_repaint();
                    }
                }
            }
        });
    }

    fn play_song(&mut self, v: &search::Video) {
        self.cancel();
        self.result = None;
        self.song = Some(Song { id: v.id.clone(), title: v.title.clone(), channel: v.channel.clone() });
        self.pending_video = Some(v.id.clone());
        self.video_hidden = false;
        if let Ok(mut s) = self.video_shared.lock() { s.cur = 0.0; s.dur = 0.0; }
        self.default_out = audio::default_output_name();
        self.start(true);
        self.in_session = true;

    }

    fn sing_again(&mut self) {
        self.result = None;
        match self.song.clone() {
            Some(s) => {
                self.pending_video = Some(s.id.clone());
                self.video_hidden = false;
                if let Ok(mut sh) = self.video_shared.lock() { sh.cur = 0.0; sh.dur = 0.0; }
                self.start(true);
            }
            None => self.start(false),
        }
        self.in_session = true;
    }

    /// Boris remembers this song / evening and makes a prediction.
    fn boris_intro(&mut self) {
        let hist = &self.settings.history;
        let mut seed = audio::stamp().bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        let mut rnd = || { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; seed };
        let recent: Vec<u32> = hist.iter().take(5).map(|h| h.score).collect();
        let base = if recent.is_empty() { 55.0 } else { recent.iter().sum::<u32>() as f32 / recent.len() as f32 };
        let guess = (base + (rnd() % 17) as f32 - 8.0).clamp(25.0, 95.0);
        let p = ((guess / 5.0).round() * 5.0) as u32;
        let mut parts: Vec<String> = Vec::new();
        if let Some(song) = &self.song {
            let short: String = split_title(&song.title, "").0.chars().take(28).collect();
            let same: Vec<&HistoryItem> = hist.iter().filter(|h| h.title == song.title).collect();
            if let Some(last) = same.first() {
                parts.push(if same.len() >= 2 { format!("Опять «{short}»? Уже {}-й раз.", same.len() + 1) }
                           else { format!("«{short}» снова? В прошлый раз было {}.", last.score) });
            }
        }
        if parts.is_empty() {
            let today = audio::stamp()[..10].to_string();
            let n = hist.iter().filter(|h| h.when.starts_with(&today)).count();
            if n >= 2 { parts.push(format!("{}-я песня за вечер. Держишься?", n + 1)); }
        }
        parts.push(format!("Ставлю на {p}."));
        self.prediction = Some(p);
        self.boris.queue(parts.join(" "));
    }

    /* =================== BORIS =================== */

    fn situation(&self, status: &Option<audio::Status>) -> avatar::Situation {
        use avatar::Situation as S;
        if let Some(r) = &self.result {
            return match r.score { 0 => S::ResultZero, 90.. => S::ResultTop, 75.. => S::ResultGood, 55.. => S::ResultMid, _ => S::ResultLow };
        }
        if !self.in_session || self.show_recs || self.show_profile || self.show_game || self.show_dub || self.show_apozh { return S::Home; }
        match status {
            None => S::Waiting,
            Some(s) => match s.phase {
                Phase::Waiting => S::Waiting,
                Phase::Calibrating => S::Intro,
                Phase::Singing if s.silent_for > 6.0 && s.music_heard => S::Silent,
                Phase::Singing if s.voice_on && s.quality > 0.8 => S::SingingGood,
                Phase::Singing if s.voice_on && s.quality > 0.0 && s.quality < 0.5 => S::SingingBad,
                Phase::Singing => S::Singing,
            },
        }
    }

    /// Boris with the bubble underneath (home and result screens).
    fn boris_column(&mut self, ui: &mut egui::Ui, size: f32, width: f32) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, size + 76.0), egui::Sense::hover());
        let br = Rect::from_min_size(Pos2::new(rect.center().x - size / 2.0, rect.top()), Vec2::splat(size));
        let (level, sing, music) = (self.voice_level, self.sing, self.music_level);
        self.boris.draw(ui.painter(), br, level, sing, music);
        if let Some((t, a)) = self.boris.phrase() {
            let t = t.to_string();
            let sz = avatar::bubble_size(ui, &t, width);
            let anchor = Pos2::new(rect.center().x - sz.x / 2.0, br.bottom() + 14.0);
            avatar::bubble(ui, ui.painter(), anchor, Pos2::new(rect.center().x, br.bottom() - 6.0), &t, a, width);
        }
    }

    /* =================== HOME =================== */

    fn sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // logo
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(44.0), egui::Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(14), ACCENT());
            kit::icon(ui.painter(), Icon::Mic, r.center(), 22.0, ON_ACCENT());
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.label(RichText::new("Караоке-судья").font(display_bold(18.0)));
                ui.label(RichText::new("Спойте — оценка начнётся сама").font(body(13.0)).color(MUTED()));
            });
        });
        ui.add_space(20.0);

        // search
        let mut go = false;
        ui.horizontal(|ui| {
            let w = ui.available_width() - 88.0;
            let left = ui.cursor().left();
            let te = egui::TextEdit::singleline(&mut self.query)
                .hint_text(RichText::new("Песня или исполнитель").color(PLACEHOLDER()))
                .font(body(15.0))
                .margin(egui::Margin { left: 40, right: 12, top: 14, bottom: 14 })
                .min_size(Vec2::new(w - 52.0, 48.0))
                .desired_width(w - 52.0);
            let r = ui.add(te);
            kit::icon(ui.painter(), Icon::Search, Pos2::new(left + 20.0, r.rect.center().y), 17.0, MUTED());
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) { go = true; }
            if kit::button_ex(ui, Kind::Primary, "Найти", None, Vec2::new(80.0, 48.0), bold(14.0), 12).clicked() { go = true; }
        });
        if go { let q = self.query.clone(); self.do_search(ctx, q); }
        ui.add_space(18.0);

        if self.searching {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.add(egui::Spinner::new().size(30.0).color(ACCENT()));
                ui.label(RichText::new("Ищу на YouTube…").color(MUTED()));
            });
            return;
        }
        if let Some(err) = self.search_err.clone() {
            kit::warning(ui, &err, ui.available_width());
            ui.add_space(8.0);
            if kit::link(ui, "Открыть поиск в браузере").clicked() { open(search::search_url(&self.last_query)); }
            if kit::link(ui, "Назад").clicked() { self.search_err = None; }
            return;
        }
        if !self.results.is_empty() {
            ui.horizontal(|ui| {
                kit::section_label(ui, "НАЙДЕНО");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if kit::link(ui, "Очистить").clicked() { self.results.clear(); }
                });
            });
            ui.add_space(6.0);
            let mut clicked = None;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for v in &self.results { if self.result_row(ui, v) { clicked = Some(v.clone()); } }
                ui.add_space(6.0);
                if kit::link(ui, "Открыть эти результаты в браузере").clicked() { open(search::search_url(&self.last_query)); }
            });
            if let Some(v) = clicked { self.play_song(&v); }
            return;
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            kit::section_label(ui, "ПОПУЛЯРНОЕ");
            ui.add_space(6.0);
            let mut pick = None;
            for s in SUGGEST {
                let (song, artist) = split_title(s, "");
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), egui::Sense::click());
                let p = ui.painter();
                if resp.hovered() { p.rect_filled(rect, CornerRadius::same(12), SURFACE_2()); ctx.set_cursor_icon(egui::CursorIcon::PointingHand); }
                let chip = Rect::from_center_size(Pos2::new(rect.left() + 26.0, rect.center().y), Vec2::splat(36.0));
                p.rect_filled(chip, CornerRadius::same(10), CHIP());
                kit::icon(p, Icon::Play, chip.center() + Vec2::new(1.0, 0.0), 14.0, ACCENT_TEXT());
                let w = rect.width() - 64.0;
                let g1 = kit::galley(ui, &song, semibold(15.0), TEXT(), w, 1);
                let g2 = kit::galley(ui, &artist, body(13.0), MUTED(), w, 1);
                let x = rect.left() + 56.0;
                let top = rect.center().y - (g1.size().y + g2.size().y) / 2.0;
                let h1 = g1.size().y;
                ui.painter().galley(Pos2::new(x, top), g1, TEXT());
                ui.painter().galley(Pos2::new(x, top + h1), g2, MUTED());
                if resp.clicked() { pick = Some(s.to_string()); }
            }
            if let Some(s) = pick { self.do_search(ctx, s); }

            ui.add_space(18.0);
            let mut again = None;
            egui::Frame::new().fill(SURFACE_2()).corner_radius(18.0).inner_margin(18.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                kit::section_label(ui, "ПОСЛЕДНИЕ ВЫСТУПЛЕНИЯ");
                ui.add_space(8.0);
                if self.settings.history.is_empty() {
                    ui.label(RichText::new("Пока пусто. Спойте что-нибудь!").font(body(14.0)).color(MUTED()));
                }
                for h in self.settings.history.iter().take(12) {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), egui::Sense::click());
                    let p = ui.painter();
                    if resp.hovered() { p.rect_filled(rect.expand2(Vec2::new(6.0, 0.0)), CornerRadius::same(10), SELECTED()); ctx.set_cursor_icon(egui::CursorIcon::PointingHand); }
                    let b = Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - 15.0), Vec2::new(40.0, 30.0));
                    let (bg, fg) = if h.score >= 80 { (AMBER_SOFT(), AMBER()) } else { (CHIP(), TEXT_2()) };
                    p.rect_filled(b, CornerRadius::same(10), bg);
                    p.text(b.center(), Align2::CENTER_CENTER, h.score.to_string(), display_bold(14.0), fg);
                    let wd = kit::galley(ui, &pretty_when(&h.when), body(12.0), MUTED(), f32::INFINITY, 1);
                    let tw = rect.width() - 56.0 - wd.size().x - 8.0;
                    let g = kit::galley(ui, &h.title, body(14.0), TEXT(), tw, 1);
                    let gy = g.size().y;
                    p.galley(Pos2::new(rect.left() + 52.0, rect.center().y - gy / 2.0), g, TEXT());
                    let wy = wd.size().y; let wx = wd.size().x;
                    p.galley(Pos2::new(rect.right() - wx, rect.center().y - wy / 2.0), wd, MUTED());
                    if resp.clicked() { again = Some(h.title.clone()); }
                    resp.on_hover_text("Найти эту песню ещё раз");
                }
            });
            if let Some(t) = again { self.do_search(ctx, t); }
        });
    }

    /// Search result row in the sidebar. Returns true when clicked.
    fn result_row(&self, ui: &mut egui::Ui, v: &search::Video) -> bool {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 74.0), egui::Sense::click());
        let selected = self.song.as_ref().map(|s| s.id == v.id).unwrap_or(false);
        let p = ui.painter().clone();
        if resp.hovered() || selected { p.rect_filled(rect, CornerRadius::same(14), SURFACE_2()); }
        if selected { p.rect_stroke(rect, CornerRadius::same(14), Stroke::new(1.5_f32, ACCENT()), egui::StrokeKind::Inside); }
        let th = Rect::from_min_size(rect.left_top() + Vec2::new(6.0, 7.0), Vec2::new(107.0, 60.0));
        match self.thumbs.get(&v.id) {
            Some(t) => { ui.put(th, egui::Image::new(egui::load::SizedTexture::new(t.id(), th.size())).corner_radius(10.0)); }
            None => { p.rect_filled(th, CornerRadius::same(10), CHIP()); kit::icon(&p, Icon::Play, th.center(), 16.0, MUTED()); }
        }
        let dg = kit::galley(ui, &v.duration, semibold(11.0), Color32::WHITE, f32::INFINITY, 1);
        let br = Rect::from_min_size(th.right_bottom() - dg.size() - Vec2::new(9.0, 5.0), dg.size() + Vec2::new(8.0, 2.0));
        p.rect_filled(br, CornerRadius::same(5), Color32::from_black_alpha(210));
        p.galley(br.min + Vec2::new(4.0, 1.0), dg, Color32::WHITE);
        if resp.hovered() {
            p.circle_filled(th.center(), 16.0, Color32::from_black_alpha(160));
            kit::icon(&p, Icon::Play, th.center() + Vec2::new(1.0, 0.0), 14.0, Color32::WHITE);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let x = th.right() + 12.0;
        let w = rect.right() - x - 8.0;
        let g = kit::galley(ui, &v.title, semibold(14.0), TEXT(), w, 2);
        let gh = g.size().y;
        p.galley(Pos2::new(x, rect.top() + 9.0), g, TEXT());
        let g2 = kit::galley(ui, &v.channel, body(12.0), MUTED(), w, 1);
        p.galley(Pos2::new(x, rect.top() + 11.0 + gh), g2, MUTED());
        ui.add_space(2.0);
        resp.clicked()
    }

    fn header_home(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            let running = self.engine.is_some();
            let mut sp = self.settings.speakers;
            if kit::segmented(ui, &mut sp, !running) { self.set_mode(sp); }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if kit::ghost(ui, "Записи", Some(Icon::Folder)).clicked() { self.open_recordings(None); }
                if kit::ghost(ui, "Звук", Some(Icon::Sliders)).clicked() { self.show_settings = true; self.default_out = audio::default_output_name(); }
                if kit::ghost(ui, "Профиль", Some(Icon::User)).clicked() { self.show_profile = true; }
                // games stand out: a button of their own colour
                let g = ui.add(egui::Button::new(RichText::new("🎮  Мини-игры").font(bold(15.0)).color(Color32::BLACK)).fill(AMBER()).corner_radius(12.0).min_size(Vec2::new(0.0, 40.0)));
                if g.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                if g.clicked() { self.show_games = true; }
            });
        });
    }

    fn set_mode(&mut self, speakers: bool) {
        self.settings.speakers = speakers;
        self.settings.monitor = if speakers { self.settings.monitor.min(0.6) } else { 0.8 };
        self.params.monitor.set(self.settings.monitor);
        self.settings.save();
    }

    fn mic_badge(&mut self, ui: &mut egui::Ui) {
        #[cfg(not(windows))]
        if std::env::var_os("KJ_DEMO").is_some() { kit::badge(ui, "Микрофон работает", OK_SOFT(), OK_TEXT(), Some(OK()), None); return; }
        if let Some(e) = &self.mic_err {
            let e = e.clone();
            kit::badge(ui, "Микрофон не найден", AMBER_WARN_BG(), AMBER_WARN_TEXT(), Some(AMBER()), None).on_hover_text(e);
        } else if self.mic_heard.map(|t| t.elapsed().as_secs_f32() < 4.0).unwrap_or(false) {
            kit::badge(ui, "Микрофон работает", OK_SOFT(), OK_TEXT(), Some(OK()), None);
        } else if self.mic_test.is_some() {
            kit::badge(ui, "Микрофон молчит — скажите что-нибудь", AMBER_WARN_BG(), AMBER_WARN_TEXT(), Some(AMBER()), None);
        }
    }

    fn home(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(8.0);
            self.mic_badge(ui);
            ui.add_space(22.0);
            let w = ui.available_width().min(920.0);
            let bw = 250.0;
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(w - bw - 16.0, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.label(RichText::new("Выберите песню —").font(display(48.0)).color(TEXT()));
                    ui.label(RichText::new("судья уже слушает").font(display(48.0)).color(ACCENT()));
                    ui.add_space(16.0);
                    kit::text(ui, "Найдите караоке слева — клип откроется здесь, а оценка начнётся сама, как только заиграет музыка.", body(18.0), TEXT_2(), (w - bw - 16.0).min(560.0));
                    ui.add_space(24.0);
                    ui.horizontal(|ui| {
                        if kit::button_ex(ui, Kind::Primary, "Петь", Some(Icon::Play), Vec2::new(150.0, 52.0), bold(16.0), 14).clicked() {
                            self.result = None; self.song = None; self.start(false);
                        }
                        ui.add_space(6.0);
                        ui.label(RichText::new("или выберите песню").font(body(14.0)).color(MUTED()));
                    });
                    ui.add_space(18.0);
                    self.who_sings(ui);
                });
                self.boris_column(ui, 170.0, bw);
            });
            ui.add_space(24.0);

            // three steps
            let steps = [
                ("01", "Найдите песню", "Введите название слева или выберите из популярного."),
                ("02", "Не пойте вступление", "Первые секунды программа слушает, как музыка звучит в микрофоне."),
                ("03", "Пойте!", "Когда песня закончится, судья сам покажет оценку."),
            ];
            let gap = 16.0;
            let cw = ((w - gap * 2.0) / 3.0).max(180.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (n, t, d) in steps {
                    ui.allocate_ui_with_layout(Vec2::new(cw, 170.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(18.0).inner_margin(22.0).show(ui, |ui| {
                            ui.set_width(cw - 44.0);
                            ui.set_min_height(124.0);
                            ui.label(RichText::new(n).font(display(20.0)).color(AMBER()));
                            ui.add_space(6.0);
                            ui.label(RichText::new(t).font(bold(17.0)).color(TEXT()));
                            kit::text(ui, d, body(14.0), MUTED(), cw - 44.0);
                        });
                    });
                }
            });
            ui.add_space(22.0);

            // dashed hint
            let hint = if self.settings.speakers {
                "Держите микрофон у рта, а колонки — подальше от него. Первые секунды песни программа слушает музыку — не пойте."
            } else { "В наушниках музыка не попадает в микрофон — можно петь сразу." };
            let g = kit::galley(ui, hint, body(14.0), TEXT_2(), w - 80.0, 0);
            let (r, _) = ui.allocate_exact_size(Vec2::new(w, g.size().y + 36.0), egui::Sense::hover());
            kit::dashed_rect(ui.painter(), r, DASH());
            kit::icon(ui.painter(), Icon::Warning, Pos2::new(r.left() + 28.0, r.center().y), 18.0, AMBER());
            let gy = g.size().y;
            ui.painter().galley(Pos2::new(r.left() + 54.0, r.center().y - gy / 2.0), g, TEXT_2());
            ui.add_space(20.0);
        });
    }

    fn current_singer(&self) -> Option<String> {
        let p = &self.settings.players;
        (!p.is_empty()).then(|| p[self.settings.singer % p.len()].clone())
    }

    /// «Играют сегодня» on the home screen: who sings now (click to choose).
    fn who_sings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            if self.settings.players.is_empty() {
                if kit::button_ex(ui, Kind::Ghost, "Играют сегодня — добавить игроков", Some(Icon::User), Vec2::new(0.0, 42.0), semibold(14.0), 12).clicked() { self.show_roster = true; }
                return;
            }
            ui.label(RichText::new("Поёт:").font(semibold(15.0)).color(MUTED()));
            let n = self.settings.players.len();
            let cur = self.settings.singer % n;
            for i in 0..n {
                let on = i == cur;
                let name = self.settings.players[i].clone();
                let txt = RichText::new(name).font(bold(14.0)).color(if on { Color32::BLACK } else { TEXT_2() });
                if ui.add(egui::Button::new(txt).fill(if on { player_color(i) } else { CHIP() }).corner_radius(12.0).min_size(Vec2::new(0.0, 38.0))).clicked() { self.settings.singer = i; self.settings.save(); }
            }
            if kit::link(ui, "Изменить").clicked() { self.show_roster = true; }
        });
    }

    /// «Мини-игры»: every game in one place.
    fn games_window(&mut self, ctx: &egui::Context) {
        if !self.show_games { return; }
        let mut open = true;
        let mut go: Option<u8> = None;
        let win = egui::Window::new("Мини-игры").title_bar(false)
            .collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(20.0).inner_margin(24.0))
            .show(ctx, |ui| {
                ui.set_width(620.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Мини-игры").font(display_bold(22.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if kit::icon_button(ui, Icon::Close, 38.0).on_hover_text("Закрыть (Esc)").clicked() { open = false; }
                    });
                });
                ui.add_space(12.0);
                let games: [(&str, &str, &str, u8); 3] = [
                    ("🔄", "АПОЖ", "Песня задом наперёд, как в «Хороших шутках»: один поёт, второй повторяет перевёрнутые кусочки — угадай, что вышло!", 3),
                    ("🎭", "Пародист", "Судья включает звук — мем, песню или зверя, — каждый повторяет. Чья попытка ближе к оригиналу?", 1),
                    ("🎬", "Озвучка", "Переозвучьте сцену из фильма: каждый озвучивает ролик целиком, потом смотрим все версии.", 2),
                ];
                for (icon, name, about, code) in games {
                    egui::Frame::new().fill(SURFACE_2()).corner_radius(16.0).inner_margin(16.0).show(ui, |ui| {
                        ui.set_width(588.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon).font(body(30.0)));
                            ui.add_space(6.0);
                            ui.vertical(|ui| {
                                ui.set_width(400.0);
                                ui.label(RichText::new(name).font(display_bold(18.0)));
                                ui.label(RichText::new(about).font(body(13.0)).color(TEXT_2()));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if kit::button_ex(ui, Kind::Primary, "Играть", Some(Icon::Play), Vec2::new(0.0, 42.0), bold(14.0), 12).clicked() { go = Some(code); }
                            });
                        });
                    });
                    ui.add_space(10.0);
                }
                ui.label(RichText::new(format!("Играют: {}", roster(&self.settings).join(", "))).font(body(13.0)).color(MUTED()));
                if kit::link(ui, "Изменить список игроков").clicked() { self.show_roster = true; }
            });
        // Esc or a click outside closes it too
        let outside = win.map(|w| ctx.input(|i| i.pointer.any_click()) && !w.response.contains_pointer()).unwrap_or(false);
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || outside { open = false; }
        if !open { self.show_games = false; }
        if let Some(c) = go {
            self.show_games = false;
            match c { 1 => self.open_game(), 3 => self.open_apozh(), _ => self.open_dub() }
        }
    }

    /// The «Играют сегодня» window.
    fn roster_window(&mut self, ctx: &egui::Context) {
        if !self.show_roster { return; }
        let mut open = true;
        egui::Window::new("Играют сегодня").title_bar(false)
            .collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(20.0).inner_margin(24.0))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Играют сегодня").font(display_bold(20.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if kit::icon_button(ui, Icon::Close, 38.0).on_hover_text("Закрыть (Esc)").clicked() { open = false; }
                    });
                });
                ui.add_space(8.0);
                kit::text(ui, "Один список для караоке, «Пародиста» и «Озвучки». В караоке после каждой песни очередь переходит к следующему.", body(14.0), MUTED(), 460.0);
                ui.add_space(10.0);
                if roster_editor(ui, &mut self.settings.players, 460.0) { self.settings.save(); }
                ui.add_space(12.0);
                if kit::button_ex(ui, Kind::Primary, "Готово", None, Vec2::new(460.0, 48.0), bold(15.0), 14).clicked() { self.show_roster = false; }
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { open = false; }
        if !open { self.show_roster = false; }
        if !self.show_roster { self.settings.players.retain(|p| !p.trim().is_empty()); self.settings.save(); }
    }

    /* =================== SESSION =================== */

    fn header_session(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            if kit::ghost(ui, "Назад", Some(Icon::Back)).clicked() { self.go_home(); return; }
            ui.add_space(12.0);
            let (song, artist) = match &self.song {
                Some(s) => split_title(&s.title, &s.channel),
                None => ("Свободное пение".into(), "без видео".into()),
            };
            ui.vertical(|ui| {
                ui.add_space(14.0);
                let w = (ui.available_width() - 360.0).max(200.0);
                let g = kit::galley(ui, &song, display_bold(18.0), TEXT(), w, 1);
                let (r, _) = ui.allocate_exact_size(g.size(), egui::Sense::hover());
                ui.painter().galley(r.min, g, TEXT());
                let g = kit::galley(ui, &artist, body(13.0), MUTED(), w, 1);
                let (r, _) = ui.allocate_exact_size(g.size(), egui::Sense::hover());
                ui.painter().galley(r.min, g, MUTED());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if kit::ghost(ui, "Звук", Some(Icon::Sliders)).clicked() { self.show_settings = true; self.default_out = audio::default_output_name(); }
                if self.engine.is_some() { kit::badge(ui, "● Запись", ACCENT_SOFT(), REC_TEXT(), None, None); }
            });
        });
    }

    fn session(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, status: &Option<audio::Status>) {
        let col_w = 380.0;
        let gap = 24.0;
        let avail = ui.available_size();
        let left_w = (avail.x - col_w - gap).max(320.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            // ---- left: video ----
            ui.allocate_ui_with_layout(Vec2::new(left_w, avail.y), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let vh = (left_w * 9.0 / 16.0).min(avail.y - 110.0).max(200.0);
                let vw = (vh * 16.0 / 9.0).min(left_w);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(vw, vh), egui::Sense::hover());
                let p = ui.painter();
                kit::glow_ellipse(p, rect.center(), vw * 0.62, vh * 0.7, ACCENT().gamma_multiply(0.07));
                p.rect_filled(rect, CornerRadius::same(24), Color32::BLACK);
                let has_video = self.video.is_some() && !self.video_hidden;
                if has_video {
                    p.text(rect.center(), Align2::CENTER_CENTER, "Загрузка видео…", body(15.0), MUTED());
                    // the native view is inset so the rounded black frame shows around it
                    self.video_rect = Some(rect.shrink(6.0));
                } else {
                    self.video_rect = None;
                    let msg = if self.song.is_some() { "Видео скрыто" } else { "Свободное пение — включите музыку в любом плеере" };
                    p.text(rect.center() - Vec2::new(0.0, 22.0), Align2::CENTER_CENTER, msg, semibold(16.0), MUTED());
                    if self.song.is_some() {
                        let br = Rect::from_center_size(rect.center() + Vec2::new(0.0, 24.0), Vec2::new(190.0, 44.0));
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(br));
                        if kit::ghost(&mut child, "Показать видео", Some(Icon::Eye)).clicked() {
                            self.video_hidden = false;
                            if self.video.is_none() { if let Some(s) = &self.song { self.pending_video = Some(s.id.clone()); } }
                        }
                    }
                }
                ui.add_space(12.0);
                // progress of the clip
                let (cur, dur, ad) = self.video_shared.lock().map(|s| (s.cur, s.dur, s.ad)).unwrap_or((0.0, 0.0, false));
                ui.allocate_ui(Vec2::new(vw, 20.0), |ui| {
                    ui.horizontal(|ui| {
                        let tl = if dur > 0.0 { format!("{} / {}", fmt_time(cur), fmt_time(dur)) } else { "—:—".into() };
                        let g = kit::galley(ui, &tl, semibold(13.0), MUTED(), f32::INFINITY, 1);
                        let tw = g.size().x;
                        ui.allocate_ui(Vec2::new(vw - tw - 16.0, 20.0), |ui| {
                            ui.add_space(7.0);
                            kit::progress(ui, if dur > 0.0 && has_video { cur / dur } else { 0.0 }, 6.0);
                        });
                        let (r, _) = ui.allocate_exact_size(g.size(), egui::Sense::hover());
                        ui.painter().galley(r.min, g, MUTED());
                    });
                });
                if ad && has_video { ui.label(RichText::new("Сейчас идёт реклама — оценка начнётся с песней").font(body(12.0)).color(MUTED())); }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if self.song.is_some() {
                        if has_video {
                            if kit::ghost(ui, "Скрыть видео", Some(Icon::EyeOff)).clicked() { self.video_hidden = true; }
                            if kit::ghost(ui, "Во весь экран", Some(Icon::Expand)).clicked() { self.set_fullscreen(ctx, true); }
                        }
                        if kit::ghost(ui, "Открыть клип в браузере", Some(Icon::External)).clicked() {
                            if let Some(s) = &self.song { open(search::watch_url(&s.id)); }
                            self.close_video = true;
                            self.video_hidden = true;
                        }
                    }
                });
            });

            // ---- right column ----
            ui.allocate_ui_with_layout(Vec2::new(col_w, avail.y), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.set_width(col_w);
                self.status_column(ui, status);
            });
        });
    }

    fn status_column(&mut self, ui: &mut egui::Ui, status: &Option<audio::Status>) {
        let running = self.engine.is_some();
        // short window: fold «Рекорд»/«Режим» into one line to leave room for Boris
        let compact = ui.available_height() < 760.0;

        // status card
        kit::card(ui, SURFACE(), |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                kit::section_label(ui, "СЕЙЧАС");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let el = status.as_ref().map(|s| s.elapsed).unwrap_or(0.0);
                    ui.label(RichText::new(fmt_time(el)).font(semibold(14.0)).color(MUTED()));
                });
            });
            ui.add_space(6.0);
            let (big, line, dot) = match status {
                None => ("Готово", "Нажмите «Петь», когда будете готовы".to_string(), MUTED()),
                Some(s) => match s.phase {
                    Phase::Waiting => ("Подготовка", "Жду музыку".to_string(), MUTED()),
                    Phase::Calibrating => ("Не пойте", if self.settings.speakers { "Запоминаю, как музыка звучит в микрофоне".into() } else { "Слушаю".into() }, MUTED()),
                    Phase::Singing if s.voice_on => ("Пойте!", "Слышу голос".into(), OK()),
                    Phase::Singing => ("Пойте!", "Жду голос".into(), MUTED()),
                },
            };
            ui.label(RichText::new(big).font(display(30.0)).color(ACCENT()));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 4.5, dot);
                ui.label(RichText::new(line).font(semibold(14.0)).color(if dot == OK() { OK_TEXT() } else { MUTED() }));
            });
            if let Some(s) = status {
                if s.phase == Phase::Calibrating {
                    ui.label(RichText::new(format!("ещё {} сек", s.cal_left.ceil() as u32)).font(body(13.0)).color(MUTED()));
                }
            }
        });
        ui.add_space(12.0);
        // voice bars
        kit::card(ui, SURFACE(), |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                kit::section_label(ui, "ГОЛОС");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(s) = status {
                        let (t, c) = if s.music_heard { ("музыка слышна", MUTED()) } else { ("музыка не играет", AMBER_TEXT()) };
                        ui.label(RichText::new(t).font(body(12.0)).color(c));
                    }
                });
            });
            ui.add_space(10.0);
            let hist: Vec<(f32, bool)> = self.voice_hist.iter().copied().collect();
            kit::level_bars(ui, &hist, 28, 44.0);
        });
        ui.add_space(12.0);
        // mini cards
        if compact {
            let sp = if self.settings.speakers { "Колонки" } else { "Наушники" };
            ui.horizontal(|ui| {
                ui.label(RichText::new("Рекорд").font(semibold(14.0)).color(MUTED()));
                ui.label(RichText::new(self.settings.best.to_string()).font(display_bold(16.0)).color(AMBER()));
                ui.add_space(10.0);
                ui.label(RichText::new("Режим").font(semibold(14.0)).color(MUTED()));
                ui.label(RichText::new(sp).font(bold(14.0)));
            });
        } else { ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let w = (ui.available_width() - 12.0) / 2.0;
            ui.allocate_ui_with_layout(Vec2::new(w, 100.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(18.0).inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
                    ui.set_width(w - 38.0);
                    kit::section_label(ui, "РЕКОРД");
                    ui.label(RichText::new(self.settings.best.to_string()).font(display(28.0)).color(AMBER()));
                });
            });
            ui.allocate_ui_with_layout(Vec2::new(w, 100.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(18.0).inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
                    ui.set_width(w - 38.0);
                    kit::section_label(ui, "РЕЖИМ");
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
                        let sp = self.settings.speakers;
                        kit::icon(ui.painter(), if sp { Icon::Speaker } else { Icon::Headphones }, r.center(), 18.0, ACCENT_TEXT());
                        ui.label(RichText::new(if sp { "Колонки" } else { "Наушники" }).font(bold(17.0)));
                    });
                });
            });
        }); }
        // warnings
        if let Some((n, t)) = &self.note {
            if t.elapsed() < Duration::from_secs(10) {
                ui.add_space(12.0);
                let n = n.clone();
                kit::warning(ui, &n, ui.available_width());
            }
        }
        ui.add_space(14.0);
        let w = 380.0;
        if running {
            if kit::button_ex(ui, Kind::Primary, "Стоп и узнать результат", Some(Icon::Stop), Vec2::new(w, 68.0), bold(17.0), 16).clicked() { self.stop(); }
        } else if kit::button_ex(ui, Kind::Primary, "Петь", Some(Icon::Play), Vec2::new(w, 68.0), bold(17.0), 16).clicked() {
            self.start(self.song.is_some());
        }
        ui.add_space(10.0);
        self.boris_below(ui, w);
    }

    /// Big Boris under the Stop button; his speech bubble sits above him.
    fn boris_below(&mut self, ui: &mut egui::Ui, width: f32) {
        let bubble_h = 70.0;
        let size = (ui.available_height() - bubble_h - 4.0).clamp(90.0, width.min(300.0));
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, bubble_h + size), egui::Sense::hover());
        let br = Rect::from_min_size(Pos2::new(rect.center().x - size / 2.0, rect.top() + bubble_h), Vec2::splat(size));
        let (level, sing, music) = (self.voice_level, self.sing, self.music_level);
        self.boris.draw(ui.painter(), br, level, sing, music);
        if let Some((t, a)) = self.boris.phrase() {
            let t = t.to_string();
            let sz = avatar::bubble_size(ui, &t, width);
            // bubble bottom sits just above the top of Boris's antenna/handle
            let bottom = br.top() + size * 0.14;
            let anchor = Pos2::new(rect.center().x - sz.x / 2.0, (bottom - sz.y - 12.0).max(rect.top()));
            avatar::bubble(ui, ui.painter(), anchor, Pos2::new(rect.center().x - size * 0.05, bottom), &t, a, width);
        }
    }

    fn set_fullscreen(&mut self, ctx: &egui::Context, on: bool) {
        if self.fullscreen == on { return; }
        self.fullscreen = on;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }

    /* =================== RESULT =================== */

    fn result_screen(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(r) = &self.result else { return };
        let (score, rec, new_best, title, artist, shown) = (r.score, r.recording.clone(), r.new_best, r.title.clone(), r.artist.clone(), r.shown);
        let (best, new_achs) = (r.best, r.new_achs.clone());
        let el = shown.elapsed().as_secs_f32();
        let full = ui.max_rect();
        kit::glow_ellipse(ui.painter(), Pos2::new(full.center().x, full.top()), full.width() * 0.55, full.height() * 0.6, ACCENT().gamma_multiply(0.22));

        let k = (el / 1.4).min(1.0);
        let anim = 1.0 - (1.0 - k).powi(3);
        let shown_score = (score as f32 * anim).round() as u32;
        let mut action: Option<u8> = None;

        let content_w = (full.width() - 48.0).min(380.0 + 56.0 + 520.0);
        let content_h = 520.0;
        let text_w = (content_w - 380.0 - 56.0).max(380.0);
        let origin = Pos2::new(full.center().x - content_w / 2.0, (full.center().y - content_h / 2.0).max(full.top() + 24.0));
        let area = Rect::from_min_size(origin, Vec2::new(content_w, content_h));
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::left_to_right(egui::Align::Center)));
        ui.spacing_mut().item_spacing.x = 56.0;
        // Boris holds up the score (counts up on his sign)
        self.boris.sign = Some(if score == 0 { 0 } else { shown_score.max(1) });
        ui.allocate_ui_with_layout(Vec2::new(380.0, content_h), egui::Layout::top_down(egui::Align::Center), |ui| {
            ui.add_space(10.0);
            self.boris_column(ui, 380.0, 380.0);
        });
        // right side
        ui.allocate_ui_with_layout(Vec2::new(text_w, content_h), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.set_width(text_w);
            if new_best && score > 0 {
                let (r, _) = ui.allocate_exact_size(Vec2::new(176.0, 32.0), egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(r, CornerRadius::same(16), AMBER_SOFT());
                kit::star(p, Pos2::new(r.left() + 20.0, r.center().y), 8.0, AMBER());
                p.text(Pos2::new(r.left() + 34.0, r.center().y), Align2::LEFT_CENTER, "Новый рекорд!", semibold(13.0), AMBER_TEXT());
                ui.add_space(10.0);
            }
            let vi = verdict_index(score);
            let verdict = vi.map(|i| VERDICTS[i].1).unwrap_or("Голос не был слышен");
            kit::text(ui, verdict, display(if verdict.chars().count() > 12 { 40.0 } else { 50.0 }), TEXT(), text_w);
            let line = match (&title, artist.is_empty()) {
                (Some(t), false) => format!("{t} · {artist}"),
                (Some(t), true) => t.clone(),
                _ => String::new(),
            };
            if !line.is_empty() { kit::text(ui, &line, body(15.0), MUTED(), text_w); }
            if score == 0 {
                ui.add_space(4.0);
                kit::text(ui, "Поднесите микрофон ближе или проверьте его в Звук → Проверить микрофон", body(14.0), AMBER_WARN_TEXT(), text_w);
            }
            ui.add_space(14.0);
            // verdict scale
            for (i, (_, name)) in VERDICTS.iter().enumerate() {
                let on = vi == Some(i);
                let (r, _) = ui.allocate_exact_size(Vec2::new(300.0, 30.0), egui::Sense::hover());
                let p = ui.painter();
                if on { p.rect_filled(r, CornerRadius::same(10), ACCENT_SOFT()); }
                p.circle_filled(Pos2::new(r.left() + 16.0, r.center().y), 4.0, if on { ACCENT() } else { BAR_OFF() });
                p.text(Pos2::new(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, *name, if on { bold(14.0) } else { body(14.0) }, if on { Color32::WHITE } else { MUTED() });
            }
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                if kit::button_ex(ui, Kind::Primary, "Спеть ещё раз", Some(Icon::Repeat), Vec2::new(0.0, 56.0), bold(16.0), 14).clicked() { action = Some(1); }
                if kit::button_ex(ui, Kind::Ghost, "Другая песня", None, Vec2::new(0.0, 56.0), semibold(15.0), 14).clicked() { action = Some(2); }
                if rec.is_some() && kit::button_ex(ui, Kind::Ghost, "Запись", Some(Icon::Play), Vec2::new(0.0, 56.0), semibold(15.0), 14).clicked() { action = Some(3); }
            });
            if rec.is_some() && best.is_some() {
                ui.add_space(6.0);
                if kit::link(ui, "Послушать лучший момент (15 секунд)").clicked() { action = Some(4); }
            }
            if !new_achs.is_empty() {
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Новые достижения:").font(semibold(13.0)).color(MUTED()));
                    for a in &new_achs { kit::badge(ui, a, AMBER_SOFT(), AMBER_TEXT(), Some(AMBER()), None); }
                });
            }
        });

        // confetti for great scores
        if score >= 75 && el < 5.0 {
            let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("confetti")));
            let cols = [ACCENT(), AMBER(), ACCENT_TEXT(), AMBER_TEXT()];
            for i in 0..90u32 {
                let h = i.wrapping_mul(2654435761);
                let x0 = (h % 10000) as f32 / 10000.0 * full.width() + full.left();
                let speed = 120.0 + ((h >> 8) % 160) as f32;
                let delay = ((h >> 16) % 100) as f32 / 100.0 * 0.8;
                let tt = (el - delay).max(0.0);
                let c = Pos2::new(x0 + (tt * 2.0 + i as f32).sin() * 24.0, full.top() - 20.0 + tt * speed);
                let a = tt * 5.0 + i as f32;
                let alpha = (1.0 - (el - 3.5).max(0.0) / 1.5).clamp(0.0, 1.0);
                p.line_segment([c - Vec2::angled(a) * 5.0, c + Vec2::angled(a) * 5.0], Stroke::new(4.0_f32, cols[(i % 4) as usize].gamma_multiply(alpha)));
            }
            ctx.request_repaint();
        }
        if el < 1.6 { ctx.request_repaint(); }
        match action {
            Some(1) => self.sing_again(),
            Some(2) => { self.go_home(); }
            Some(3) => { self.open_recordings(rec); }
            Some(4) => { if let (Some(p), Some(b)) = (rec, best) { self.open_recordings(None); self.play_highlight(&p, b); } }
            _ => {}
        }
    }

    /* =================== PROFILE =================== */

    fn header_profile(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            if kit::ghost(ui, "Назад", Some(Icon::Back)).clicked() { self.show_profile = false; self.vt = None; return; }
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new("Профиль").font(display_bold(18.0)));
                ui.label(RichText::new("голос и достижения").font(body(13.0)).color(MUTED()));
            });
        });
    }

    fn start_voice_test(&mut self) {
        self.mic_test = None;
        match audio::PitchMonitor::start(&self.settings.input, self.params.clone()) {
            Ok(mon) => self.vt = Some(VoiceTest { mon, step: 1, low: None, high: None, recent: VecDeque::new(), cur: None }),
            Err(e) => self.say(e),
        }
    }

    /// Feed the voice test with the latest note; a note counts once it is held ~0.45 s.
    fn tick_voice_test(&mut self) {
        let Some(vt) = &mut self.vt else { return };
        let now = Instant::now();
        vt.cur = vt.mon.midi();
        match vt.cur {
            Some(m) => {
                vt.recent.push_back((now, m));
                while vt.recent.front().map(|(t, _)| now.duration_since(*t).as_secs_f32() > 0.5).unwrap_or(false) { vt.recent.pop_front(); }
                let span = vt.recent.front().map(|(t, _)| now.duration_since(*t).as_secs_f32()).unwrap_or(0.0);
                let (mn, mx) = vt.recent.iter().fold((f32::MAX, f32::MIN), |(a, b), (_, m)| (a.min(*m), b.max(*m)));
                if span >= 0.45 && mx - mn <= 0.7 {
                    let mut v: Vec<f32> = vt.recent.iter().map(|(_, m)| *m).collect();
                    v.sort_by(|a, b| a.total_cmp(b));
                    let med = v[v.len() / 2];
                    if vt.step == 1 { vt.low = Some(vt.low.map_or(med, |l| l.min(med))); }
                    else { vt.high = Some(vt.high.map_or(med, |h| h.max(med))); }
                }
            }
            None => vt.recent.clear(),
        }
    }

    fn finish_voice_test(&mut self) {
        let Some(vt) = self.vt.take() else { return };
        let (Some(lo), Some(hi)) = (vt.low, vt.high) else { return };
        let (lo, hi) = (lo.min(hi), hi.max(lo));
        self.settings.voice_low = lo;
        self.settings.voice_high = hi;
        if let Some(t) = self.settings.grant("voice") { self.say(format!("Новое достижение: {t}")); }
        self.settings.save();
        let vt_name = voice_type(lo, hi);
        self.boris.queue(format!("{vt_name}, значит. Ну-ну, посмотрим, как ты им распорядишься."));
    }

    /// Piano strip C2…C6 with the range highlighted and an optional live marker.
    fn keyboard(ui: &mut egui::Ui, range: Option<(f32, f32)>, marker: Option<f32>) {
        let (lo_k, hi_k) = (36i32, 84i32);
        let whites: Vec<i32> = (lo_k..=hi_k).filter(|m| ![1, 3, 6, 8, 10].contains(&m.rem_euclid(12))).collect();
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 64.0), egui::Sense::hover());
        let p = ui.painter();
        let kw = w / whites.len() as f32;
        let in_range = |m: i32| range.map(|(a, b)| (m as f32) >= a.round() && (m as f32) <= b.round()).unwrap_or(false);
        let mut xs: HashMap<i32, f32> = HashMap::new();
        for (i, &m) in whites.iter().enumerate() {
            let r = Rect::from_min_size(Pos2::new(rect.left() + i as f32 * kw, rect.top() + 14.0), Vec2::new(kw - 1.5, 50.0));
            p.rect_filled(r, CornerRadius::same(3), if in_range(m) { kit::mix(ACCENT(), Color32::WHITE, 0.55) } else { TEXT_2() });
            xs.insert(m, r.center().x);
            if m.rem_euclid(12) == 0 { p.text(Pos2::new(r.center().x, r.bottom() - 8.0), Align2::CENTER_CENTER, format!("C{}", m / 12 - 1), body(9.0), Color32::from_rgb(0x55, 0x48, 0x66)); }
        }
        for m in lo_k..=hi_k {
            if [1, 3, 6, 8, 10].contains(&m.rem_euclid(12)) {
                let x = xs.get(&(m - 1)).copied().unwrap_or(rect.left()) + kw / 2.0;
                let r = Rect::from_center_size(Pos2::new(x, rect.top() + 14.0 + 15.0), Vec2::new(kw * 0.6, 30.0));
                p.rect_filled(r, CornerRadius::same(2), if in_range(m) { ACCENT() } else { BG() });
                xs.insert(m, x);
            }
        }
        if let Some(mk) = marker {
            let m = mk.round() as i32;
            if let Some(&x) = xs.get(&m.clamp(lo_k, hi_k)) {
                p.circle_filled(Pos2::new(x, rect.top() + 6.0), 6.0, AMBER());
            }
        }
    }

    fn profile(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.tick_voice_test();
        if self.vt.is_some() { ctx.request_repaint_after(Duration::from_millis(30)); }
        let mut search: Option<String> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let w = ui.available_width().min(1180.0);
            // stats
            let n_ach = ACHS.iter().filter(|a| self.settings.has(a.id)).count();
            let stats = [
                ("ПЕСЕН СПЕТО", self.settings.songs_total.max(self.settings.history.len() as u32).to_string(), TEXT()),
                ("РЕКОРД", self.settings.best.to_string(), AMBER()),
                ("САМАЯ ДЛИННАЯ НОТА", format!("{:.1} с", self.settings.longest_best).replace('.', ","), TEXT()),
                ("ДОСТИЖЕНИЯ", format!("{n_ach} из {}", ACHS.len()), ACCENT_TEXT()),
            ];
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                let cw = (w - 48.0) / 4.0;
                for (label, val, col) in stats {
                    ui.allocate_ui_with_layout(Vec2::new(cw, 96.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(18.0).inner_margin(egui::Margin::symmetric(20, 16)).show(ui, |ui| {
                            ui.set_width(cw - 42.0);
                            kit::section_label(ui, label);
                            ui.label(RichText::new(val).font(display(26.0)).color(col));
                        });
                    });
                }
            });
            ui.add_space(20.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 24.0;
                let colw = (w - 24.0) / 2.0;
                // ---- voice ----
                ui.allocate_ui_with_layout(Vec2::new(colw, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(24.0).inner_margin(28.0).show(ui, |ui| {
                        ui.set_width(colw - 58.0);
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let inner = colw - 58.0;
                        if let Some(vt) = &self.vt {
                            let (step, cur, low, high, lvl) = (vt.step, vt.cur, vt.low, vt.high, vt.mon.level());
                            kit::section_label(ui, &format!("ШАГ {step} ИЗ 2"));
                            ui.add_space(4.0);
                            let (title, desc) = if step == 1 {
                                ("Спойте самую низкую ноту", "Тяните «а-а-а» и опускайтесь ниже, пока удобно. Держите ноту секунду — программа её запомнит.")
                            } else {
                                ("Теперь — самую высокую", "Тяните «а-а-а» и поднимайтесь выше, пока голос не начнёт напрягаться. Держите ноту секунду.")
                            };
                            ui.label(RichText::new(title).font(display(28.0)));
                            kit::text(ui, desc, body(15.0), TEXT_2(), inner);
                            ui.add_space(14.0);
                            match cur {
                                Some(m) => { ui.label(RichText::new(note_name(m)).font(display(30.0)).color(ACCENT())); }
                                None => { ui.label(RichText::new("Слушаю…").font(display(30.0)).color(MUTED())); }
                            }
                            let found = if step == 1 { low } else { high };
                            match found {
                                Some(f) => { ui.label(RichText::new(format!("{}: {}", if step == 1 { "Самая низкая" } else { "Самая высокая" }, note_name(f))).font(semibold(15.0)).color(OK_TEXT())); }
                                None => { ui.label(RichText::new("Держите ноту, пока она не запишется").font(body(14.0)).color(MUTED())); }
                            }
                            ui.add_space(10.0);
                            let rng = match (low, high) { (Some(a), Some(b)) => Some((a, b)), (Some(a), None) => Some((a, a)), _ => None };
                            Self::keyboard(ui, rng, cur);
                            ui.add_space(8.0);
                            kit::segment_meter(ui, (lvl * 5.0).min(1.0), 32, 10.0);
                            ui.add_space(16.0);
                            ui.horizontal(|ui| {
                                if found.is_some() {
                                    let label = if step == 1 { "Дальше" } else { "Готово" };
                                    if kit::primary(ui, label, None).clicked() {
                                        if step == 1 { if let Some(vt) = &mut self.vt { vt.step = 2; vt.recent.clear(); } } else { self.finish_voice_test(); }
                                    }
                                }
                                if kit::ghost(ui, "Отмена", None).clicked() { self.vt = None; }
                            });
                        } else if self.settings.voice_high > 0.0 {
                            let (lo, hi) = (self.settings.voice_low, self.settings.voice_high);
                            let vname = voice_type(lo, hi);
                            kit::section_label(ui, "МОЙ ГОЛОС");
                            ui.label(RichText::new(vname).font(display(40.0)).color(ACCENT()));
                            kit::text(ui, &format!("от {} до {}", note_name(lo), note_name(hi)), body(16.0), TEXT_2(), inner);
                            let oct = (hi - lo) / 12.0;
                            ui.label(RichText::new(format!("диапазон {:.1} {}", oct, if oct < 2.0 { "октавы" } else { "октав" }).replace('.', ",")).font(body(14.0)).color(MUTED()));
                            ui.add_space(12.0);
                            Self::keyboard(ui, Some((lo, hi)), None);
                            ui.add_space(18.0);
                            kit::section_label(ui, "ПОПРОБУЙТЕ СПЕТЬ");
                            ui.add_space(4.0);
                            for s in songs_for(vname) {
                                let (song, artist) = split_title(s, "");
                                let (rect, resp) = ui.allocate_exact_size(Vec2::new(inner, 48.0), egui::Sense::click());
                                let p = ui.painter();
                                if resp.hovered() { p.rect_filled(rect, CornerRadius::same(12), SURFACE_2()); ctx.set_cursor_icon(egui::CursorIcon::PointingHand); }
                                let chip = Rect::from_center_size(Pos2::new(rect.left() + 22.0, rect.center().y), Vec2::splat(32.0));
                                p.rect_filled(chip, CornerRadius::same(9), CHIP());
                                kit::icon(p, Icon::Play, chip.center() + Vec2::new(1.0, 0.0), 13.0, ACCENT_TEXT());
                                p.text(Pos2::new(rect.left() + 48.0, rect.center().y - 8.0), Align2::LEFT_CENTER, &song, semibold(14.0), TEXT());
                                p.text(Pos2::new(rect.left() + 48.0, rect.center().y + 9.0), Align2::LEFT_CENTER, &artist, body(12.0), MUTED());
                                if resp.clicked() { search = Some(s.to_string()); }
                            }
                            ui.add_space(12.0);
                            if kit::ghost(ui, "Пройти тест заново", Some(Icon::Mic)).clicked() { self.start_voice_test(); }
                        } else {
                            kit::section_label(ui, "МОЙ ГОЛОС");
                            ui.label(RichText::new("Узнай свой голос").font(display(32.0)));
                            ui.add_space(4.0);
                            kit::text(ui, "Спойте самую низкую и самую высокую ноту — программа определит тип голоса, покажет диапазон и подскажет песни, которые вам подойдут. Займёт полминуты.", body(15.0), TEXT_2(), inner);
                            ui.add_space(14.0);
                            Self::keyboard(ui, None, None);
                            ui.add_space(16.0);
                            if kit::primary(ui, "Пройти тест", Some(Icon::Mic)).clicked() { self.start_voice_test(); }
                        }
                    });
                });
                // ---- achievements ----
                ui.allocate_ui_with_layout(Vec2::new(colw, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(24.0).inner_margin(28.0).show(ui, |ui| {
                        ui.set_width(colw - 58.0);
                        let inner = colw - 58.0;
                        kit::section_label(ui, "ДОСТИЖЕНИЯ");
                        ui.add_space(8.0);
                        for a in ACHS.iter() {
                            let got = self.settings.achs.iter().find(|(id, _)| id == a.id).map(|(_, w)| w.clone());
                            let (rect, _) = ui.allocate_exact_size(Vec2::new(inner, 56.0), egui::Sense::hover());
                            let p = ui.painter();
                            let ic = Rect::from_center_size(Pos2::new(rect.left() + 22.0, rect.center().y), Vec2::splat(40.0));
                            p.rect_filled(ic, CornerRadius::same(12), if got.is_some() { AMBER_SOFT() } else { CHIP() });
                            kit::star(p, ic.center(), 10.0, if got.is_some() { AMBER() } else { BAR_OFF() });
                            let tc = if got.is_some() { TEXT() } else { MUTED() };
                            p.text(Pos2::new(rect.left() + 56.0, rect.center().y - 9.0), Align2::LEFT_CENTER, a.title, semibold(15.0), tc);
                            p.text(Pos2::new(rect.left() + 56.0, rect.center().y + 10.0), Align2::LEFT_CENTER, a.desc, body(13.0), MUTED());
                            if let Some(wh) = got {
                                p.text(Pos2::new(rect.right(), rect.center().y), Align2::RIGHT_CENTER, pretty_when(&wh), body(12.0), AMBER_TEXT());
                            }
                        }
                    });
                });
            });
            ui.add_space(20.0);
        });
        if let Some(q) = search { self.show_profile = false; self.do_search(ctx, q); }
    }

    /* =================== RECORDINGS =================== */

    fn open_recordings(&mut self, play: Option<PathBuf>) {
        self.recs = load_recordings();
        self.show_recs = true;
        self.confirm_delete = None;
        if let Some(p) = play { self.play_recording(&p); }
    }

    fn play_highlight(&mut self, path: &std::path::Path, from: f32) {
        let same = self.player.as_ref().map(|(p, _)| p == path).unwrap_or(false);
        if !same {
            self.player = None;
            match audio::Player::open(path, &None) {
                Ok(p) => self.player = Some((path.to_path_buf(), p)),
                Err(e) => { self.say(e); return; }
            }
        }
        if let Some((_, p)) = &self.player { p.play_range(from, 15.0); }
    }

    fn save_highlight_clip(&mut self, path: &std::path::Path, from: f32) {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("запись").to_string();
        let name = match stem.rsplit_once(" — ") { Some((a, b)) => format!("{a} (лучший момент) — {b}.wav"), None => format!("{stem} (лучший момент).wav") };
        let dst = path.with_file_name(name);
        match audio::save_clip(path, from, 15.0, &dst) {
            Ok(_) => { self.say("Лучший момент сохранён отдельным файлом"); self.recs = load_recordings(); show_in_folder(&dst); }
            Err(e) => self.say(format!("Не удалось сохранить: {e}")),
        }
    }

    fn play_recording(&mut self, path: &std::path::Path) {
        if let Some((cur, pl)) = &self.player {
            if cur == path { pl.toggle(); return; }
        }
        self.player = None;
        match audio::Player::open(path, &None) {
            Ok(p) => self.player = Some((path.to_path_buf(), p)),
            Err(e) => self.say(e),
        }
    }

    fn header_recordings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            if kit::ghost(ui, "Назад", Some(Icon::Back)).clicked() { self.show_recs = false; self.player = None; return; }
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new("Записи").font(display_bold(18.0)));
                let n = self.recs.len();
                ui.label(RichText::new(match n { 0 => "пока пусто".to_string(), _ => format!("выступлений: {n}") }).font(body(13.0)).color(MUTED()));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if kit::ghost(ui, "Открыть папку", Some(Icon::Folder)).clicked() { let d = record_dir(); let _ = std::fs::create_dir_all(&d); open(d); }
            });
        });
    }

    fn recordings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.recs.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Пока нет записей").font(display(28.0)));
                ui.add_space(6.0);
                ui.label(RichText::new("Каждое выступление сохраняется само — спойте что-нибудь!").font(body(15.0)).color(MUTED()));
            });
            return;
        }
        let mut action: Option<(u8, PathBuf)> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let w = ui.available_width().min(960.0);
            let pad = ((ui.available_width() - w) / 2.0).max(0.0);
            for it in &self.recs {
                let playing = self.player.as_ref().filter(|(p, _)| *p == it.path).map(|(_, pl)| pl);
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.allocate_ui_with_layout(Vec2::new(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        let fill = if playing.is_some() { SURFACE_2() } else { SURFACE() };
                        egui::Frame::new().fill(fill).stroke(Stroke::new(1.0_f32, if playing.is_some() { BORDER_STRONG() } else { BORDER() }))
                            .corner_radius(18.0).inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
                            ui.set_width(w - 36.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 14.0;
                                // play / pause
                                let (r, resp) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::click());
                                let p = ui.painter();
                                let col = if resp.hovered() { kit::mix(ACCENT(), Color32::WHITE, 0.1) } else { ACCENT() };
                                p.circle_filled(r.center(), 24.0, col);
                                let is_on = playing.map(|pl| !pl.paused() && !pl.finished()).unwrap_or(false);
                                kit::icon(p, if is_on { Icon::Pause } else { Icon::Play }, r.center() + if is_on { Vec2::ZERO } else { Vec2::new(2.0, 0.0) }, 18.0, ON_ACCENT());
                                if resp.hovered() { ctx.set_cursor_icon(egui::CursorIcon::PointingHand); }
                                if resp.clicked() { action = Some((1, it.path.clone())); }
                                // text
                                let tw = w - 36.0 - 48.0 - 14.0 - 250.0;
                                ui.vertical(|ui| {
                                    ui.add_space(3.0);
                                    let g = kit::galley(ui, &it.title, semibold(16.0), TEXT(), tw, 1);
                                    let (r, _) = ui.allocate_exact_size(g.size(), egui::Sense::hover());
                                    ui.painter().galley(r.min, g, TEXT());
                                    let meta = format!("{} · {}", it.when, fmt_time(it.secs));
                                    ui.label(RichText::new(meta).font(body(13.0)).color(MUTED()));
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    let confirm = self.confirm_delete.as_ref() == Some(&it.path);
                                    if confirm {
                                        if kit::button_ex(ui, Kind::Primary, "Удалить?", Some(Icon::Trash), Vec2::new(0.0, 44.0), bold(14.0), 12).clicked() { action = Some((4, it.path.clone())); }
                                    } else if kit::icon_button(ui, Icon::Trash, 44.0).on_hover_text("Удалить").clicked() { action = Some((5, it.path.clone())); }
                                    if kit::icon_button(ui, Icon::Folder, 44.0).on_hover_text("Показать в папке").clicked() { action = Some((3, it.path.clone())); }
                                    if kit::icon_button(ui, Icon::External, 44.0).on_hover_text("Открыть в проигрывателе").clicked() { action = Some((2, it.path.clone())); }
                                    if it.best.is_some() && kit::icon_button(ui, Icon::Star, 44.0).on_hover_text("Лучший момент — 15 секунд").clicked() { action = Some((6, it.path.clone())); }
                                    if let Some(sc) = it.score {
                                        let (bg, fg) = if sc >= 80 { (AMBER_SOFT(), AMBER()) } else { (CHIP(), TEXT_2()) };
                                        let (r, _) = ui.allocate_exact_size(Vec2::new(48.0, 36.0), egui::Sense::hover());
                                        ui.painter().rect_filled(r, CornerRadius::same(10), bg);
                                        ui.painter().text(r.center(), Align2::CENTER_CENTER, sc.to_string(), display_bold(15.0), fg);
                                    }
                                });
                            });
                            if let Some(pl) = playing {
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    let (pos, dur) = (pl.position(), pl.duration());
                                    let tl = format!("{} / {}", fmt_time(pos), fmt_time(dur));
                                    let g = kit::galley(ui, &tl, FontId::monospace(13.0), MUTED(), f32::INFINITY, 1);
                                    let bw = ui.available_width() - g.size().x - 16.0;
                                    let (bar, resp) = ui.allocate_exact_size(Vec2::new(bw, 20.0), egui::Sense::click_and_drag());
                                    let p = ui.painter();
                                    let track = Rect::from_center_size(bar.center(), Vec2::new(bar.width(), 6.0));
                                    p.rect_filled(track, CornerRadius::same(3), SELECTED());
                                    let fr = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) } else { 0.0 };
                                    let mut f = track; f.set_width(track.width() * fr);
                                    p.rect_filled(f, CornerRadius::same(3), ACCENT());
                                    p.circle_filled(Pos2::new(track.left() + track.width() * fr, track.center().y), if resp.hovered() { 8.0 } else { 6.0 }, TEXT());
                                    if let Some(pp) = resp.interact_pointer_pos() {
                                        if resp.clicked() || resp.dragged() { pl.seek_frac((pp.x - track.left()) / track.width()); }
                                    }
                                    if resp.hovered() { ctx.set_cursor_icon(egui::CursorIcon::PointingHand); }
                                    let (r, _) = ui.allocate_exact_size(g.size(), egui::Sense::hover());
                                    ui.painter().galley(r.min, g, MUTED());
                                });
                                if it.best.is_some() {
                                    ui.horizontal(|ui| {
                                        if kit::link(ui, "▶ Лучший момент").clicked() { action = Some((6, it.path.clone())); }
                                        ui.add_space(12.0);
                                        if kit::link(ui, "Сохранить лучший момент отдельным файлом").clicked() { action = Some((7, it.path.clone())); }
                                    });
                                }
                            }
                        });
                    });
                });
                ui.add_space(10.0);
            }
        });
        if let Some((a, path)) = action {
            match a {
                1 => self.play_recording(&path),
                2 => { if self.player.as_ref().map(|(p, _)| *p == path).unwrap_or(false) { self.player = None; } open(&path); }
                3 => show_in_folder(&path),
                5 => self.confirm_delete = Some(path),
                6 => { if let Some(b) = self.recs.iter().find(|r| r.path == path).and_then(|r| r.best) { self.play_highlight(&path, b); } }
                7 => { if let Some(b) = self.recs.iter().find(|r| r.path == path).and_then(|r| r.best) { self.save_highlight_clip(&path, b); } }
                4 => {
                    if self.player.as_ref().map(|(p, _)| *p == path).unwrap_or(false) { self.player = None; }
                    if let Err(e) = std::fs::remove_file(&path) { self.say(format!("Не удалось удалить: {e}")); }
                    self.confirm_delete = None;
                    self.recs = load_recordings();
                }
                _ => {}
            }
        }
    }

    /* =================== SETTINGS MODAL =================== */

    fn settings_modal(&mut self, ctx: &egui::Context) {
        if !self.show_settings { return; }
        let screen = ctx.screen_rect();
        let mut close = false;
        egui::Area::new(egui::Id::new("modal_dim")).fixed_pos(Pos2::ZERO).order(egui::Order::Middle).show(ctx, |ui| {
            ui.painter().rect_filled(screen, CornerRadius::ZERO, BG_DEEP().gamma_multiply(0.88));
            if ui.allocate_rect(screen, egui::Sense::click()).clicked() { close = true; }
        });
        let running = self.engine.is_some();
        egui::Window::new("sound").title_bar(false).resizable(false).collapsible(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO).min_width(800.0).max_width(800.0).order(egui::Order::Foreground)
            .frame(egui::Frame::new().fill(SIDEBAR()).corner_radius(28.0).inner_margin(egui::Margin::symmetric(40, 36))
                .stroke(Stroke::new(1.0_f32, BORDER()))
                .shadow(egui::epaint::Shadow { offset: [0, 20], blur: 60, spread: 0, color: Color32::from_black_alpha(170) }))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Звук").font(display(28.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if kit::icon_button(ui, Icon::Close, 44.0).clicked() { close = true; }
                    });
                });
                ui.add_space(12.0);
                // colour schemes
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.label(RichText::new("Оформление").font(semibold(13.0)).color(TEXT_2()));
                    ui.add_space(6.0);
                    for i in 0..theme::SCHEMES.len() {
                        let (bg, acc, amb) = theme::scheme_swatch(i);
                        let on = theme::scheme_index() == i;
                        let (r, resp) = ui.allocate_exact_size(Vec2::new(118.0, 40.0), egui::Sense::click());
                        let p = ui.painter();
                        p.rect_filled(r, CornerRadius::same(12), bg);
                        p.rect_stroke(r, CornerRadius::same(12), Stroke::new(if on { 2.0_f32 } else { 1.0 }, if on { acc } else { BORDER_STRONG() }), egui::StrokeKind::Inside);
                        p.circle_filled(Pos2::new(r.left() + 16.0, r.center().y), 7.0, acc);
                        p.circle_filled(Pos2::new(r.left() + 30.0, r.center().y), 7.0, amb);
                        p.text(Pos2::new(r.left() + 44.0, r.center().y), Align2::LEFT_CENTER, theme::SCHEMES[i], semibold(13.0), Color32::from_rgb(0xF4, 0xEE, 0xF8));
                        if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                        if resp.clicked() && !on {
                            theme::set_scheme(i);
                            theme::apply(ui.ctx());
                            self.settings.scheme = i;
                            self.settings.save();
                        }
                    }
                });
                ui.add_space(16.0);
                let max_h = (ctx.screen_rect().height() - 220.0).max(300.0);
                egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                let mut changed = false;
                let mut live = false;
                let mut restart_test = false;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 40.0;
                    let colw = (800.0 - 40.0) / 2.0;
                    // left column
                    ui.allocate_ui_with_layout(Vec2::new(colw, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.spacing_mut().interact_size.y = 26.0;
                        ui.set_width(colw);
                        ui.label(RichText::new("Микрофон").font(semibold(13.0)).color(TEXT_2()));
                        ui.add_enabled_ui(!running, |ui| {
                            let cur = self.settings.input.clone().unwrap_or_else(|| "По умолчанию".into());
                            egui::ComboBox::from_id_salt("in").width(colw).height(320.0).selected_text(RichText::new(cur).font(body(15.0))).show_ui(ui, |ui| {
                                let mut c = ui.selectable_value(&mut self.settings.input, None, "По умолчанию").changed();
                                for d in &self.devices.0 { c |= ui.selectable_value(&mut self.settings.input, Some(d.clone()), d).changed(); }
                                if c { changed = true; restart_test = true; }
                            });
                        });
                        if kit::link(ui, "Обновить список").clicked() { self.devices = audio::list_devices(); self.default_out = audio::default_output_name(); }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Громкость микрофона").font(semibold(15.0)));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if (self.settings.mic_gain - 1.0).abs() > 0.01 && kit::link(ui, "Сбросить").on_hover_text("Вернуть 100%").clicked() {
                                    self.settings.mic_gain = 1.0; self.params.mic_gain.set(1.0); changed = true;
                                }
                                ui.label(RichText::new(format!("{:>3}%", (self.settings.mic_gain * 100.0).round() as i32)).font(FontId::monospace(14.0)).color(MUTED()));
                            });
                        });
                        ui.spacing_mut().slider_width = colw;
                        let gr = ui.add(egui::Slider::new(&mut self.settings.mic_gain, 0.25..=4.0).logarithmic(true).show_value(false));
                        if gr.changed() { self.params.mic_gain.set(self.settings.mic_gain); }
                        if gr.drag_stopped() || (gr.changed() && !gr.dragged()) { changed = true; }
                        ui.add_space(8.0);
                        if running { ui.label(RichText::new("Микрофон можно сменить после выступления").font(body(12.0)).color(MUTED())); }

                        ui.add_space(14.0);
                        egui::Frame::new().fill(SURFACE_2()).corner_radius(16.0).inner_margin(16.0).show(ui, |ui| {
                            ui.set_width(colw - 32.0);
                            let lvl = self.mic_test.as_ref().map(|m| m.level()).unwrap_or(0.0);
                            let heard = self.mic_heard.map(|t| t.elapsed().as_secs_f32() < 3.0).unwrap_or(false);
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
                                let (dot, txt, col) = if running { (MUTED(), "Идёт выступление", MUTED()) }
                                    else if self.mic_test.is_none() { (MUTED(), "Проверка выключена", MUTED()) }
                                    else if heard { (OK(), "Микрофон работает", OK_TEXT()) } else { (AMBER(), "Тишина — скажите что-нибудь", AMBER_TEXT()) };
                                ui.painter().circle_filled(r.center(), 4.5, dot);
                                ui.label(RichText::new(txt).font(semibold(14.0)).color(col));
                            });
                            ui.add_space(6.0);
                            if !running {
                                let testing = self.mic_test.is_some() && self.mic_test_user;
                                let resp = if testing { kit::ghost(ui, "Остановить проверку", Some(Icon::Stop)) } else { kit::primary(ui, "Проверить микрофон", Some(Icon::Mic)) };
                                if resp.clicked() {
                                    if testing { self.mic_test = None; self.mic_test_user = false; }
                                    else { self.mic_test = None; self.start_mic_test(true); }
                                }
                            }
                            ui.add_space(8.0);
                            kit::segment_meter(ui, (lvl * 5.0).min(1.0), 32, 16.0);
                            let pk = self.mic_test.as_ref().map(|m| m.peak()).unwrap_or(0.0);
                            if pk >= 0.98 {
                                ui.add_space(4.0);
                                ui.label(RichText::new("Перегруз — убавьте громкость микрофона").font(semibold(13.0)).color(AMBER_TEXT()));
                            } else if self.mic_test.is_some() && !heard && self.settings.mic_gain < 1.0 {
                                ui.add_space(4.0);
                                ui.label(RichText::new("Тихо — попробуйте прибавить громкость микрофона").font(semibold(13.0)).color(MUTED()));
                            }
                            ui.add_space(8.0);
                            kit::text(ui, "Скажите что-нибудь — деления должны загораться. Если шкала не двигается, выберите другой микрофон.", body(13.0), MUTED(), colw - 32.0);
                        });
                        if kit::link(ui, "Открыть Параметры → Конфиденциальность → Микрофон").clicked() { open("ms-settings:privacy-microphone"); }
                    });
                    // right column
                    ui.allocate_ui_with_layout(Vec2::new(colw, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.spacing_mut().interact_size.y = 26.0;
                        ui.set_width(colw);
                        let mon_label = if self.settings.speakers { "Голос в колонки" } else { "Голос в наушники" };
                        let rows: [(&str, &mut f32, f32); 3] = [
                            (mon_label, &mut self.settings.monitor, 1.5),
                            ("Эффект зала", &mut self.settings.reverb, 1.0),
                            ("Эхо", &mut self.settings.echo, 1.0),
                        ];
                        for (label, val, max) in rows {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(label).font(semibold(15.0)));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(RichText::new(format!("{:>3}%", (*val / max * 100.0).round() as i32)).font(FontId::monospace(14.0)).color(MUTED()));
                                });
                            });
                            ui.spacing_mut().slider_width = colw;
                            let sr = ui.add(egui::Slider::new(val, 0.0..=max).show_value(false));
                            if sr.changed() { live = true; }
                            if sr.drag_stopped() || (sr.changed() && !sr.dragged()) { changed = true; }
                            ui.add_space(4.0);
                        }
                        // auto-tune: what you hear is pulled to the nearest note (the score is not)
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Автотюн").font(semibold(15.0)));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for (k, lab) in [(2u8, "Робот"), (1, "Мягко"), (0, "Выкл")] {
                                    let on = self.settings.autotune == k;
                                    let t = RichText::new(lab).font(semibold(13.0)).color(if on { ON_ACCENT() } else { TEXT_2() });
                                    if ui.add(egui::Button::new(t).fill(if on { ACCENT() } else { CHIP() }).corner_radius(8.0).min_size(Vec2::new(64.0, 30.0))).clicked() {
                                        self.settings.autotune = k;
                                        self.params.autotune.store(k, std::sync::atomic::Ordering::Relaxed);
                                        self.settings.save();
                                    }
                                }
                            });
                        });
                        ui.label(RichText::new("Подтягивает голос в колонках к ближайшей ноте. «Мягко» — незаметно, «Робот» — как в поп-хитах. На оценку не влияет.").font(body(12.0)).color(MUTED()));
                        ui.add_space(10.0);
                        let mut toggled = false;
                        toggled |= kit::switch_row(ui, "Убирать музыку из микрофона",
                            if self.settings.speakers { "Вычитает звук колонок — судья лучше слышит голос" } else { "Нужно только в режиме «Колонки»" },
                            &mut self.settings.aec, self.settings.speakers);
                        ui.add_space(6.0);
                        toggled |= kit::switch_row(ui, "Шумоподавление", "Убирает фоновый шум: вентилятор, улицу, гул", &mut self.settings.denoise, true);
                        ui.add_space(6.0);
                        toggled |= kit::switch_row(ui, "Звуки зала", "Аплодисменты и реакция зала после песни", &mut self.settings.crowd_sfx, true);
                        if toggled {
                            self.params.aec.store(self.settings.aec, std::sync::atomic::Ordering::Relaxed);
                            self.params.denoise.store(self.settings.denoise, std::sync::atomic::Ordering::Relaxed);
                            self.settings.save();
                        }
                        ui.add_space(10.0);
                        kit::warning(ui, "Если появляется свист — уменьшите «Голос в колонки» или отодвиньте микрофон от колонок.", colw);
                        ui.add_space(8.0);
                        let dev = self.default_out.clone().unwrap_or_else(|| "не найдено".into());
                        kit::text(ui, &format!("Звук идёт через устройство Windows по умолчанию: «{dev}». Чтобы сменить колонки или наушники, выберите другое устройство в Windows."), body(13.0), MUTED(), colw);
                        if kit::link(ui, "Открыть настройки звука Windows").clicked() { open("ms-settings:sound"); }
                    });
                });
                ui.add_space(8.0);
                if changed || live {
                    self.params.monitor.set(self.settings.monitor);
                    self.params.reverb.set(self.settings.reverb);
                    self.params.echo.set(self.settings.echo);
                }
                if changed { self.settings.save(); }
                if restart_test && self.mic_test.is_some() { let user = self.mic_test_user; self.mic_test = None; self.start_mic_test(user); }
                });
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if kit::button_ex(ui, Kind::Primary, "Готово", None, Vec2::new(140.0, 48.0), bold(15.0), 12).clicked() { close = true; }
                });
            });
        if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) { self.show_settings = false; }
    }

    fn start_mic_test(&mut self, user: bool) {
        match audio::MicTest::start(&self.settings.input, self.params.clone()) {
            Ok(m) => { self.mic_test = Some(m); self.mic_test_user = user; self.mic_err = None; }
            Err(e) => { self.mic_err = Some(e.clone()); if user { self.say(e); } }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // search messages
        while let Ok(m) = self.rx.try_recv() {
            match m {
                SearchMsg::Results(g, r) if g == self.search_gen => {
                    self.searching = false;
                    match r { Ok(v) => self.results = v, Err(e) => self.search_err = Some(e) }
                }
                SearchMsg::Thumb(id, img) => { let t = ctx.load_texture(format!("th_{id}"), img, egui::TextureOptions::LINEAR); self.thumbs.insert(id, t); }
                _ => {}
            }
        }

        // open the chosen clip inside the app (or in the browser if WebView2 fails)
        if let Some(id) = self.pending_video.take() {
            let url = if id.starts_with("file:") { id.clone() } else { search::watch_url(&id) };
            match &self.video {
                Some(v) => v.load(&url),
                None => match make_video(frame, &url, self.video_shared.clone(), ctx.clone()) {
                    Ok(v) => self.video = Some(v),
                    Err(e) => { open(&url); self.video_hidden = true; self.say(format!("Не удалось показать видео в программе ({e}) — открыл в браузере.")); }
                },
            }
        }
        if self.close_video { self.close_video = false; self.video = None; self.video_rect = None; }

        self.games_window(ctx);
        self.roster_window(ctx);

        // fullscreen requests from the player
        let (fs_t, fs_x) = self.video_shared.lock().map(|mut s| { let r = (s.fs_toggle, s.fs_exit); s.fs_toggle = false; s.fs_exit = false; r }).unwrap_or((false, false));
        if fs_t && self.video.is_some() { let on = !self.fullscreen; self.set_fullscreen(ctx, on); }
        if fs_x || (self.fullscreen && ctx.input(|i| i.key_pressed(egui::Key::Escape))) { self.set_fullscreen(ctx, false); }
        if self.fullscreen && (self.video.is_none() || self.video_hidden || self.result.is_some()) { self.set_fullscreen(ctx, false); }

        // engine status
        let status = self.engine.as_ref().and_then(|e| e.status.lock().ok().map(|mut s| { let st = s.clone(); s.toast = None; st }));
        if let Some(t) = status.as_ref().and_then(|s| s.toast.clone()) { self.note = Some((t, Instant::now())); }
        if status.as_ref().map(|s| s.finished).unwrap_or(false) { self.stop(); }
        if self.last_bar.elapsed() >= Duration::from_millis(70) {
            self.last_bar = Instant::now();
            let v = status.as_ref().map(|s| (s.mic_level.max(s.voice_level * 0.8), s.voice_on)).unwrap_or((0.0, false));
            self.voice_hist.push_back(v);
            while self.voice_hist.len() > 28 { self.voice_hist.pop_front(); }
        }

        // Boris
        let dt = self.last_frame.elapsed().as_secs_f32().min(0.1);
        self.last_frame = Instant::now();
        let target_lvl = status.as_ref().map(|s| if s.voice_on { s.voice_level } else { 0.0 }).unwrap_or(0.0);
        self.voice_level += (target_lvl - self.voice_level) * (dt * 12.0).min(1.0);
        let sit = self.situation(&status);
        self.boris.update(sit, dt);
        if self.result.is_none() { self.boris.sign = None; }
        // singing along: height of the note relative to this singer's usual range
        self.sing = None;
        let ml = status.as_ref().map(|s| s.music_level).unwrap_or(0.0);
        self.music_level += (ml - self.music_level) * (dt * 6.0).min(1.0);
        if let Some(s) = &status {
            if s.phase == Phase::Singing && s.voice_on && s.pitch > 0.0 {
                if self.pitch_center == 0.0 {
                    self.pitch_center = if self.settings.voice_high > 0.0 { (self.settings.voice_low + self.settings.voice_high) / 2.0 } else { s.pitch };
                }
                self.pitch_center += (s.pitch - self.pitch_center) * (dt / 6.0).min(1.0); // ~6 s memory
                let h = ((s.pitch - self.pitch_center) / 5.0).clamp(-1.0, 1.0);          // ±5 semitones = full range
                self.sing = Some(avatar::Sing { open: (self.voice_level * 1.4).min(1.0), height: h });
            }
        } else { self.pitch_center = 0.0; }
        ctx.request_repaint_after(Duration::from_millis(if self.video.is_some() { 40 } else { 16 }));

        // background mic monitor on the home screen
        let screen = self.screen();
        if self.engine.is_none() && self.mic_test.is_none() && self.mic_err.is_none() && screen == Screen::Home { self.start_mic_test(false); }
        if screen != Screen::Home && !self.show_settings && self.mic_test.is_some() && !self.mic_test_user { self.mic_test = None; }
        if let Some(m) = &self.mic_test { if m.level() > 0.01 { self.mic_heard = Some(Instant::now()); } }
        if let Some(s) = &status { if s.mic_level > 0.04 { self.mic_heard = Some(Instant::now()); } }

        if self.player.as_ref().map(|(_, p)| !p.paused() && !p.finished()).unwrap_or(false) { ctx.request_repaint_after(Duration::from_millis(100)); }
        if self.engine.is_some() && self.video.is_none() { ctx.request_repaint(); }
        else if self.engine.is_some() || self.mic_test.is_some() { ctx.request_repaint_after(Duration::from_millis(50)); }
        else if self.toast.is_some() || self.searching { ctx.request_repaint_after(Duration::from_millis(100)); }

        if self.fullscreen {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ctx, |ui| { self.video_rect = Some(ui.max_rect()); });
        } else {
            match screen {
                Screen::Home => {
                    self.video_rect = None;
                    egui::SidePanel::left("songs").exact_width(360.0).resizable(false).show_separator_line(true)
                        .frame(egui::Frame::new().fill(SIDEBAR()))
                        .show(ctx, |ui| {
                            let r = ui.max_rect().shrink2(Vec2::new(20.0, 0.0));
                            let r = Rect::from_min_max(r.min + Vec2::new(0.0, 20.0), r.max - Vec2::new(0.0, 12.0));
                            let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
                            inner.set_clip_rect(r.expand(2.0));
                            self.sidebar(&mut inner, ctx);
                        });
                    egui::TopBottomPanel::top("hdr").exact_height(76.0).show_separator_line(true)
                        .frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin::symmetric(32, 0)))
                        .show(ctx, |ui| self.header_home(ui));
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin { left: 64, right: 64, top: 40, bottom: 24 }))
                        .show(ctx, |ui| self.home(ui, ctx));
                }
                Screen::Session => {
                    egui::TopBottomPanel::top("hdr_s").exact_height(76.0).show_separator_line(true)
                        .frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin::symmetric(32, 0)))
                        .show(ctx, |ui| self.header_session(ui));
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin { left: 32, right: 32, top: 24, bottom: 20 }))
                        .show(ctx, |ui| self.session(ui, ctx, &status));
                }
                Screen::Dub => {
                    egui::TopBottomPanel::top("hdr_d").exact_height(76.0).show_separator_line(true)
                        .frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin::symmetric(32, 0)))
                        .show(ctx, |ui| self.header_dub(ui));
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin { left: 32, right: 32, top: 24, bottom: 20 }))
                        .show(ctx, |ui| self.dub_screen(ui, ctx));
                }
                Screen::Apozh => {
                    egui::TopBottomPanel::top("hdr_a").exact_height(76.0).show_separator_line(true)
                        .frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin::symmetric(32, 0)))
                        .show(ctx, |ui| self.header_apozh(ui));
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin { left: 32, right: 32, top: 24, bottom: 20 }))
                        .show(ctx, |ui| self.apozh_screen(ui, ctx));
                }
                Screen::Game => {
                    egui::TopBottomPanel::top("hdr_g").exact_height(76.0).show_separator_line(true)
                        .frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin::symmetric(32, 0)))
                        .show(ctx, |ui| self.header_game(ui));
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin { left: 32, right: 32, top: 24, bottom: 20 }))
                        .show(ctx, |ui| self.game_screen(ui, ctx));
                }
                Screen::Profile => {
                    egui::TopBottomPanel::top("hdr_p").exact_height(76.0).show_separator_line(true)
                        .frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin::symmetric(32, 0)))
                        .show(ctx, |ui| self.header_profile(ui));
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin { left: 48, right: 48, top: 28, bottom: 20 }))
                        .show(ctx, |ui| self.profile(ui, ctx));
                }
                Screen::Recordings => {
                    egui::TopBottomPanel::top("hdr_r").exact_height(76.0).show_separator_line(true)
                        .frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin::symmetric(32, 0)))
                        .show(ctx, |ui| self.header_recordings(ui));
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin { left: 32, right: 32, top: 24, bottom: 20 }))
                        .show(ctx, |ui| self.recordings(ui, ctx));
                }
                Screen::Result => {
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(BG())).show(ctx, |ui| self.result_screen(ui, ctx));
                }
            }
        }

        self.settings_modal(ctx);

        // toast (non-session messages)
        if let Some((msg, t)) = &self.toast {
            if t.elapsed() > Duration::from_secs(7) { self.toast = None; } else {
                let msg = msg.clone();
                egui::Area::new(egui::Id::new("toast")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -24.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
                    kit::warning(ui, &msg, 480.0);
                });
            }
        }
        // engine warnings outside the session screen
        if screen != Screen::Session {
            if let Some((n, _)) = self.note.take() { self.toast = Some((n, Instant::now())); }
        }

        // keep the native video view glued to its slot; hide it under overlays
        let show = !self.show_settings && !self.show_roster && !self.show_games && (matches!(self.screen(), Screen::Session | Screen::Dub) || self.fullscreen) && !self.video_hidden;
        let (vr, ppp) = (self.video_rect, ctx.pixels_per_point());
        if let Some(v) = &mut self.video { v.place(vr, ppp, show); }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.settings.save();
        self.cancel();
    }
}

fn main() -> eframe::Result<()> {
    let mut vp = egui::ViewportBuilder::default()
        .with_title("Караоке-судья")
        .with_inner_size(if std::env::var_os("KJ_DEMO").is_some() { [1600.0, 1000.0] } else { [1280.0, 800.0] })
        .with_min_inner_size([1100.0, 700.0]);
    if let Ok(i) = image::load_from_memory(include_bytes!("../assets/icon256.png")) {
        let i = i.to_rgba8();
        vp = vp.with_icon(Arc::new(egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() }));
    }
    let options = eframe::NativeOptions { viewport: vp, ..Default::default() };
    eframe::run_native("Караоке-судья", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn voice_types_and_notes() {
        // typical ranges land on their own type
        for (name, lo, hi) in VOICES { assert_eq!(voice_type(lo, hi), name, "{name}"); }
        // an untrained male voice G2–D4 → baritone or bass, never soprano
        let t = voice_type(43.0, 62.0);
        assert!(t == "Баритон" || t == "Бас", "{t}");
        // an untrained female voice A3–C5
        let t = voice_type(57.0, 72.0);
        assert!(t == "Контральто" || t == "Меццо-сопрано", "{t}");
        assert_eq!(note_name(69.0), "Ля первой октавы");
        assert_eq!(note_name(57.0), "Ля малой октавы");
        assert_eq!(note_name(45.0), "Ля большой октавы");
        assert_eq!(note_name(72.0), "До второй октавы");
        assert_eq!(note_name(61.0), "До♯ первой октавы");
        println!("43–62 → {}, 57–72 → {}", voice_type(43.0, 62.0), voice_type(57.0, 72.0));
    }

    #[test]
    fn recording_names_roundtrip() {
        let dir = std::env::temp_dir().join(format!("kj_rec_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        for (title, score) in [("Виктор Цой - Группа крови (Караоке)", 87u32), ("AC/DC: Highway to Hell? \"Live\" <karaoke>", 64), ("Кино — Группа крови — минус", 100)] {
            let raw = dir.join("karaoke_2026-11-11_00-41-07.wav");
            let spec = hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
            let mut w = hound::WavWriter::create(&raw, spec).unwrap();
            for i in 0..48000 * 2 { w.write_sample(((i as f32 * 0.05).sin() * 8000.0) as i16).unwrap(); }
            w.finalize().unwrap();
            let named = name_recording(&raw, title, score);
            let it = parse_recording(&named).unwrap();
            println!("{} -> {:?} | {} | {} | {:.1}s", title, named.file_name().unwrap(), it.title, it.when, it.secs);
            assert!(it.when.contains("00:41"));
            assert_eq!(it.score, Some(score));
            assert!((it.secs - 2.0).abs() < 0.01);
            assert!(!it.title.is_empty());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moves_folders_and_files() {
        let base = std::env::temp_dir().join("kj_move_test");
        let _ = std::fs::remove_dir_all(&base);
        let a = base.join("old/Озвучка");
        std::fs::create_dir_all(a.join("sub")).unwrap();
        std::fs::write(a.join("ролик.mp4"), b"x").unwrap();
        std::fs::write(a.join("sub/1.wav"), b"y").unwrap();
        let b = base.join("new/Озвучка");
        std::fs::create_dir_all(base.join("new")).unwrap();
        assert!(super::move_any(&a, &b));
        assert!(b.join("ролик.mp4").exists() && b.join("sub/1.wav").exists() && !a.exists());
    }

    #[test]
    fn odd_stamps_do_not_crash() {
        assert_eq!(super::pretty_when("2026-11-13 21:40:00"), "13.11 21:40");
        assert_eq!(super::pretty_when("сегодня 21:40 вечером"), "сегодня 21:40 вечером");
    }
}
