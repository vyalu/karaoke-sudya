//! «Озвучка»: a short clip (video or sound) is cut into lines by pauses; every player
//! dubs the whole clip in turn while the video plays silently; in the end each version
//! plays back with that player's voice, and the judge says whose was closer.

use super::*;
use crate::wave;
use std::path::Path;

/* ---------- pure helpers (tested) ---------- */

pub const EXT_VIDEO: [&str; 4] = ["mp4", "m4v", "mov", "webm"];
pub const EXT_AUDIO: [&str; 5] = ["mp3", "wav", "ogg", "flac", "m4a"];
/// Longest clip we load (a dub scene should be short anyway).
pub const MAX_SECS: f32 = 180.0;
/// The video starts this much before a line, so the player sees it coming.
pub const PREROLL: f32 = 1.0;
/// How eagerly the loudness cutter finds lines (0..1).
const SENS: f32 = 0.5;

pub fn is_video(p: &Path) -> bool { ext_in(p, &EXT_VIDEO) }
fn ext_in(p: &Path, list: &[&str]) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| list.contains(&e.to_lowercase().as_str())).unwrap_or(false)
}
pub fn is_scene(p: &Path) -> bool { p.is_file() && (ext_in(p, &EXT_VIDEO) || ext_in(p, &EXT_AUDIO)) }

/// Speech-like stretches: loud parts separated by pauses. `sens` 0..1 — higher finds quieter speech.
pub fn segment(x: &[f32], sr: u32, sens: f32) -> Vec<(f32, f32)> {
    let hop = (sr as f32 * 0.02) as usize;
    if hop == 0 || x.len() < hop * 10 { return vec![]; }
    // listen only to the voice range: birds, whistles and hiss (above ~1,5 kHz) don't make lines
    let (mut f1, mut f2, mut f3) = (Bq::new(1300.0, sr, false), Bq::new(1300.0, sr, false), Bq::new(1300.0, sr, false));
    let (mut h1, mut h2) = (Bq::new(80.0, sr, true), Bq::new(80.0, sr, true));
    let low: Vec<f32> = x.iter().map(|&v| h2.run(h1.run(f3.run(f2.run(f1.run(v)))))).collect();
    // (loudness in the voice range, is it mostly high-pitched sound — a bird alone?)
    let frames: Vec<(f32, bool)> = low.chunks(hop).zip(x.chunks(hop)).map(|(c, full)| {
        let e = c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32;
        let ef = full.iter().map(|v| v * v).sum::<f32>() / full.len() as f32;
        (10.0 * (e + 1e-10).log10(), e < ef * 0.15)
    }).collect();
    let mut sorted: Vec<f32> = frames.iter().map(|f| f.0).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let q = |p: f32| sorted[((sorted.len() - 1) as f32 * p) as usize];
    let (floor, peak) = (q(0.15), q(0.95));
    let thr = floor + (peak - floor) * (0.55 - 0.4 * sens.clamp(0.0, 1.0));
    // a frame that is loud overall but has little in the voice range is not a line
    let db: Vec<f32> = frames.iter().map(|&(d, high)| if high { floor } else { d }).collect();
    let t = |i: usize| i as f32 * 0.02;
    // raw runs above the threshold
    let mut segs: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (i, &d) in db.iter().enumerate() {
        match (d > thr, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => { segs.push((s, i)); start = None; }
            _ => {}
        }
    }
    if let Some(s) = start { segs.push((s, db.len())); }
    // join short gaps, drop blips
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for s in segs {
        match merged.last_mut() {
            Some(l) if s.0 - l.1 <= 20 => l.1 = s.1,
            _ => merged.push(s),
        }
    }
    merged.retain(|s| s.1 - s.0 >= 18);
    // split long ones at their quietest point
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut stack: Vec<(usize, usize)> = merged.into_iter().rev().collect();
    while let Some((a, b)) = stack.pop() {
        if b - a > 350 {
            let lo = a + (b - a) / 4;
            let hi = b - (b - a) / 4;
            let m = (lo..hi).min_by(|&i, &j| db[i].total_cmp(&db[j])).unwrap_or((a + b) / 2);
            stack.push((m, b));
            stack.push((a, m));
        } else { out.push((a, b)); }
    }
    let dur = t(db.len());
    let mut v: Vec<(f32, f32)> = out.into_iter().map(|(a, b)| ((t(a) - 0.12).max(0.0), (t(b) + 0.12).min(dur))).collect();
    // neighbours must not overlap: meet in the middle with a small gap
    for i in 1..v.len() {
        if v[i].0 < v[i - 1].1 + 0.06 {
            let m = (v[i].0 + v[i - 1].1) / 2.0;
            v[i - 1].1 = m - 0.03;
            v[i].0 = m + 0.03;
        }
    }
    v
}

/// Pitch of a piece, one value (MIDI) per 20 ms; None where nobody speaks.
pub fn contour(x: &[f32], sr: u32) -> Vec<Option<f32>> {
    let hop = (sr as f32 * 0.02) as usize;
    let win = ((sr as f32 * 0.04) as usize).max(512);
    if hop == 0 || x.len() < win { return vec![]; }
    let mut out: Vec<Option<f32>> = (0..(x.len() - win) / hop).map(|i| {
        let w = &x[i * hop..i * hop + win];
        if dsp::rms(w) < 0.01 { return None; }
        dsp::pitch(w, sr as f32, 0.8).filter(|(f, _)| *f > 70.0 && *f < 700.0).map(|(f, _)| dsp::to_midi(f))
    }).collect();
    // drop lonely blips (music, clicks)
    for i in 1..out.len().saturating_sub(1) { if out[i].is_some() && out[i - 1].is_none() && out[i + 1].is_none() { out[i] = None; } }
    out
}

/// Middle of the voiced notes.
pub fn median_note(c: &[Option<f32>]) -> Option<f32> {
    let mut v: Vec<f32> = c.iter().flatten().copied().collect();
    if v.len() < 5 { return None; }
    v.sort_by(|a, b| a.total_cmp(b));
    Some(v[v.len() / 2])
}

/// Linear resampling (good enough for voices in a mix). Going down, the highs that the new
/// rate can't hold are filtered out first, so they don't fold back as hiss.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() { return x.to_vec(); }
    let filtered;
    let x = if to < from {
        let (mut a, mut b) = (Bq::new(to as f32 * 0.45, from, false), Bq::new(to as f32 * 0.45, from, false));
        filtered = x.iter().map(|&v| b.run(a.run(v))).collect::<Vec<f32>>();
        &filtered[..]
    } else { x };
    let n = (x.len() as f64 * to as f64 / from as f64) as usize;
    (0..n).map(|i| {
        let p = i as f64 * from as f64 / to as f64;
        let k = p as usize;
        let f = (p - k as f64) as f32;
        let a = x[k.min(x.len() - 1)];
        let b = x[(k + 1).min(x.len() - 1)];
        a + (b - a) * f
    }).collect()
}

/// The finished dub: takes placed at their times over a quiet original.
pub fn mix(orig: &[f32], sr: u32, bg: f32, takes: &[(f32, &[f32], u32)]) -> Vec<f32> {
    let mut out: Vec<f32> = orig.iter().map(|v| v * bg).collect();
    for &(at, s, tsr) in takes {
        let s = resample(s, tsr, sr);
        let start = (at.max(0.0) * sr as f32) as usize;
        if out.len() < start + s.len() { out.resize(start + s.len(), 0.0); }
        for (k, v) in s.iter().enumerate() { out[start + k] += v; }
    }
    for v in out.iter_mut() { *v = v.clamp(-1.0, 1.0); }
    out
}

/// RBJ biquad, low- or high-pass.
struct Bq { b: [f32; 3], a: [f32; 2], x: [f32; 2], y: [f32; 2] }
impl Bq {
    fn new(f: f32, sr: u32, high: bool) -> Self {
        let w = std::f32::consts::TAU * f / sr as f32;
        let (sn, cs) = w.sin_cos();
        let al = sn / (2.0 * 0.707);
        let a0 = 1.0 + al;
        let b = if high { [(1.0 + cs) / 2.0, -(1.0 + cs), (1.0 + cs) / 2.0] } else { [(1.0 - cs) / 2.0, 1.0 - cs, (1.0 - cs) / 2.0] };
        Self { b: [b[0] / a0, b[1] / a0, b[2] / a0], a: [-2.0 * cs / a0, (1.0 - al) / a0], x: [0.0; 2], y: [0.0; 2] }
    }
    fn run(&mut self, v: f32) -> f32 {
        let y = self.b[0] * v + self.b[1] * self.x[0] + self.b[2] * self.x[1] - self.a[0] * self.y[0] - self.a[1] * self.y[1];
        self.x = [v, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// The clip's sound with the voices pulled out during the lines. With `clean` (the neural net's
/// background without voices, «Нейросети» add-on) that is used; without it the original is only
/// turned down there.
pub fn devoice(mid: &[f32], sr: u32, lines: &[(f32, f32)], clean: Option<&[f32]>) -> Vec<f32> {
    let n = mid.len();
    // 0 = original, 1 = voice removed; soft edges
    let mut mask = vec![0f32; n];
    let fade = (0.08 * sr as f32) as usize;
    for &(a, b) in lines {
        let a = ((a - 0.1).max(0.0) * sr as f32) as usize;
        let b = (((b + 0.1) * sr as f32) as usize).min(n);
        for (i, m) in mask.iter_mut().enumerate().take((b + fade).min(n)).skip(a.saturating_sub(fade)) {
            let k = if i < a { 1.0 - (a - i) as f32 / fade as f32 } else if i >= b { 1.0 - (i - b) as f32 / fade as f32 } else { 1.0 };
            *m = m.max(k.clamp(0.0, 1.0));
        }
    }
    match clean.filter(|c| c.len() == n) {
        Some(c) => (0..n).map(|i| mid[i] * (1.0 - mask[i]) + c[i] * mask[i]).collect(),
        None => (0..n).map(|i| mid[i] * (1.0 - 0.8 * mask[i])).collect(),
    }
}

/// `file:///C:/...` for the WebView (Cyrillic and spaces escaped).
pub fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file:///");
    for b in s.trim_start_matches('/').bytes() {
        if b.is_ascii_alphanumeric() || b"/:-_.~".contains(&b) { out.push(b as char); } else { out.push_str(&format!("%{b:02X}")); }
    }
    out
}

fn lines_path(scene: &Path) -> PathBuf { scene.with_extension(format!("{}.lines", scene.extension().and_then(|e| e.to_str()).unwrap_or(""))) }

pub fn save_lines(scene: &Path, lines: &[Line]) {
    let t: String = lines.iter().map(|l| {
        let txt = l.text.replace(['\t', '\n', '\r'], " ");
        if txt.trim().is_empty() { format!("{:.2}\t{:.2}\n", l.start, l.end) } else { format!("{:.2}\t{:.2}\t{}\n", l.start, l.end, txt.trim()) }
    }).collect();
    let _ = std::fs::write(lines_path(scene), t);
}
/// Moments from the text of a .lines file (tabs or spaces, dot or comma decimals); a third
/// tab column is the phrase's text. Subtitles (.srt / .vtt text) are understood as well.
pub fn parse_lines(t: &str) -> Option<Vec<Line>> {
    if t.contains("-->") {
        let v: Vec<Line> = crate::subtitles::parse_subtitles(t).into_iter().map(|p| Line { start: p.start, end: p.end, text: p.text }).collect();
        return (!v.is_empty()).then_some(v);
    }
    let num = |s: &str| s.trim().replace(',', ".").parse::<f32>().ok();
    let mut v: Vec<Line> = t.lines().filter_map(|l| {
        let p: Vec<&str> = l.split(|c: char| c == '\t' || c == ' ' || c == ';').filter(|x| !x.is_empty()).collect();
        let (a, b) = (num(p.first()?)?, num(p.get(1)?)?);
        // text: everything after the second tab, unless it is just numbers (old extra columns)
        let text = l.splitn(3, '\t').nth(2).map(|x| x.trim().to_string()).filter(|x| !x.is_empty() && x.split_whitespace().any(|w| num(w).is_none())).unwrap_or_default();
        (b > a).then_some(Line { start: a, end: b, text })
    }).collect();
    v.sort_by(|a, b| a.start.total_cmp(&b.start));
    (!v.is_empty()).then_some(v)
}

/// `a..b` seconds of a sound.
fn slice(x: &[f32], sr: u32, a: f32, b: f32) -> Vec<f32> {
    let i = ((a.max(0.0) * sr as f32).round() as usize).min(x.len());
    let j = ((b * sr as f32).round() as usize).min(x.len()).max(i);
    x[i..j].to_vec()
}

/// After keeping only `pieces` of a clip: where the moments are now. A moment keeps its place
/// inside the piece it overlaps most; one that falls into a cut-out gap is dropped.
pub fn move_moments(lines: &[Line], pieces: &[(f32, f32)]) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let mut off = 0.0;
    let starts: Vec<f32> = pieces.iter().map(|&(a, b)| { let o = off; off += b - a; o }).collect();
    for l in lines {
        let best = pieces.iter().enumerate()
            .map(|(k, &(a, b))| (k, l.end.min(b) - l.start.max(a)))
            .filter(|&(_, ov)| ov > 0.1)
            .max_by(|x, y| x.1.total_cmp(&y.1));
        let Some((k, _)) = best else { continue };
        let (a, b) = pieces[k];
        let (s, e) = (starts[k] + l.start.max(a) - a, starts[k] + l.end.min(b) - a);
        if e - s >= 0.2 { out.push(Line { start: s, end: e, text: l.text.clone() }); }
    }
    out.sort_by(|x, y| x.start.total_cmp(&y.start));
    out
}

/// Share a phrase's words between two parts of a moment by time (0..1).
fn split_text(t: &str, frac: f32) -> (String, String) {
    let w: Vec<&str> = t.split_whitespace().collect();
    if w.is_empty() { return (String::new(), String::new()); }
    let total: usize = w.iter().map(|x| x.chars().count() + 1).sum();
    let (mut acc, mut k) = (0usize, 0usize);
    while k < w.len() && (acc as f32 + (w[k].chars().count() + 1) as f32 / 2.0) < frac * total as f32 { acc += w[k].chars().count() + 1; k += 1; }
    (w[..k].join(" "), w[k..].join(" "))
}

/// A moments file: our .lines or subtitles (.srt, .vtt).
fn is_moments_file(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| ["lines", "srt", "vtt"].iter().any(|x| e.eq_ignore_ascii_case(x))).unwrap_or(false)
}

/// A name without extensions and copy marks: «Сцена (2).mp4.lines» → «сцена».
fn base_key(name: &str) -> String {
    let mut n = name.to_lowercase();
    for e in [".lines", ".srt", ".vtt"] { if let Some(x) = n.strip_suffix(e) { n = x.to_string(); break; } }
    for e in EXT_VIDEO.iter().chain(EXT_AUDIO.iter()) { if let Some(x) = n.strip_suffix(&format!(".{e}")) { n = x.to_string(); break; } }
    let t = n.trim_end();
    // « (2)», « (обрезка)» and the like
    let t = match t.rfind(" (") { Some(i) if t.ends_with(')') => &t[..i], _ => t };
    t.trim().to_string()
}

/// The moments file of a clip: its own, or one that clearly belongs to it (another copy's name, no extension).
pub fn find_lines(scene: &Path) -> Option<PathBuf> {
    let own = lines_path(scene);
    if own.exists() { return Some(own); }
    let dir = scene.parent()?;
    let key = base_key(&scene.file_name()?.to_string_lossy());
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path())
        .filter(|p| is_moments_file(p))
        .find(|p| base_key(&p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()) == key)
}

pub fn load_lines(scene: &Path) -> Option<Vec<Line>> {
    let found = find_lines(scene)?;
    let v = parse_lines(&std::fs::read_to_string(&found).ok()?)?;
    // adopt a file found under another name: from now on it is this clip's own
    if found != lines_path(scene) { save_lines(scene, &v); }
    Some(v)
}

/// The old (1.0 beta) «virtual» trim: moments were counted from its start. Shift them to the file's
/// own time once and put the old file aside.
fn upgrade_old_trim(scene: &Path) {
    let tp = scene.with_extension(format!("{}.trim", scene.extension().and_then(|e| e.to_str()).unwrap_or("")));
    let Ok(t) = std::fs::read_to_string(&tp) else { return };
    let a = t.split_whitespace().next().and_then(|x| x.replace(',', ".").parse::<f32>().ok()).unwrap_or(0.0);
    if a > 0.0 {
        if let Some(v) = find_lines(scene).and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| parse_lines(&t)) {
            let shifted: Vec<Line> = v.into_iter().map(|l| Line { start: l.start + a, end: l.end + a, text: l.text }).collect();
            save_lines(scene, &shifted);
        }
    }
    let _ = std::fs::rename(&tp, tp.with_extension("trim.old"));
}

/* ---------- state ---------- */

#[derive(Clone)]
pub struct Take { pub at: f32, pub samples: Vec<f32>, pub sr: u32 }

#[derive(Clone, Default, PartialEq, Debug)]
pub struct Line { pub start: f32, pub end: f32, /// what is said (from speech recognition or subtitles)
    pub text: String }
impl Line { pub fn new(start: f32, end: f32) -> Self { Line { start, end, text: String::new() } } }

#[derive(Clone, Copy)]
enum Drag { Edge(usize, bool), New(f32, f32) }


#[derive(Clone, Copy, PartialEq)]
pub enum DPhase { Setup, Edit, Record, Final }

pub struct Dub {
    root: PathBuf,
    scenes: Vec<PathBuf>,
    /// the line under each clip in the list
    status: std::collections::HashMap<PathBuf, &'static str>,
    /// categories: folders inside «Озвучка»
    cats: Vec<String>,
    /// the list shows: None — everything, Some("") — clips without a category, Some(name)
    cat: Option<String>,
    /// a new category being named
    new_cat: Option<String>,
    /// the clip dubbed last (the random pick avoids it)
    last_pick: Option<PathBuf>,
    pub phase: DPhase,
    players: Vec<String>,
    /// takes[player][line]
    takes: Vec<Vec<Option<Take>>>,
    /// who is dubbing now / whose version is shown
    who: usize,
    scene: Option<PathBuf>,
    video: bool,
    orig: Vec<f32>, side: Vec<f32>, sr: u32,
    /// pull the original voices out during the lines (keeps music and effects)
    devoice: bool,
    lines: Vec<Line>,
    cur: usize,
    pub mic: Option<audio::Recorder>,
    rec_since: Option<Instant>,
    /// video is playing a piece; pause it at this moment
    stop_at: Option<Instant>,
    /// final playback started at
    show_since: Option<Instant>,
    bg: f32,
    last_scan: Instant,
    loading: bool,
    /// live pitch of the mic in the recording screen («окно голоса»)
    mon: Option<audio::PitchMonitor>,
    /// what the player sang during the current take: (clip time, MIDI)
    live: Vec<(f32, f32)>,
    /// pitch of the original around the current line: (line, from, contour)
    refc: Option<(usize, f32, Vec<Option<f32>>)>,
    /// loudness outline of the same stretch
    refenv: wave::Env,
    /// loudness of the take in progress: (clip time, level)
    live_lvl: Vec<(f32, f32)>,
    /// the clip is being shown whole before the first line
    intro: bool,
    /// the selected moment in the editor
    sel: Option<usize>,
    /// editor: visible stretch of the clip, a cursor, what is being dragged
    view: (f32, f32),
    cursor: f32,
    drag: Option<Drag>,
    /// the neural net listening to a clip for its phrases
    listen: Option<Listen>,
    /// the clip's sound with the voices taken out by the neural net («Нейросети» add-on)
    clean: Option<Vec<f32>>,
    /// a finished dub being saved as video
    export: Option<std::thread::JoinHandle<Result<PathBuf, String>>>,
    /// keep nudging the video to the right place for a moment (the player may still be loading)
    sync_until: Option<Instant>,
    /// play the clip's background (music, effects — no voices) under the recording
    rec_bg: bool,
    /// what was played under the take in progress (to take it out of the recording afterwards)
    rec_ref: Option<Vec<f32>>,
    /// who dubs on the next «Озвучивать» (index in today's players)
    next_who: usize,
    /// «✂ Обрезать ролик»: the editor shows the pieces to keep instead of the moments
    cut_mode: bool,
    lines_stash: Vec<Line>,
    /// preview of the cut: (started, pieces to keep)
    preview: Option<(Instant, Vec<(f32, f32)>)>,
    last_sync: Instant,
    /// loudness outline of the whole clip (20 ms per bin)
    fullenv: wave::Env,
    /// loudest sample of the clip (to draw the zoomed waveform on the same scale)
    peak_max: f32,
    /// something is playing from this clip time since this moment (for the cursor)
    play_from: Option<(Instant, f32)>,
}

/// The neural net taking the voices out of one clip, in the background.
pub struct Listen {
    scene: PathBuf,
    /// progress, per mille
    stage: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// the moments when it started; if they are still the automatic ones, they are cut anew
    /// on the voices alone (much cleaner than on the whole sound)
    before: Option<Vec<Line>>,
    /// (the background without voices, the moments found on the voices)
    job: std::thread::JoinHandle<Result<(Vec<f32>, Vec<Line>), String>>,
}
impl Listen {
    pub fn status(&self) -> String {
        format!("Нейросеть убирает голоса из ролика… {}%", self.stage.load(std::sync::atomic::Ordering::Relaxed) / 10)
    }
}

fn pcol(i: usize) -> Color32 { player_color(i) }

pub fn dub_root() -> PathBuf { user_dir("Озвучка") }

/// Category folders that come with the program (made once, when there are none yet).
const DEFAULT_CATS: [&str; 3] = ["Фильмы", "Мультфильмы", "Мемы"];
const DONE: &str = "Готовые";

/// The category folders inside «Озвучка» (made the first time).
pub fn categories(root: &Path) -> Vec<String> {
    let list = |root: &Path| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(root).map(|rd| rd.flatten().filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != DONE && !n.starts_with('.')).collect()).unwrap_or_default();
        v.sort_by_key(|n| (DEFAULT_CATS.iter().position(|d| d == n).unwrap_or(99), n.to_lowercase()));
        v
    };
    let v = list(root);
    let mark = root.join(".categories");
    if v.is_empty() && !mark.exists() {
        for c in DEFAULT_CATS { let _ = std::fs::create_dir_all(root.join(c)); }
        let _ = std::fs::write(&mark, "");
        return list(root);
    }
    v
}

/// The category of a clip ("" — none).
fn cat_of(root: &Path, p: &Path) -> String {
    match p.parent() { Some(d) if d != root => d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), _ => String::new() }
}

/// Move a clip with its moments and its voiceless sound into another folder.
fn move_scene(p: &Path, to_dir: &Path) -> Option<PathBuf> {
    let name = p.file_name()?;
    let mut dst = to_dir.join(name);
    let mut k = 2;
    while dst.exists() { dst = to_dir.join(format!("{} ({k}).{}", scene_name(p), p.extension().and_then(|e| e.to_str()).unwrap_or("mp4"))); k += 1; }
    if !move_any(p, &dst) { return None; }
    for (a, b) in [(lines_path(p), lines_path(&dst)), (crate::separate::cache_path(p), crate::separate::cache_path(&dst))] {
        if a.exists() { move_any(&a, &b); }
    }
    Some(dst)
}

/// A random pick out of `n`, never `avoid` when there is a choice.
fn random_pick(n: usize, avoid: Option<usize>) -> Option<usize> {
    if n == 0 { return None; }
    let r = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let r = (r ^ (r >> 17)).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 7;
    match avoid.filter(|_| n > 1) {
        Some(a) => { let k = (r % (n as u128 - 1)) as usize; Some(if k >= a { k + 1 } else { k }) }
        None => Some((r % n as u128) as usize),
    }
}
/// Finished dubs go here, so they don't show up as clips to dub.
fn done_dir(root: &Path) -> PathBuf { let d = root.join(DONE); let _ = std::fs::create_dir_all(&d); d }

impl Dub {
    /// While the neural nets still work on the open clip: what they say. Recording waits for them,
    /// so the moments don't change under the player's takes.
    fn cleaning(&self) -> Option<String> {
        let l = self.listen.as_ref().filter(|l| Some(&l.scene) == self.scene.as_ref())?;
        Some(format!("Подождите — нейросеть убирает голоса из ролика: {}%", l.stage.load(std::sync::atomic::Ordering::Relaxed) / 10))
    }
    /// Somebody has already recorded something on this clip.
    fn any_takes(&self) -> bool { self.takes.iter().any(|v| v.iter().any(|t| t.is_some())) }

    /// What plays under the players' voices.
    fn background(&self) -> Vec<f32> {
        if !self.devoice { return self.orig.clone(); }
        let spans: Vec<(f32, f32)> = self.lines.iter().map(|l| (l.start, l.end)).collect();
        devoice(&self.orig, self.sr, &spans, self.clean.as_deref())
    }
    pub fn new() -> Self {
        let mut d = Self {
            root: dub_root(), scenes: vec![], status: Default::default(), cats: vec![], cat: None, new_cat: None, last_pick: None, phase: DPhase::Setup, players: vec![], takes: vec![], who: 0,
            scene: None, video: false, orig: vec![], side: vec![], devoice: true, sr: 48000, lines: vec![], cur: 0, mic: None,
            rec_since: None, stop_at: None, show_since: None, bg: 0.8, last_scan: Instant::now(), loading: false, mon: None, live: vec![], refc: None, refenv: wave::Env::default(), live_lvl: vec![], intro: false, sel: None, view: (0.0, 0.0), cursor: 0.0, drag: None, export: None, sync_until: None, last_sync: Instant::now(), rec_bg: true, rec_ref: None, next_who: 0, listen: None, clean: None, cut_mode: false, lines_stash: vec![], preview: None, fullenv: wave::Env::default(), peak_max: 1.0, play_from: None,
        };
        d.scan();
        d
    }
    fn scan(&mut self) {
        self.cats = categories(&self.root);
        if let Some(c) = &self.cat { if !c.is_empty() && !self.cats.contains(c) { self.cat = None; } }
        // clips lie in «Озвучка» itself and one level down, in the category folders
        let mut v: Vec<PathBuf> = std::iter::once(self.root.clone()).chain(self.cats.iter().map(|c| self.root.join(c)))
            .flat_map(|dir| std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| is_scene(p)).collect::<Vec<_>>()).unwrap_or_default())
            .collect();
        v.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
        // a line under each clip in the list (read here, every couple of seconds — not every frame)
        self.status = v.iter().map(|p| {
            let lp = lines_path(p);
            let st = match std::fs::read_to_string(&lp) {
                Err(_) => "моменты найдутся сами",
                Ok(t) if t.lines().any(|l| l.splitn(3, '\t').nth(2).is_some()) => "реплики с текстом",
                Ok(_) => "моменты размечены",
            };
            let clean = if crate::separate::cache_path(p).exists() { " · голоса убраны" } else { "" };
            (p.clone(), if clean.is_empty() { st } else { match st { "реплики с текстом" => "реплики с текстом · голоса убраны", "моменты размечены" => "моменты размечены · голоса убраны", _ => "голоса убраны" } })
        }).collect();
        self.scenes = v;
        self.last_scan = Instant::now();
    }
    fn resplit(&mut self) {
        self.lines = segment(&self.orig, self.sr, SENS).into_iter().map(|(a, b)| Line::new(a, b)).collect();
        self.save();
    }
    /// Save the moments (not in cut mode: then `lines` are the pieces of the cut).
    fn save(&self) {
        if self.cut_mode { return; }
        if let Some(s) = &self.scene { save_lines(s, &self.lines); }
    }
    /// Where the preview of the cut is now: (time in the file, time in the result).
    fn preview_pos(&self) -> Option<(f32, f32)> {
        let (t0, p) = self.preview.as_ref()?;
        let mut e = t0.elapsed().as_secs_f32();
        let total = e;
        for &(a, b) in p { if e < b - a { return Some((a + e, total)); } e -= b - a; }
        None
    }
    /// The stretch of the clip shown/played for a line: a bit before and after it.
    fn span(&self, i: usize) -> (f32, f32) {
        let l = &self.lines[i];
        let prev = if i > 0 { self.lines[i - 1].end } else { 0.0 };
        let next = self.lines.get(i + 1).map(|n| n.start).unwrap_or(self.dur());
        ((l.start - PREROLL).max(prev).max(0.0), (l.end + 0.6).min(next))
    }
    /// Recording of line `i`: starts with the video at `.0`, stops at `.1` (never inside a neighbour).
    fn rec_window(&self, i: usize) -> (f32, f32) {
        let l = &self.lines[i];
        let next = self.lines.get(i + 1).map(|n| n.start).unwrap_or(self.dur() + 1.0);
        (self.span(i).0, (l.end + 0.8).min(next))
    }
    fn piece(&self, a: f32, b: f32) -> Vec<f32> {
        let a = ((a * self.sr as f32) as usize).min(self.orig.len());
        let b = ((b * self.sr as f32) as usize).min(self.orig.len()).max(a);
        self.orig[a..b].to_vec()
    }
    /// The next line without a take, starting after `from`.
    fn next_open(&self, from: usize) -> usize {
        (from..self.lines.len()).chain(0..from).find(|&i| self.take(self.who, i).is_none()).unwrap_or(self.lines.len())
    }
    fn take(&self, p: usize, i: usize) -> Option<&Take> { self.takes.get(p).and_then(|v| v.get(i)).and_then(|t| t.as_ref()) }
    fn has_takes(&self, p: usize) -> bool { self.takes.get(p).map(|v| v.iter().any(|t| t.is_some())).unwrap_or(false) }
    /// How many lines a player dubbed.
    fn dubbed(&self, p: usize) -> usize { self.takes.get(p).map(|v| v.iter().flatten().count()).unwrap_or(0) }
    /// One player's version of the whole clip.
    fn version(&self, p: usize) -> Vec<f32> {
        // each take ends where the next moment starts, so voices never pile up
        let takes: Vec<(f32, &[f32], u32)> = self.takes.get(p).map(|v| v.iter().enumerate().filter_map(|(i, t)| t.as_ref().map(|t| {
            let next = self.lines.get(i + 1).map(|n| n.start).unwrap_or(f32::MAX);
            let n = (((next - t.at).max(0.0) * t.sr as f32) as usize).min(t.samples.len());
            (t.at, &t.samples[..n], t.sr)
        })).collect()).unwrap_or_default();
        mix(&self.background(), self.sr, self.bg, &takes)
    }
    fn dur(&self) -> f32 { self.orig.len() as f32 / self.sr as f32 }
}

/// The moments editor: the whole clip as a waveform with every moment on it.
/// Drag a moment's edge to move it, drag on an empty place to add one, click to select
/// or to put the cursor; the wheel zooms, shift+wheel scrolls.
fn moments_editor(ui: &mut egui::Ui, d: &mut Dub, width: f32, head: Option<f32>, act: &mut Option<(u8, usize)>) {
    let dur = d.dur().max(0.5);
    if let Some(l) = &d.listen {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(16.0).color(wave::W_FRAME));
            ui.label(RichText::new(l.status()).font(semibold(13.0)).color(wave::W_FRAME));
        });
        ui.ctx().request_repaint_after(Duration::from_millis(250));
        ui.add_space(4.0);
    }
    if d.view.1 <= d.view.0 || d.view.1 > dur + 0.01 { d.view = (0.0, dur); }
    let (r, resp) = ui.allocate_exact_size(Vec2::new(width, 130.0), egui::Sense::click_and_drag());
    let inner = r.shrink2(Vec2::new(6.0, 4.0));
    let (v0, v1) = d.view;
    let x = |t: f32| inner.left() + inner.width() * (t - v0) / (v1 - v0);
    let tx = |px: f32| (v0 + (px - inner.left()) / inner.width() * (v1 - v0)).clamp(0.0, dur);
    // zoom / scroll with the wheel
    if resp.hovered() {
        let (scroll, shift) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.shift));
        let dy = scroll.y + scroll.x;
        if dy.abs() > 0.1 {
            let w = v1 - v0;
            if shift || scroll.x.abs() > scroll.y.abs() {
                let sh = -dy / inner.width() * w;
                let a = (v0 + sh).clamp(0.0, dur - w);
                d.view = (a, a + w);
            } else if let Some(pos) = resp.hover_pos() {
                let at = tx(pos.x);
                let nw = (w * (1.0 - dy * 0.002).clamp(0.5, 2.0)).clamp(0.4, dur);
                let k = (at - v0) / w;
                let a = (at - k * nw).clamp(0.0, (dur - nw).max(0.0));
                d.view = (a, (a + nw).min(dur));
            }
        }
    }
    let p = ui.painter();
    wave::frame(p, r);
    if v1 - v0 > 20.0 {
        wave::draw_env(p, inner, &d.fullenv, v0, v1, wave::W_OUTER, wave::W_INNER);
    } else {
        // close up: straight from the samples, every syllable visible
        let cols = (inner.width() / 2.0) as usize;
        let h = inner.height() / 2.0 - 4.0;
        for c in 0..cols {
            let a = ((v0 + (v1 - v0) * c as f32 / cols as f32) * d.sr as f32) as usize;
            let b = (((v0 + (v1 - v0) * (c + 1) as f32 / cols as f32) * d.sr as f32) as usize).min(d.orig.len());
            if a >= b { continue; }
            let w = &d.orig[a..b];
            let pk = w.iter().fold(0f32, |m, v| m.max(v.abs())) / d.peak_max;
            let rm = ((w.iter().map(|v| v * v).sum::<f32>() / w.len() as f32).sqrt() / d.peak_max * 1.6).min(1.0);
            let xx = inner.left() + (c as f32 + 0.5) * inner.width() / cols as f32;
            p.line_segment([Pos2::new(xx, inner.center().y - pk * h), Pos2::new(xx, inner.center().y + pk * h)], Stroke::new(2.2_f32, wave::W_OUTER));
            p.line_segment([Pos2::new(xx, inner.center().y - rm * h), Pos2::new(xx, inner.center().y + rm * h)], Stroke::new(2.2_f32, wave::W_INNER));
        }
    }
    // ruler with times
    let span = v1 - v0;
    let step = [0.1f32, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0].into_iter().find(|s| span / s <= 12.0).unwrap_or(60.0);
    let mut k = (v0 / step).ceil() * step;
    while k < v1 {
        let xx = x(k);
        p.line_segment([Pos2::new(xx, r.bottom() - 9.0), Pos2::new(xx, r.bottom() - 2.0)], Stroke::new(1.0_f32, Color32::from_white_alpha(120)));
        let lab = if step < 1.0 { format!("{:.2}", k).replace('.', ",") } else { wave::time_label(k).split(',').next().unwrap_or("").to_string() };
        p.text(Pos2::new(xx + 3.0, r.bottom() - 4.0), Align2::LEFT_BOTTOM, lab, body(10.0), Color32::from_white_alpha(140));
        k += step;
    }
    // moments
    for (i, l) in d.lines.iter().enumerate() {
        if l.end < v0 || l.start > v1 { continue; }
        let mr = Rect::from_min_max(Pos2::new(x(l.start).max(inner.left()), r.top() + 4.0), Pos2::new(x(l.end).min(inner.right()), r.bottom() - 4.0));
        let sel = d.sel == Some(i) || d.cut_mode;
        p.rect_filled(mr, CornerRadius::same(6), if sel { Color32::from_rgba_unmultiplied(86, 204, 242, 46) } else { Color32::from_rgba_unmultiplied(255, 181, 71, 34) });
        p.rect_stroke(mr, CornerRadius::same(6), Stroke::new(if sel { 2.5_f32 } else { 1.5 }, if sel { wave::W_FRAME } else { AMBER() }), egui::StrokeKind::Inside);
        p.text(mr.left_top() + Vec2::new(6.0, 4.0), Align2::LEFT_TOP, format!("{}", i + 1), bold(12.0), if sel { wave::W_FRAME } else { AMBER() });
        if !l.text.is_empty() && mr.width() > 50.0 {
            p.with_clip_rect(mr.shrink(4.0)).text(mr.left_bottom() + Vec2::new(6.0, -14.0), Align2::LEFT_BOTTOM, &l.text, semibold(11.0), Color32::from_white_alpha(225));
        }
        if sel {
            for xx in [mr.left(), mr.right()] { p.rect_filled(Rect::from_center_size(Pos2::new(xx, mr.center().y), Vec2::new(8.0, 30.0)), CornerRadius::same(3), wave::W_FRAME); }
        }
    }
    // cut mode: darken what will be thrown away
    if d.cut_mode {
        let mut kept: Vec<(f32, f32)> = d.lines.iter().map(|l| (l.start, l.end)).collect();
        kept.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut t = 0.0f32;
        for (a, b) in kept.into_iter().chain(std::iter::once((dur, dur))) {
            if a > t && a > v0 && t < v1 {
                let gr = Rect::from_min_max(Pos2::new(x(t.max(v0)), r.top() + 2.0), Pos2::new(x(a.min(v1)), r.bottom() - 2.0));
                p.rect_filled(gr, CornerRadius::same(4), Color32::from_black_alpha(185));
                if gr.width() > 40.0 { p.text(gr.center(), Align2::CENTER_CENTER, "✂", bold(16.0), Color32::from_white_alpha(120)); }
            }
            t = t.max(b);
        }
    }
    // a moment being drawn
    if let Some(Drag::New(a, b)) = d.drag {
        let mr = Rect::from_min_max(Pos2::new(x(a.min(b)), r.top() + 4.0), Pos2::new(x(a.max(b)), r.bottom() - 4.0));
        p.rect_filled(mr, CornerRadius::same(6), Color32::from_rgba_unmultiplied(86, 204, 242, 60));
    }
    // cursor and playhead
    let cx = x(d.cursor);
    if cx >= inner.left() && cx <= inner.right() { p.line_segment([Pos2::new(cx, r.top() + 2.0), Pos2::new(cx, r.bottom() - 2.0)], Stroke::new(1.5_f32, Color32::WHITE)); }
    if let Some(h) = head { let hx = x(h); if hx >= inner.left() && hx <= inner.right() { p.line_segment([Pos2::new(hx, r.top() + 2.0), Pos2::new(hx, r.bottom() - 2.0)], Stroke::new(2.5_f32, wave::W_HEAD)); } }
    wave::frame_border(p, r);

    // ---- mouse
    // an edge outside the visible part is grabbed where it is drawn: at the window's border
    let near_edge = |px: f32, d: &Dub| -> Option<(usize, bool)> {
        let clampx = |t: f32| x(t).clamp(inner.left(), inner.right());
        d.lines.iter().enumerate().filter(|(_, l)| l.end >= v0 && l.start <= v1)
            .flat_map(|(i, l)| [(i, true, clampx(l.start)), (i, false, clampx(l.end))])
            .filter(|(_, _, ex)| (ex - px).abs() < 10.0)
            // the selected moment wins when edges sit together
            .min_by(|a, b| {
                let ka = ((a.2 - px).abs(), if d.sel == Some(a.0) { 0 } else { 1 });
                let kb = ((b.2 - px).abs(), if d.sel == Some(b.0) { 0 } else { 1 });
                ka.1.cmp(&kb.1).then(ka.0.total_cmp(&kb.0))
            })
            .map(|(i, s, _)| (i, s))
    };
    if let Some(pos) = resp.hover_pos() {
        ui.ctx().set_cursor_icon(if near_edge(pos.x, d).is_some() { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::Crosshair });
        let ht = tx(pos.x);
        ui.painter().text(Pos2::new(pos.x + 8.0, r.top() + 6.0), Align2::LEFT_TOP, wave::time_label(ht), semibold(11.0), Color32::from_white_alpha(200));
    }
    if resp.drag_started() {
        // decide by where the button went down, not where the mouse is after the first pixels of movement
        let origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
        if let Some(pos) = origin {
            d.drag = match near_edge(pos.x, d) {
                Some((i, s)) => { d.sel = Some(i); Some(Drag::Edge(i, s)) }
                None if !d.lines.iter().any(|l| tx(pos.x) >= l.start && tx(pos.x) <= l.end) => Some(Drag::New(tx(pos.x), tx(pos.x))),
                None => None,
            };
        }
    }
    if let (Some(pos), Some(dr)) = (resp.interact_pointer_pos(), d.drag) {
        let t = tx(pos.x);
        match dr {
            Drag::Edge(i, start) => {
                // near the window's border the view follows the mouse
                let w = d.view.1 - d.view.0;
                let edge_px = 14.0;
                let shift = if pos.x < inner.left() + edge_px { -w * 0.02 } else if pos.x > inner.right() - edge_px { w * 0.02 } else { 0.0 };
                if shift != 0.0 { let a0 = (d.view.0 + shift).clamp(0.0, (dur - w).max(0.0)); d.view = (a0, a0 + w); ui.ctx().request_repaint(); }
                let lo = if i > 0 { d.lines[i - 1].end + 0.06 } else { 0.0 };
                let hi = d.lines.get(i + 1).map(|n| n.start - 0.06).unwrap_or(dur);
                let l = &mut d.lines[i];
                if start { l.start = t.clamp(lo, l.end - 0.3); } else { l.end = t.clamp(l.start + 0.3, hi); }
                d.cursor = t;
            }
            Drag::New(a, _) => {
                // stay inside the free gap around the press
                let lo = d.lines.iter().filter(|l| l.end <= a).map(|l| l.end + 0.06).fold(0.0f32, f32::max);
                let hi = d.lines.iter().filter(|l| l.start >= a).map(|l| l.start - 0.06).fold(dur, f32::min);
                d.drag = Some(Drag::New(a, t.clamp(lo, hi)));
            }
        }
    }
    if resp.drag_stopped() {
        match d.drag.take() {
            Some(Drag::Edge(..)) => { d.save(); d.takes.clear(); }
            Some(Drag::New(a, b)) if (a - b).abs() >= 0.3 => {
                let (a, b) = (a.min(b), a.max(b));
                let i = d.lines.iter().position(|l| l.start > a).unwrap_or(d.lines.len());
                d.lines.insert(i, Line::new(a, b));
                d.sel = Some(i);
                d.save(); d.takes.clear();
            }
            _ => {}
        }
    }
    if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = tx(pos.x);
            d.cursor = t;
            d.sel = d.lines.iter().position(|l| t >= l.start && t <= l.end);
            *act = Some((27, 0)); // show that frame
        }
    }
    // ---- the whole clip in a thin strip: where you are; drag to scroll
    ui.add_space(4.0);
    let (mr, mresp) = ui.allocate_exact_size(Vec2::new(width, 24.0), egui::Sense::click_and_drag());
    {
        let p = ui.painter();
        p.rect_filled(mr, CornerRadius::same(6), Color32::BLACK);
        wave::draw_env(p, mr.shrink(2.0), &d.fullenv, 0.0, dur, wave::W_OUTER.gamma_multiply(0.7), wave::W_OUTER);
        let mx = |t: f32| mr.left() + mr.width() * t / dur;
        for l in &d.lines { p.rect_filled(Rect::from_min_max(Pos2::new(mx(l.start), mr.bottom() - 5.0), Pos2::new(mx(l.end).max(mx(l.start) + 1.0), mr.bottom() - 1.0)), CornerRadius::ZERO, AMBER()); }
        let vr = Rect::from_min_max(Pos2::new(mx(d.view.0), mr.top()), Pos2::new(mx(d.view.1).max(mx(d.view.0) + 3.0), mr.bottom()));
        p.rect_filled(vr, CornerRadius::same(4), Color32::from_rgba_unmultiplied(86, 204, 242, 40));
        p.rect_stroke(vr, CornerRadius::same(4), Stroke::new(1.5_f32, wave::W_FRAME), egui::StrokeKind::Inside);
    }
    if mresp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::Grab); }
    if let Some(pos) = mresp.interact_pointer_pos().filter(|_| mresp.dragged() || mresp.clicked()) {
        let c = (pos.x - mr.left()) / mr.width() * dur;
        let w = d.view.1 - d.view.0;
        let a = (c - w / 2.0).clamp(0.0, (dur - w).max(0.0));
        d.view = (a, a + w);
    }

    // ---- toolbar: playback · tools · zoom, then the selected moment
    ui.add_space(8.0);
    let sep = |ui: &mut egui::Ui| { ui.add_space(4.0); let (r, _) = ui.allocate_exact_size(Vec2::new(1.0, 26.0), egui::Sense::hover()); ui.painter().rect_filled(r, CornerRadius::ZERO, BORDER_STRONG()); ui.add_space(4.0); };
    let tool = |ui: &mut egui::Ui, t: &str, tip: &str, on: bool| ui.add_enabled(on, egui::Button::new(RichText::new(t).font(semibold(13.0)).color(TEXT())).fill(CHIP()).corner_radius(8.0).min_size(Vec2::new(34.0, 34.0))).on_hover_text(tip.to_string()).clicked();
    let playing = head.is_some();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        // playback
        if tool(ui, "⏮", "К предыдущему моменту", true) { *act = Some((40, 0)); }
        let (lab, code) = if playing { ("⏸", 39) } else { ("▶", 26) };
        if ui.add(egui::Button::new(RichText::new(lab).font(bold(15.0)).color(ON_ACCENT())).fill(ACCENT()).corner_radius(17.0).min_size(Vec2::new(46.0, 34.0))).on_hover_text("Пауза / продолжить (пробел)").clicked() { *act = Some((code, 0)); }
        if tool(ui, "⏭", "К следующему моменту", true) { *act = Some((41, 0)); }
        let now = head.unwrap_or(d.cursor);
        ui.label(RichText::new(format!("{} / {}", wave::time_label(now), wave::time_label(dur))).font(semibold(13.0)).color(TEXT_2()));
        sep(ui);
        // tools
        let word = if d.cut_mode { "+ Кусок" } else { "+ Момент" };
        if ui.add(egui::Button::new(RichText::new(word).font(semibold(13.0)).color(TEXT())).fill(CHIP()).corner_radius(8.0).min_size(Vec2::new(0.0, 34.0))).on_hover_text("Добавить у курсора (или протяните мышью по пустому месту)").clicked() { *act = Some((33, 0)); }
        if !d.cut_mode && ui.add(egui::Button::new(RichText::new("✂ Обрезать ролик").font(semibold(13.0)).color(TEXT_2())).fill(CHIP()).corner_radius(8.0).min_size(Vec2::new(0.0, 34.0))).on_hover_text("Вырезать лишнее из самого ролика").clicked() { *act = Some((21, 0)); }
        // zoom, on the right
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if tool(ui, "Весь", "Показать весь ролик", true) { d.view = (0.0, dur); }
            if tool(ui, "+", "Приблизить (или колесо мыши)", true) { let c = d.sel.and_then(|i| d.lines.get(i)).map(|l| (l.start + l.end) / 2.0).unwrap_or(d.cursor); let w = ((d.view.1 - d.view.0) / 1.6).max(0.4); let a = (c - w / 2.0).clamp(0.0, (dur - w).max(0.0)); d.view = (a, a + w); }
            if tool(ui, "−", "Отдалить", true) { let c = (d.view.0 + d.view.1) / 2.0; let w = ((d.view.1 - d.view.0) * 1.6).min(dur); let a = (c - w / 2.0).clamp(0.0, dur - w); d.view = (a, a + w); }
            ui.label(RichText::new("масштаб").font(body(12.0)).color(MUTED()));
        });
    });
    ui.add_space(6.0);
    // the selected moment
    egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, if d.sel.is_some() { wave::W_FRAME.gamma_multiply(0.6) } else { BORDER() })).corner_radius(12.0).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(width - 24.0);
        match d.sel.filter(|i| *i < d.lines.len()) {
            None => { ui.label(RichText::new(if d.cut_mode { "Нажмите на кусок, чтобы выбрать его. Пробел — пауза, ← → — сдвиг курсора." } else { "Нажмите на момент на волне, чтобы выбрать его. Пробел — пауза, ← → — сдвиг курсора." }).font(body(13.0)).color(MUTED())); }
            Some(i) => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new(format!("{} {}", if d.cut_mode { "Кусок" } else { "Момент" }, i + 1)).font(bold(14.0)).color(wave::W_FRAME));
                    ui.add_space(10.0);
                    let edge = |ui: &mut egui::Ui, title: &str, t: f32, minus: u8, plus: u8, act: &mut Option<(u8, usize)>| {
                        ui.label(RichText::new(title).font(body(12.0)).color(MUTED()));
                        if tool(ui, "‹", "на 0,05 с раньше", true) { *act = Some((minus, i)); }
                        ui.label(RichText::new(wave::time_label(t)).font(semibold(13.0)).color(TEXT()));
                        if tool(ui, "›", "на 0,05 с позже", true) { *act = Some((plus, i)); }
                    };
                    edge(ui, "начало", d.lines[i].start, 34, 35, act);
                    ui.add_space(8.0);
                    edge(ui, "конец", d.lines[i].end, 36, 37, act);
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("{:.1} с", d.lines[i].end - d.lines[i].start).replace('.', ",")).font(body(12.0)).color(MUTED()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if (!d.cut_mode || d.lines.len() > 1) && kit::icon_button(ui, Icon::Trash, 34.0).on_hover_text(if d.cut_mode { "Выкинуть этот кусок" } else { "Удалить момент" }).clicked() { *act = Some((7, i)); }
                        if tool(ui, "Склеить", "Склеить со следующим", i + 1 < d.lines.len()) { *act = Some((8, i)); }
                        let c = d.cursor;
                        let can_split = c > d.lines[i].start + 0.3 && c < d.lines[i].end - 0.3;
                        if tool(ui, "Разрезать", "Разрезать по белому курсору", can_split) { *act = Some((30, i)); }
                        if tool(ui, "🔍", "Приблизить к моменту", true) { *act = Some((38, i)); }
                        if tool(ui, "▶ Прослушать", "Только этот момент", true) { *act = Some((25, i)); }
                    });
                });
                if !d.cut_mode {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("текст").font(body(12.0)).color(MUTED()));
                        let r = ui.add(egui::TextEdit::singleline(&mut d.lines[i].text).hint_text("что здесь говорят — подсказка при записи").font(body(13.0)).desired_width(width - 90.0));
                        if r.lost_focus() { *act = Some((51, i)); }
                    });
                }
            }
        }
    });
    if d.cut_mode {
        ui.add_space(8.0);
        egui::Frame::new().fill(Color32::from_rgba_unmultiplied(86, 204, 242, 18)).stroke(Stroke::new(1.0_f32, wave::W_FRAME.gamma_multiply(0.6))).corner_radius(12.0).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
            ui.set_width(width - 28.0);
            let total: f32 = d.lines.iter().map(|l| l.end - l.start).sum();
            ui.horizontal(|ui| {
                ui.label(RichText::new("✂ Обрезка ролика").font(bold(14.0)).color(wave::W_FRAME));
                ui.add_space(8.0);
                ui.label(RichText::new(format!("останется {} из {} · кусков: {}", wave::time_label(total), wave::time_label(dur), d.lines.len())).font(semibold(13.0)).color(TEXT_2()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if kit::link(ui, "Отмена").clicked() { *act = Some((48, 0)); } });
            });
            ui.label(RichText::new("Синее — то, что останется. Поставьте курсор и нажмите «Разрезать», потом раздвиньте края — между кусками всё вырежется.").font(body(12.0)).color(MUTED()));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let pv = d.preview.is_some();
                if kit::button_ex(ui, Kind::Ghost, if pv { "Стоп" } else { "▶ Предпросмотр результата" }, None, Vec2::new(0.0, 38.0), semibold(13.0), 10).clicked() { *act = Some(if pv { (27, 0) } else { (45, 0) }); }
                if kit::button_ex(ui, Kind::Primary, "Перезаписать", None, Vec2::new(0.0, 38.0), bold(13.0), 10).on_hover_text("Заменить ролик обрезанным").clicked() { *act = Some((46, 0)); }
                if kit::button_ex(ui, Kind::Ghost, "Сохранить как новый", Some(Icon::Folder), Vec2::new(0.0, 38.0), semibold(13.0), 10).on_hover_text("Оригинал останется").clicked() { *act = Some((47, 0)); }
            });
        });
    }
    // keyboard: space — pause/play, arrows — move the cursor
    let (space, left, right, shift) = ui.input(|i| (i.key_pressed(egui::Key::Space), i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight), i.modifiers.shift));
    let typing = ui.ctx().memory(|m| m.focused().is_some());
    if !typing {
        if space { *act = Some(if playing { (39, 0) } else { (26, 0) }); }
        let step = if shift { 1.0 } else { 0.1 };
        if left || right {
            d.cursor = (d.cursor + if right { step } else { -step }).clamp(0.0, dur);
            if d.cursor < d.view.0 || d.cursor > d.view.1 { let w = d.view.1 - d.view.0; let a = (d.cursor - w / 2.0).clamp(0.0, (dur - w).max(0.0)); d.view = (a, a + w); }
            if !playing { *act = Some((27, 0)); }
        }
    }
}


/// «Окно голоса»: the original's melody of the line (amber), what you sing (pink),
/// and a hint how far you are from its pitch.
fn voice_panel(p: &egui::Painter, r: Rect, d: &Dub) {
    wave::frame(p, r);
    let Some((_, from, c)) = &d.refc else { wave::frame_border(p, r); return };
    let (a, b) = d.span(d.cur);
    let len = (b - a).max(0.5);
    let area = r.shrink2(Vec2::new(8.0, 6.0));
    let x = |t: f32| area.left() + area.width() * ((t - a) / len).clamp(0.0, 1.0);
    // the original: mirrored loudness, dark outline + bright core
    wave::draw_env(p, area, &d.refenv, 0.0, len, wave::W_OUTER, wave::W_INNER);
    // where the line itself is
    let l = &d.lines[d.cur];
    for t in [l.start, l.end] { p.line_segment([Pos2::new(x(t), area.top()), Pos2::new(x(t), area.top() + 8.0)], Stroke::new(2.0_f32, Color32::WHITE)); }
    // your take, over it
    let h = area.height() / 2.0 - 4.0;
    let mx = d.live_lvl.iter().map(|v| v.1).fold(0.05f32, f32::max);
    for w in d.live_lvl.windows(2) {
        let lv = (w[1].1 / mx).min(1.0);
        let xx = x(w[1].0);
        p.line_segment([Pos2::new(xx, area.center().y - lv * h), Pos2::new(xx, area.center().y + lv * h)], Stroke::new(2.0_f32, wave::W_YOU.gamma_multiply(0.75)));
    }
    // pitch: a thin line along the bottom (original orange, yours cyan)
    let band = Rect::from_min_max(Pos2::new(area.left(), area.bottom() - 22.0), area.max);
    let center = median_note(c).unwrap_or(55.0);
    let y = |m: f32| band.bottom() - band.height() * ((m - center + 9.0) / 18.0).clamp(0.0, 1.0);
    let step = 0.02;
    for (i, w) in c.windows(2).enumerate() {
        if let (Some(m0), Some(m1)) = (w[0], w[1]) {
            let t0 = from + i as f32 * step;
            if (m1 - m0).abs() < 3.0 { p.line_segment([Pos2::new(x(t0), y(m0)), Pos2::new(x(t0 + step), y(m1))], Stroke::new(1.6_f32, wave::W_PITCH)); }
        }
    }
    for w in d.live.windows(2) {
        // compare in the original's octave
        let fold = |m: f32| m - ((m - center) / 12.0).round() * 12.0;
        if w[1].0 - w[0].0 < 0.15 { p.line_segment([Pos2::new(x(w[0].0), y(fold(w[0].1))), Pos2::new(x(w[1].0), y(fold(w[1].1)))], Stroke::new(1.6_f32, wave::W_YOU)); }
    }
    // playhead
    if let Some((t0, f)) = d.play_from {
        let tx = x(f + t0.elapsed().as_secs_f32());
        p.line_segment([Pos2::new(tx, r.top() + 3.0), Pos2::new(tx, r.bottom() - 3.0)], Stroke::new(2.5_f32, wave::W_HEAD));
    }
    wave::frame_border(p, r);
}

/// One line under the voice window: the original's note and how far you are from it.
fn voice_hint(d: &Dub) -> (String, Color32) {
    let orig = d.refc.as_ref().and_then(|(_, _, c)| median_note(c));
    let now = d.mon.as_ref().and_then(|m| m.midi());
    match (orig, now) {
        (Some(o), Some(n)) => {
            let mut dl = o - n;
            let oct = dl.abs() > 9.0;
            if oct { dl -= (dl / 12.0).round() * 12.0; }
            let what = if dl.abs() < 1.0 { "в тон!".to_string() } else if dl > 0.0 { format!("выше на {:.0} полут.", dl) } else { format!("ниже на {:.0} полут.", -dl) };
            let col = if dl.abs() < 1.0 { OK_TEXT() } else if dl.abs() < 3.0 { AMBER() } else { ACCENT_TEXT() };
            (format!("Оригинал: {} · ты: {} — {what}{}", note_name(o), note_name(n), if oct { " (через октаву)" } else { "" }), col)
        }
        (Some(o), None) => (format!("Оригинал: {} — скажи что-нибудь, подберём тон", note_name(o)), MUTED()),
        _ => ("Оранжевая линия — интонация оригинала, голубая — твоя".to_string(), MUTED()),
    }
}

fn scene_name(p: &Path) -> String { p.file_stem().map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default() }

#[cfg(windows)]
fn pick_scene() -> Vec<PathBuf> {
    let mut all: Vec<&str> = EXT_VIDEO.to_vec();
    all.extend(EXT_AUDIO);
    rfd::FileDialog::new().set_title("Ролик для озвучки").add_filter("Видео и звук", &all).pick_files().unwrap_or_default()
}
#[cfg(not(windows))]
fn pick_scene() -> Vec<PathBuf> { vec![] }

/* ---------- the screen ---------- */

impl App {
    pub(crate) fn open_dub(&mut self) {
        self.cancel();
        self.mic_test = None;
        match &mut self.dub { Some(d) => { d.scan(); d.phase = DPhase::Setup; } None => self.dub = Some(Dub::new()) }
        self.show_dub = true;
    }

    pub(crate) fn exit_dub(&mut self) {
        self.show_dub = false;
        self.game_player = None;
        self.close_video = true;
        if let Some(d) = &mut self.dub { d.mic = None; d.mon = None; d.rec_since = None; d.phase = DPhase::Setup; }
    }

    /// Runs JS in the clip page (the page defines kj(t, play)).
    fn dub_js(&self, js: &str) {
        #[cfg(windows)]
        if let Some(v) = &self.video { let _ = v.wv.evaluate_script(js); }
        #[cfg(not(windows))]
        let _ = js;
    }
    fn dub_video_at(&mut self, t: f32, play: bool) {
        self.dub_js(&format!("window.kj && kj({:.3}, {});", t.max(0.0), play));
        if let Some(d) = &mut self.dub { d.sync_until = play.then(|| Instant::now() + Duration::from_millis(2500)); d.last_sync = Instant::now(); }
    }

    fn dub_add(&mut self, paths: Vec<PathBuf>) {
        let Some(d) = &mut self.dub else { return };
        let (mut n, mut bad) = (0, None);
        let mut first: Option<PathBuf> = None;
        let mut moments = 0;
        for p in paths {
            // a moments file: goes to the clip with the same name
            if is_moments_file(&p) {
                let key = base_key(&p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                let target = d.scenes.iter().find(|s| base_key(&s.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()) == key).cloned();
                match (target, std::fs::read_to_string(&p).ok().and_then(|t| parse_lines(&t))) {
                    (Some(sc), Some(v)) => { save_lines(&sc, &v); moments += 1; }
                    (None, _) => bad = Some(format!("{} — нет ролика с таким названием", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())),
                    (_, None) => bad = Some(format!("{} — не файл моментов", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())),
                }
                continue;
            }
            if !is_scene(&p) { bad = Some(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()); continue; }
            let name = p.file_name().unwrap_or_default();
            // into the category that is open (or «Озвучка» itself)
            let dir = match &d.cat { Some(c) if !c.is_empty() => d.root.join(c), _ => d.root.clone() };
            let mut dst = dir.join(name);
            let mut k = 2;
            while dst.exists() { dst = dir.join(format!("{} ({k}).{}", scene_name(&p), p.extension().and_then(|e| e.to_str()).unwrap_or("mp4"))); k += 1; }
            if std::fs::copy(&p, &dst).is_ok() {
                n += 1;
                // the clip's moments, if they lie next to it, come along
                if let Some(v) = find_lines(&p).and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| parse_lines(&t)) { save_lines(&dst, &v); }
                if first.is_none() { first = Some(dst); }
            }
        }
        d.scan();
        if moments > 0 { self.say(format!("Моменты перенесены: {moments}")); }
        match (n, bad) {
            (0, Some(b)) if moments == 0 => self.say(format!("«{b}» — не ролик (нужен mp4, mov, webm или mp3/wav)")),
            (n, _) if n > 0 => self.say(format!("Добавлено роликов: {n}. Проверьте моменты в редакторе.")),
            _ => {}
        }
        // a new clip opens in the editor right away
        if let Some(p) = first { self.dub_open_scene(p, false); }
    }

    /// Load a clip; `play` — start dubbing at once, else open the editor.
    fn dub_open_scene(&mut self, p: PathBuf, play: bool) {
        let Some(d) = &mut self.dub else { return };
        d.loading = true;
        match audio::load_mid_side(&p, MAX_SECS) {
            Ok((s, side, sr)) => {
                d.cut_mode = false; d.lines_stash.clear(); d.preview = None;
                d.orig = s; d.side = side; d.sr = sr;
                d.fullenv = wave::envelope(&d.orig, d.sr, ((d.orig.len() as f32 / sr as f32) * 100.0) as usize + 1);
                d.peak_max = d.orig.iter().fold(1e-4f32, |m, v| m.max(v.abs()));
                d.sel = None;
                d.view = (0.0, d.dur());
                d.cursor = 0.0;
                d.video = is_video(&p);
                d.scene = Some(p.clone());
                d.takes.clear();
                upgrade_old_trim(&p);
                d.clean = crate::separate::load_cached(&p, d.orig.len(), d.sr);
                let mut fresh = false;
                match load_lines(&p) { Some(v) => d.lines = v.into_iter().filter(|l| l.start < d.dur()).map(|l| Line { end: l.end.min(d.dur()), ..l }).collect(), None => { d.resplit(); fresh = true; } }
                if d.lines.is_empty() { d.resplit(); }
                d.cur = 0;
                d.phase = DPhase::Edit;
                d.loading = false;
                let long = d.dur() >= MAX_SECS - 0.5;
                if d.video { self.pending_video = Some(file_url(&p)); }
                if long { self.say("Ролик длинный — взял первые 3 минуты"); }
                let sep = self.dub.as_ref().map(|d| d.clean.is_none()).unwrap_or(false) && crate::separate::available();
                if play { self.dub_begin(); } else { self.boris.queue("Редактор! Подвиньте моменты, если где-то лишнее.".into()); }
                if sep { self.dub_listen(fresh); }
            }
            Err(e) => { d.loading = false; self.say(format!("Не получилось прочитать звук ролика: {e}")); }
        }
    }

    /// The neural net takes the voices out of the open clip's sound, in the background (once,
    /// kept next to the clip). `fresh` — the moments are the automatic ones: cut them again on
    /// the voices alone when it's done.
    fn dub_listen(&mut self, fresh: bool) {
        let Some(d) = &mut self.dub else { return };
        if d.listen.is_some() || d.clean.is_some() || !crate::separate::available() { return; }
        let Some(scene) = d.scene.clone() else { return };
        let (x, side, sr) = (d.orig.clone(), d.side.clone(), d.sr);
        let stage = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let st = stage.clone();
        let sc = scene.clone();
        let job = std::thread::spawn(move || {
            use std::sync::atomic::Ordering;
            let bg = crate::separate::background(&x, &side, sr, &|p| st.store(((p * 999.0) as u32).min(999), Ordering::Relaxed))?;
            crate::separate::save_cache(&sc, &bg, sr);
            let voice: Vec<f32> = x.iter().zip(&bg).map(|(a, b)| a - b).collect();
            let lines = segment(&voice, sr, SENS).into_iter().map(|(a, b)| Line::new(a, b)).collect();
            Ok((bg, lines))
        });
        d.listen = Some(Listen { scene, stage, before: fresh.then(|| d.lines.clone()), job });
        self.boris.queue("Убираю голоса из ролика — фон будет чище!".into());
    }

    /// Play the original of a line: the video (silent) + our own copy of its sound.
    fn dub_preview(&mut self, i: usize) {
        let Some(d) = &mut self.dub else { return };
        if i >= d.lines.len() { return; }
        let (s, e) = d.span(i);
        let piece = d.piece(s, e);
        let sr = d.sr;
        d.stop_at = Some(Instant::now() + Duration::from_secs_f32(e - s + 0.1));
        d.play_from = Some((Instant::now(), s));
        let video = d.video;
        self.game_player = audio::Player::from_samples(piece, sr, &None, 1.0).ok();
        if video { self.dub_video_at(s, true); }
    }

    /// Make line `i` current and play its original right away.
    fn dub_goto(&mut self, i: usize) {
        let n = self.dub.as_ref().map(|d| d.lines.len()).unwrap_or(0);
        if let Some(d) = &mut self.dub { d.cur = i.min(n); d.intro = false; }
        if i < n { self.dub_preview(i); }
    }

    /// The whole clip with its own sound.
    fn dub_play_all(&mut self) {
        let Some(d) = &mut self.dub else { return };
        let (s, sr, video) = (d.orig.clone(), d.sr, d.video);
        d.stop_at = Some(Instant::now() + Duration::from_secs_f32(d.dur() + 0.2));
        d.play_from = Some((Instant::now(), 0.0));
        self.game_player = audio::Player::from_samples(s, sr, &None, 1.0).ok();
        if video { self.dub_video_at(0.0, true); }
    }

    fn dub_stop_play(&mut self) {
        self.game_player = None;
        if let Some(d) = &mut self.dub { d.stop_at = None; d.play_from = None; d.preview = None; }
        self.dub_js("window.kj && kj(-1, false);");
    }

    fn dub_start_rec(&mut self) {
        let Some(d) = &mut self.dub else { return };
        if d.cur >= d.lines.len() { return; }
        if d.mic.is_none() {
            match audio::Recorder::mic(&self.settings.input, self.params.clone()) { Ok(m) => d.mic = Some(m), Err(e) => { self.say(e); return; } }
        }
        let (at, stop) = d.rec_window(d.cur);
        if let Some(st) = d.cleaning() { self.say(st); return; }
        // the background under the take: with the neural net's voiceless sound the whole window
        // (the lead-in second too) is free of voices; otherwise the voices are cut in the moments
        let bg = if d.rec_bg {
            let full = d.clean.clone().filter(|_| d.devoice).unwrap_or_else(|| d.background());
            let a = ((at * d.sr as f32) as usize).min(full.len());
            let b = (((stop + 0.3) * d.sr as f32) as usize).min(full.len()).max(a);
            Some(full[a..b].iter().map(|v| v * 0.7).collect::<Vec<f32>>())
        } else { None };
        if let Some(m) = &d.mic { m.begin(); }
        d.rec_since = Some(Instant::now());
        d.stop_at = None;
        d.live.clear();
        d.live_lvl.clear();
        d.play_from = Some((Instant::now(), at));
        d.rec_ref = bg.clone();
        let (video, sr) = (d.video, d.sr);
        self.game_player = bg.and_then(|b| audio::Player::from_samples(b, sr, &None, 1.0).ok());
        if video { self.dub_video_at(at, true); }
    }

    /// Save the pieces of the clip for real: over the file or as a new one.
    fn dub_save_cut(&mut self, over: bool) {
        self.dub_stop_play();
        let Some(d) = &mut self.dub else { return };
        let Some(scene) = d.scene.clone() else { return };
        let mut pieces: Vec<(f32, f32)> = d.lines.iter().map(|l| (l.start, l.end)).collect();
        pieces.sort_by(|a, b| a.0.total_cmp(&b.0));
        let ext = scene.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_lowercase();
        let new_path = |ext: &str| {
            let base = format!("{} (обрезка)", scene_name(&scene));
            let dir = scene.parent().map(|x| x.to_path_buf()).unwrap_or_else(|| d.root.clone());
            let mut dst = dir.join(format!("{base}.{ext}"));
            let mut k = 2;
            while dst.exists() { dst = dir.join(format!("{base} {k}.{ext}")); k += 1; }
            dst
        };
        // exact cuts are always .mp4 (H.264 + AAC)
        let vext = if matches!(ext.as_str(), "mp4" | "m4v" | "mov") { ext.clone() } else { "mp4".to_string() };
        let mut exact = true;
        let res: Result<PathBuf, String> = if d.video {
            let dst = if over { scene.with_file_name(format!(".cut-{}.{vext}", std::process::id())) } else { new_path(&vext) };
            // keep the picture as good as it was: the source's bitrate, a bit more
            let kbps = std::fs::metadata(&scene).map(|m| (m.len() as f32 * 8.0 / d.dur().max(1.0) / 1000.0 * 1.3) as u32).unwrap_or(6000);
            let target = if over { scene.with_extension(&vext) } else { dst.clone() };
            match crate::ff::cut_pieces(&scene, &dst, &pieces, kbps) {
                Ok(how) if over => {
                    exact = how == crate::ff::Cut::Exact;
                    // the player must let go of the file before it is replaced
                    #[cfg(windows)]
                    { self.video = None; }
                    let mut r = Err(String::new());
                    for _ in 0..30 {
                        r = std::fs::rename(&dst, &target).map_err(|e| format!("Не удалось заменить ролик: {e}"));
                        if r.is_ok() { break; }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    if r.is_err() { let _ = std::fs::remove_file(&dst); }
                    // a .webm / .mkv became an .mp4: the old file goes
                    if r.is_ok() && target != scene { let _ = std::fs::remove_file(&scene); }
                    r.map(|_| target.clone())
                }
                Ok(how) => { exact = how == crate::ff::Cut::Exact; Ok(dst) }
                Err(e) => { let _ = std::fs::remove_file(&dst); Err(e) }
            }
        } else {
            // a sound scene: glue the pieces, write WAV
            let parts: Vec<Vec<f32>> = pieces.iter().map(|&(a, b)| d.piece(a, b)).collect();
            let x = crate::apozh::glue(&parts, d.sr);
            let dst = if over && ext == "wav" { scene.clone() } else if over { scene.with_extension("wav") } else { new_path("wav") };
            audio::write_wav(&dst, &x, d.sr).map(|_| { if over && dst != scene { let _ = std::fs::remove_file(&scene); } dst })
        };
        let Some(d) = &mut self.dub else { return };
        match res {
            Ok(p) => {
                // the moments move with the picture: each keeps its place inside its piece
                let moved = move_moments(&d.lines_stash, &pieces);
                // the voiceless sound is cut by the very same pieces (sample-exact, same length)
                let clean = d.clean.as_ref().filter(|_| exact).map(|c| {
                    let parts: Vec<Vec<f32>> = pieces.iter().map(|&(a, b)| slice(c, d.sr, a, b)).collect();
                    if d.video { parts.concat() } else { crate::apozh::glue(&parts, d.sr) }
                });
                d.cut_mode = false; d.lines_stash.clear(); d.preview = None;
                if over {
                    let _ = std::fs::remove_file(lines_path(&scene));
                    let _ = std::fs::remove_file(crate::separate::cache_path(&scene));
                }
                let _ = std::fs::remove_file(crate::separate::cache_path(&p));
                save_lines(&p, &moved);
                if let Some(c) = clean { crate::separate::save_cache(&p, &c, d.sr); }
                d.scan();
                let how = if exact { "" } else { " (по ключевым кадрам: в этой Windows нет кодировщика H.264 — края могут сдвинуться на долю секунды)" };
                self.say(if over { format!("Ролик обрезан насовсем{how}") } else { format!("Сохранено как «{}»{how}", scene_name(&p)) });
                self.dub_open_scene(p, false);
            }
            Err(e) => self.say(e),
        }
    }

    /// Start dubbing the loaded clip: whole clip first (fresh start), then moment by moment.
    fn dub_begin(&mut self) {
        let players = roster(&self.settings);
        let Some(d) = &mut self.dub else { return };
        if d.lines.is_empty() { self.say("В ролике нет моментов — нажмите ✂ и отметьте их на волне"); return; }
        // other players' versions of this clip stay for comparison, if the list is the same
        if d.players != players || d.takes.iter().any(|v| v.len() != d.lines.len()) { d.takes.clear(); }
        d.players = players;
        d.takes.resize(d.players.len(), vec![]);
        for v in &mut d.takes { v.resize(d.lines.len(), None); }
        d.who = d.next_who.min(d.players.len() - 1);
        let w = d.who;
        d.takes[w] = vec![None; d.lines.len()];
        d.phase = DPhase::Record; d.sel = None; d.cur = 0; d.intro = true;
        let name = d.players[w].clone();
        // first the whole clip with sound, then moment by moment
        self.dub_play_all();
        self.boris.queue(format!("Мотор! Озвучивает {name}. Сначала смотрим сцену целиком."));
    }

    /// Editor: play from the white cursor to the end.
    fn dub_act_play_from_cursor(&mut self) {
        let r = self.dub.as_mut().map(|d| { let a = d.cursor.min(d.dur() - 0.05).max(0.0); let b = d.dur(); d.stop_at = Some(Instant::now() + Duration::from_secs_f32(b - a + 0.05)); d.play_from = Some((Instant::now(), a)); (d.piece(a, b), d.sr, a, d.video) });
        if let Some((s, sr, a, video)) = r { self.game_player = audio::Player::from_samples(s, sr, &None, 1.0).ok(); if video { self.dub_video_at(a, true); } }
    }


    fn dub_stop_rec(&mut self) {
        let Some(d) = &mut self.dub else { return };
        let Some(_) = d.rec_since.take() else { return };
        d.play_from = None;
        let i = d.cur;
        let at = d.rec_window(i).0;
        let (mut s, sr) = d.mic.as_ref().map(|m| (m.end(), m.sr)).unwrap_or_default();
        // take the background we played out of the recording, so it is not doubled in the result
        if let Some(r) = d.rec_ref.take() { s = crate::cleanup::remove_known(&s, sr, &r, d.sr); }
        let (w, n) = (d.who, d.lines.len());
        if d.takes.len() <= w { d.takes.resize(w + 1, vec![]); }
        d.takes[w].resize(n, None);
        d.takes[w][i] = Some(Take { at, samples: s, sr });
        let video = d.video;
        let done = d.next_open(i) >= d.lines.len();
        self.game_player = None;
        let Some(d) = &mut self.dub else { return };
        let name = d.players.get(d.who).cloned().unwrap_or_default();
        if video { self.dub_js("window.kj && kj(-1, false);"); }
        if done { self.boris.queue(format!("{name} озвучил весь ролик! Жмите «Смотреть результат».")); }
    }

    /// Play player `p`'s version from the start.
    fn dub_show(&mut self, p: usize) {
        let Some(d) = &mut self.dub else { return };
        d.who = p;
        let m = d.version(p);
        let sr = d.sr;
        d.phase = DPhase::Final;
        d.show_since = Some(Instant::now());
        d.stop_at = None;
        let video = d.video;
        self.game_player = audio::Player::from_samples(m, sr, &None, 1.0).ok();
        if video { self.dub_video_at(0.0, true); }
    }

    fn dub_tick(&mut self) {
        let mut pause = false;
        let mut stop_rec = false;
        if let Some(d) = &mut self.dub {
            if d.stop_at.map(|t| Instant::now() >= t).unwrap_or(false) { d.stop_at = None; d.play_from = None; pause = d.video; }
            if let (Some(_), Some((t0, from)), Some(m)) = (d.rec_since, d.play_from, d.mon.as_ref().and_then(|m| m.midi())) {
                d.live.push((from + t0.elapsed().as_secs_f32(), m));
            }
            if let (Some(_), Some((t0, from))) = (d.rec_since, d.play_from) {
                let lv = d.mic.as_ref().map(|m| m.level()).unwrap_or(0.0);
                d.live_lvl.push((from + t0.elapsed().as_secs_f32(), lv));
            }
            if let Some(t0) = d.rec_since.filter(|_| d.cur < d.lines.len()) {
                let (at, stop) = d.rec_window(d.cur);
                if t0.elapsed().as_secs_f32() > stop - at { stop_rec = true; }
            }
            if d.phase == DPhase::Final && d.show_since.map(|t| t.elapsed().as_secs_f32() > d.dur() + 0.5).unwrap_or(false) { d.show_since = None; pause = d.video; }
            if d.phase == DPhase::Setup && d.last_scan.elapsed().as_secs_f32() > 2.0 { d.scan(); }
        }
        let heard = self.dub.as_mut().and_then(|d| if d.listen.as_ref().map(|l| l.job.is_finished()).unwrap_or(false) { d.listen.take() } else { None });
        if let Some(l) = heard {
            let res = l.job.join().unwrap_or_else(|_| Err("сбой".into()));
            // the clip was deleted while the net worked on it
            if !l.scene.exists() { let _ = std::fs::remove_file(crate::separate::cache_path(&l.scene)); }
            match res {
                Ok((bg, found)) => {
                    let d = self.dub.as_mut().unwrap();
                    let open = d.scene.as_ref() == Some(&l.scene);
                    if open && bg.len() == d.orig.len() { d.clean = Some(bg); }
                    // automatic moments that nobody touched or recorded on: cut again on the voices
                    let auto = |cur: &[Line]| l.before.as_deref() == Some(cur) && !found.is_empty();
                    if open && !d.cut_mode && auto(&d.lines) && !d.any_takes() {
                        let n = found.len();
                        d.lines = found; d.sel = None; d.cur = 0; d.refc = None; d.save();
                        for v in &mut d.takes { *v = vec![None; n]; }
                        self.say(format!("Голоса убраны, моменты найдены заново по голосам: {n}"));
                    } else if !open && load_lines(&l.scene).map(|c| auto(&c)).unwrap_or(false) {
                        save_lines(&l.scene, &found);
                        self.say(format!("Голоса из «{}» убраны", scene_name(&l.scene)));
                    } else {
                        self.say("Голоса из ролика убраны — фон под записью будет чистым");
                    }
                }
                Err(e) => self.say(e),
            }
            // another clip was opened meanwhile: now it's its turn
            let next = self.dub.as_ref().map(|d| d.scene.is_some() && d.phase != DPhase::Setup && d.clean.is_none() && crate::separate::available()).unwrap_or(false);
            if next { self.dub_listen(false); }
        }
        let done = self.dub.as_mut().and_then(|d| if d.export.as_ref().map(|j| j.is_finished()).unwrap_or(false) { d.export.take() } else { None });
        if let Some(j) = done {
            match j.join().unwrap_or_else(|_| Err("сбой".into())) {
                Ok(p) => { self.say("Видео с озвучкой сохранено"); show_in_folder(&p); self.boris.queue("Премьера в записи! Можно отправлять друзьям.".into()); }
                Err(e) => self.say(format!("Не получилось собрать видео: {e}")),
            }
        }
        // the video follows our clock while it settles
        let nudge = self.dub.as_mut().and_then(|d| {
            let due = d.video && d.sync_until.map(|u| Instant::now() < u).unwrap_or(false) && d.last_sync.elapsed().as_millis() > 400;
            let at = d.play_from.map(|(t0, f)| f + t0.elapsed().as_secs_f32()).or_else(|| d.show_since.map(|t0| t0.elapsed().as_secs_f32()));
            if due { d.last_sync = Instant::now(); at } else { None }
        });
        if let Some(t) = nudge { self.dub_js(&format!("window.kj && kj({t:.3}, true);")); }
        let pv = self.dub.as_mut().and_then(|d| {
            if d.preview.is_none() { return None; }
            match d.preview_pos() {
                None => { d.preview = None; Some(None) }
                Some((t, _)) if d.video && d.last_sync.elapsed().as_millis() > 200 => { d.last_sync = Instant::now(); Some(Some(t)) }
                _ => None,
            }
        });
        match pv {
            Some(Some(t)) => self.dub_js(&format!("window.kj && kj({t:.3}, true);")),
            Some(None) => self.dub_js("window.kj && kj(-1, false);"),
            None => {}
        }
        let intro_over = self.dub.as_ref().map(|d| d.phase == DPhase::Record && d.intro && d.play_from.is_none()).unwrap_or(false);
        if pause { self.dub_js("window.kj && kj(-1, false);"); }
        if stop_rec { self.dub_stop_rec(); }
        if intro_over {
            let c = self.dub.as_ref().map(|d| d.cur).unwrap_or(0);
            self.boris.queue("Посмотрели? Теперь по моментам. Слушаем первый!".into());
            self.dub_goto(c);
        }
    }

    pub(crate) fn header_dub(&mut self, ui: &mut egui::Ui) {
        let sub = self.dub.as_ref().map(|d| match d.phase {
            DPhase::Setup => "мини-игра".to_string(),
            _ => d.scene.as_ref().map(|p| scene_name(p)).unwrap_or_default(),
        }).unwrap_or_default();
        ui.horizontal_centered(|ui| {
            if kit::ghost(ui, "Назад", Some(Icon::Back)).clicked() {
                let ph = self.dub.as_ref().map(|d| d.phase).unwrap_or(DPhase::Setup);
                if ph == DPhase::Setup { self.exit_dub(); } else {
                    self.game_player = None;
                    self.dub_js("window.kj && kj(-1, false);");
                    if let Some(d) = &mut self.dub { d.rec_since = None; d.mic = None; d.intro = false; if d.cut_mode { d.lines = std::mem::take(&mut d.lines_stash); d.cut_mode = false; } d.preview = None; d.phase = DPhase::Setup; d.scan(); }
                    self.close_video = true;
                }
                return;
            }
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new("Озвучка").font(display_bold(18.0)));
                ui.label(RichText::new(sub).font(body(13.0)).color(MUTED()));
            });
        });
    }

    pub(crate) fn dub_screen(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.video_rect = None;
        let (input, params) = (self.settings.input.clone(), self.params.clone());
        if let Some(d) = &mut self.dub {
            if d.phase == DPhase::Record { if d.mon.is_none() { d.mon = audio::PitchMonitor::start(&input, params).ok(); } } else { d.mon = None; }
        }
        self.dub_tick();
        ctx.request_repaint_after(Duration::from_millis(40));
        match self.dub.as_ref().map(|d| d.phase).unwrap_or(DPhase::Setup) {
            DPhase::Setup => {
                let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
                if !dropped.is_empty() { self.dub_add(dropped); }
                self.dub_setup(ui, ctx);
            }
            ph => {
                if ph == DPhase::Edit {
                    let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
                    let got = dropped.iter().filter(|p| is_moments_file(p)).find_map(|p| std::fs::read_to_string(p).ok().and_then(|t| parse_lines(&t)));
                    if let (Some(v), Some(d)) = (got, self.dub.as_mut()) {
                        let dur = d.dur();
                        d.lines = v.into_iter().filter(|l| l.start < dur).map(|l| Line { end: l.end.min(dur), ..l }).collect();
                        d.takes.clear(); d.sel = None; d.save();
                        let n = d.lines.len();
                        self.say(format!("Моменты из файла: {n}"));
                    }
                }
                self.dub_work(ui, ph)
            }
        }
    }

    fn dub_setup(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut open_scene = None;
        let mut add = false;
        let mut del: Option<PathBuf> = None;
        let mut mv: Option<(PathBuf, String)> = None;
        let mut folder = false;
        let mut roster_changed = false;
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| ui.vertical_centered(|ui| {
            ui.add_space(16.0);
            ui.allocate_ui_with_layout(Vec2::new(720.0, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.label(RichText::new("Переозвучьте сцену").font(display(40.0)));
                ui.add_space(6.0);
                kit::text(ui, "Возьмите короткую сцену — программа разрежет её на реплики. Каждый игрок по очереди озвучивает ролик целиком под фон без голосов, а в конце вы смотрите все версии и сохраняете лучшую как видео.", body(16.0), TEXT_2(), 720.0);
                ui.add_space(16.0);
                let d = self.dub.as_mut().unwrap();
                egui::Frame::new().fill(if hovering { SELECTED() } else { SURFACE() }).stroke(Stroke::new(1.0_f32, if hovering { ACCENT() } else { BORDER() })).corner_radius(20.0).inner_margin(24.0).show(ui, |ui| {
                    ui.set_width(672.0);
                    ui.horizontal(|ui| {
                        kit::section_label(ui, "РОЛИКИ");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if kit::link(ui, "Открыть папку").clicked() { folder = true; } });
                    });
                    ui.add_space(4.0);
                    kit::text(ui, "Лучше всего — 20–90 секунд с диалогом. Перетащите mp4 в окно или нажмите «Добавить ролик».", body(13.0), MUTED(), 672.0);
                    ui.add_space(8.0);
                    // who dubs this time
                    let names = roster(&self.settings);
                    if d.next_who >= names.len() { d.next_who = 0; }
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                        ui.label(RichText::new("Озвучивает:").font(semibold(14.0)).color(MUTED()));
                        for (i, n) in names.iter().enumerate() {
                            let on = i == d.next_who;
                            let t = RichText::new(n).font(bold(14.0)).color(if on { Color32::BLACK } else { TEXT_2() });
                            if ui.add(egui::Button::new(t).fill(if on { player_color(i) } else { CHIP() }).corner_radius(12.0).min_size(Vec2::new(0.0, 34.0))).clicked() { d.next_who = i; }
                        }
                    });
                    ui.add_space(8.0);
                    // categories: chips; the list shows only the chosen one
                    let has_loose = d.scenes.iter().any(|p| cat_of(&d.root, p).is_empty());
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                        let chip = |ui: &mut egui::Ui, label: &str, on: bool| {
                            let t = RichText::new(label).font(semibold(13.0)).color(if on { ON_ACCENT() } else { TEXT_2() });
                            ui.add(egui::Button::new(t).fill(if on { ACCENT() } else { CHIP() }).corner_radius(10.0).min_size(Vec2::new(0.0, 30.0))).clicked()
                        };
                        if chip(ui, &format!("Все · {}", d.scenes.len()), d.cat.is_none()) { d.cat = None; }
                        for c in d.cats.clone() {
                            let n = d.scenes.iter().filter(|p| cat_of(&d.root, p) == c).count();
                            if chip(ui, &format!("{c} · {n}"), d.cat.as_deref() == Some(c.as_str())) { d.cat = Some(c); }
                        }
                        if has_loose && chip(ui, "Без категории", d.cat.as_deref() == Some("")) { d.cat = Some(String::new()); }
                        match &mut d.new_cat {
                            None => { if kit::link(ui, "+ категория").clicked() { d.new_cat = Some(String::new()); } }
                            Some(name) => {
                                let r = ui.add(egui::TextEdit::singleline(name).hint_text("название").desired_width(140.0).font(body(13.0)));
                                if name.is_empty() && !r.has_focus() && !r.lost_focus() { r.request_focus(); }
                                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                let clean: String = name.trim().chars().filter(|c| !"\\/:*?\"<>|".contains(*c)).collect();
                                if enter && !clean.is_empty() && clean != DONE {
                                    let _ = std::fs::create_dir_all(d.root.join(&clean));
                                    d.cat = Some(clean); d.new_cat = None; d.scan();
                                } else if r.lost_focus() { d.new_cat = None; }
                            }
                        }
                    });
                    ui.add_space(8.0);
                    let shown: Vec<PathBuf> = d.scenes.iter().filter(|p| d.cat.as_ref().map(|c| &cat_of(&d.root, p) == c).unwrap_or(true)).cloned().collect();
                    // the quickest way in: a random clip from what is shown
                    if !shown.is_empty() {
                        let lab = match d.cat.as_deref() { None => "Случайный ролик".to_string(), Some("") => "Случайный ролик без категории".to_string(), Some(c) => format!("Случайный ролик: {c}") };
                        if kit::button_ex(ui, Kind::Primary, &lab, Some(Icon::Play), Vec2::new(672.0, 52.0), bold(17.0), 14).on_hover_text("Программа сама выберет, что озвучивать").clicked() {
                            let avoid = d.last_pick.as_ref().and_then(|l| shown.iter().position(|p| p == l));
                            if let Some(k) = random_pick(shown.len(), avoid) { open_scene = Some((shown[k].clone(), true)); }
                        }
                        ui.add_space(10.0);
                    }
                    if shown.is_empty() { kit::text(ui, if d.scenes.is_empty() { "Пока пусто." } else { "В этой категории пока пусто — добавьте ролик, он попадёт сюда." }, body(14.0), MUTED(), 672.0); }
                    for p in &shown {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.label(RichText::new(if is_video(p) { "▶" } else { "♪" }).font(bold(16.0)).color(ACCENT_TEXT()));
                            // the name gets the space left by the buttons; a long one ends with «…»
                            let name_w = (ui.available_width() - 250.0).max(120.0);
                            ui.allocate_ui_with_layout(Vec2::new(name_w, 44.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.set_width(name_w);
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.add(egui::Label::new(RichText::new(scene_name(p)).font(semibold(15.0))).truncate()).on_hover_text(scene_name(p));
                                let st = if d.listen.as_ref().map(|l| &l.scene == p).unwrap_or(false) { "нейросеть работает…" }
                                    else { d.status.get(p).copied().unwrap_or("") };
                                let c = cat_of(&d.root, p);
                                let st = if d.cat.is_none() && !c.is_empty() { format!("{c} · {st}") } else { st.to_string() };
                                ui.label(RichText::new(st).font(body(12.0)).color(MUTED()));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                if kit::icon_button(ui, Icon::Trash, 36.0).on_hover_text("Удалить ролик").clicked() { del = Some(p.clone()); }
                                let here = cat_of(&d.root, p);
                                ui.menu_button(RichText::new("•••").font(bold(12.0)).color(TEXT_2()), |ui| {
                                    ui.label(RichText::new("Перенести в категорию").font(semibold(12.0)).color(MUTED()));
                                    for c in d.cats.iter().chain(std::iter::once(&String::new())) {
                                        let name = if c.is_empty() { "Без категории" } else { c.as_str() };
                                        if ui.add_enabled(*c != here, egui::Button::new(name)).clicked() { mv = Some((p.clone(), c.clone())); ui.close_menu(); }
                                    }
                                }).response.on_hover_text("Категория");
                                if kit::button_ex(ui, Kind::Primary, "Озвучивать", Some(Icon::Mic), Vec2::new(0.0, 36.0), bold(13.0), 10).clicked() { open_scene = Some((p.clone(), true)); }
                                let cut = ui.add(egui::Button::new(RichText::new("✂").font(bold(16.0)).color(TEXT())).fill(CHIP()).corner_radius(10.0).min_size(Vec2::new(36.0, 36.0)));
                                if cut.on_hover_text("Редактировать: моменты, обрезка, предпросмотр").clicked() { open_scene = Some((p.clone(), false)); }
                            });
                        });
                        ui.add_space(4.0);
                    }
                    ui.add_space(8.0);
                    if kit::button_ex(ui, Kind::Ghost, "Добавить ролик", Some(Icon::Folder), Vec2::new(0.0, 42.0), semibold(14.0), 12).clicked() { add = true; }
                });
                ui.add_space(14.0);
                let players = &mut self.settings.players;
                egui::Frame::new().fill(SURFACE()).stroke(Stroke::new(1.0_f32, BORDER())).corner_radius(20.0).inner_margin(24.0).show(ui, |ui| {
                    ui.set_width(672.0);
                    kit::section_label(ui, "ИГРАЮТ СЕГОДНЯ");
                    ui.add_space(4.0);
                    kit::text(ui, "Каждый по очереди озвучит весь ролик, потом сравним версии.", body(13.0), MUTED(), 672.0);
                    ui.add_space(6.0);
                    roster_changed = roster_editor(ui, players, 672.0);
                });
            });
        }));
        if roster_changed { self.settings.save(); }
        if folder {
            let dir = self.dub.as_ref().and_then(|d| d.cat.as_ref().filter(|c| !c.is_empty()).map(|c| d.root.join(c))).unwrap_or_else(dub_root);
            open(dir);
        }
        if add { let f = pick_scene(); if !f.is_empty() { self.dub_add(f); } }
        if let Some(p) = del {
            let _ = std::fs::remove_file(&p);
            let _ = std::fs::remove_file(lines_path(&p));
            let _ = std::fs::remove_file(crate::separate::cache_path(&p));
            if let Some(d) = &mut self.dub { d.scan(); }
        }
        if let Some((p, c)) = mv {
            if let Some(d) = &mut self.dub {
                let dir = if c.is_empty() { d.root.clone() } else { d.root.join(&c) };
                if move_scene(&p, &dir).is_none() { self.say("Не удалось перенести ролик"); }
                if let Some(d) = &mut self.dub { d.scan(); }
            }
        }
        if let Some((p, play)) = open_scene {
            if play { if let Some(d) = &mut self.dub { d.last_pick = Some(p.clone()); } }
            self.dub_open_scene(p, play);
        }
    }

    /// Lines / Record / Final share one layout: the clip on the left, lines on the right.
    fn dub_work(&mut self, ui: &mut egui::Ui, ph: DPhase) {
        let col_w = 420.0;
        let gap = 24.0;
        let avail = ui.available_size();
        let left_w = (avail.x - col_w - gap).max(360.0);
        let mut act: Option<(u8, usize)> = None;
        let t = ui.ctx().input(|i| i.time) as f32;
        if let Some(d) = &mut self.dub {
            if ph == DPhase::Record && d.cur < d.lines.len() && d.refc.as_ref().map(|r| r.0 != d.cur).unwrap_or(true) {
                let (a, b) = d.span(d.cur);
                let c = contour(&d.piece(a, b), d.sr);
                d.refenv = wave::envelope(&d.piece(a, b), d.sr, 500);
                d.refc = Some((d.cur, a, c));
                d.live.clear();
                d.live_lvl.clear();
            }
        }
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            // ---- left: the clip and the main controls ----
            ui.allocate_ui_with_layout(Vec2::new(left_w, avail.y), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(left_w);
                let d = self.dub.as_ref().unwrap();
                let voice_h = if ph == DPhase::Record && !d.intro { 150.0 } else if ph == DPhase::Edit { 230.0 } else { 0.0 };
                let vh = (left_w * 9.0 / 16.0).min(avail.y - 270.0 - voice_h).max(160.0);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(left_w, vh), egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, CornerRadius::same(18), if d.video { Color32::BLACK } else { SURFACE() });
                if d.video { self.video_rect = Some(rect.shrink(4.0)); } else {
                    let playing = self.game_player.as_ref().map(|p| !p.finished() && !p.paused()).unwrap_or(false) || d.rec_since.is_some();
                    for k in 0..32 {
                        let ph = k as f32 * 0.55;
                        let a = if playing { 0.25 + 0.75 * ((t * 7.0 + ph).sin() * 0.5 + 0.5) * ((t * 3.1 + ph * 1.7).sin() * 0.5 + 0.5) } else { 0.12 };
                        let bw = (rect.width() - 80.0) / 32.0;
                        let x = rect.left() + 40.0 + k as f32 * bw;
                        p.rect_filled(Rect::from_center_size(Pos2::new(x + bw / 2.0, rect.center().y), Vec2::new(bw * 0.55, ((vh - 60.0) * a).max(4.0))), CornerRadius::same(3), if playing { ACCENT() } else { BAR_OFF() });
                    }
                }
                ui.add_space(10.0);
                // timeline (record / final): every line as a coloured block, recorded ones solid
                if ph != DPhase::Edit {
                    let (tl, _) = ui.allocate_exact_size(Vec2::new(left_w, 26.0), egui::Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(tl, CornerRadius::same(6), SURFACE());
                    let dur = d.dur().max(0.1);
                    for (i, l) in d.lines.iter().enumerate() {
                        let x0 = tl.left() + tl.width() * l.start / dur;
                        let x1 = (tl.left() + tl.width() * l.end / dur).max(x0 + 3.0);
                        let r = Rect::from_min_max(Pos2::new(x0, tl.top() + 4.0), Pos2::new(x1, tl.bottom() - 4.0));
                        let c = pcol(d.who);
                        p.rect_filled(r, CornerRadius::same(3), if d.take(d.who, i).is_some() { c } else { c.gamma_multiply(0.3) });
                    }
                    // a running cursor whenever something plays: the premiere, the whole clip at the start, a moment
                    let now = d.show_since.map(|t0| t0.elapsed().as_secs_f32()).or_else(|| d.play_from.map(|(t0, f)| f + t0.elapsed().as_secs_f32()));
                    if let Some(tn) = now {
                        let x = tl.left() + tl.width() * (tn / dur).clamp(0.0, 1.0);
                        p.line_segment([Pos2::new(x, tl.top() - 2.0), Pos2::new(x, tl.bottom() + 2.0)], Stroke::new(2.5_f32, wave::W_HEAD));
                        p.circle_filled(Pos2::new(x, tl.top() - 2.0), 4.0, wave::W_HEAD);
                    }
                }
                if ph == DPhase::Record && d.cur < d.lines.len() && !d.intro {
                    // the prompter: what is said in this moment
                    let txt = d.lines[d.cur].text.trim();
                    if !txt.is_empty() {
                        ui.add_space(10.0);
                        egui::Frame::new().fill(SURFACE()).corner_radius(12.0).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
                            ui.set_width(left_w - 28.0);
                            ui.label(RichText::new(format!("«{txt}»")).font(bold(20.0)).color(TEXT()));
                        });
                    }
                    ui.add_space(10.0);
                    let (vr, _) = ui.allocate_exact_size(Vec2::new(left_w, 118.0), egui::Sense::hover());
                    voice_panel(ui.painter(), vr, d);
                    let (h, col) = voice_hint(d);
                    ui.label(RichText::new(h).font(semibold(13.0)).color(col));
                }
                // the moments editor: the whole clip, every moment on it
                if ph == DPhase::Edit {
                    let head = d.preview_pos().map(|p| p.0).or_else(|| d.play_from.map(|(t0, f)| f + t0.elapsed().as_secs_f32()));
                    if let Some(dm) = self.dub.as_mut() { moments_editor(ui, dm, left_w, head, &mut act); }
                }
                let d = self.dub.as_ref().unwrap();
                ui.add_space(14.0);
                ui.vertical_centered(|ui| {
                    let big = Vec2::new((left_w * 0.6).clamp(320.0, 520.0), 72.0);
                    match ph {
                        DPhase::Edit if d.cut_mode => {}
                        DPhase::Edit => {
                            ui.allocate_ui_with_layout(Vec2::new(500.0, 56.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 12.0;
                                if kit::button_ex(ui, Kind::Primary, "Готово", None, Vec2::new(240.0, 56.0), bold(18.0), 16).on_hover_text("Сохранить и вернуться к списку").clicked() { act = Some((32, 0)); }
                                if kit::button_ex(ui, Kind::Ghost, "Озвучивать", Some(Icon::Mic), Vec2::new(240.0, 56.0), semibold(16.0), 16).clicked() { act = Some((10, 0)); }
                            });
                            ui.add_space(6.0);
                            ui.label(RichText::new(format!("{} моментов. Всё сохраняется само.", d.lines.len())).font(body(14.0)).color(MUTED()));
                        }
                        DPhase::Record => {
                            let who = d.players.get(d.who).cloned().unwrap_or_default();
                            if d.cur < d.lines.len() {
                                if d.rec_since.is_some() {
                                    if kit::button_ex(ui, Kind::Primary, &format!("Стоп — {who}"), Some(Icon::Stop), big, bold(21.0), 20).clicked() { act = Some((12, 0)); }
                                    ui.add_space(8.0);
                                    ui.label(RichText::new("Говорите, когда на экране начнётся реплика. Остановится само.").font(body(14.0)).color(REC_TEXT()));
                                } else if d.intro {
                                    if kit::button_ex(ui, Kind::Primary, "Пропустить просмотр", Some(Icon::Play), big, bold(21.0), 20).clicked() { act = Some((22, 0)); }
                                    ui.add_space(8.0);
                                    ui.label(RichText::new("Сначала смотрим ролик целиком — со звуком. Потом пойдём по моментам.").font(body(14.0)).color(MUTED()));
                                } else if d.take(d.who, d.cur).is_some() {
                                    let more = d.next_open(d.cur) < d.lines.len();
                                    let (label, code) = if more { ("Следующий момент".to_string(), 23) } else { ("Смотреть результат".to_string(), 14) };
                                    if kit::button_ex(ui, Kind::Primary, &label, Some(if code == 14 { Icon::Play } else { Icon::Mic }), big, bold(21.0), 20).clicked() { act = Some((code, 0)); }
                                    ui.add_space(8.0);
                                    ui.label(RichText::new(format!("Момент {} из {} записан.", d.cur + 1, d.lines.len())).font(body(14.0)).color(OK_TEXT()));
                                    ui.add_space(10.0);
                                    ui.allocate_ui_with_layout(Vec2::new(620.0, 44.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                        ui.spacing_mut().item_spacing.x = 10.0;
                                        if kit::button_ex(ui, Kind::Ghost, "Перезаписать", Some(Icon::Repeat), Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some((11, 0)); }
                                        if kit::button_ex(ui, Kind::Ghost, "Моя запись", Some(Icon::Headphones), Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some((3, d.cur)); }
                                        if kit::button_ex(ui, Kind::Ghost, "Оригинал", Some(Icon::Play), Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some((2, d.cur)); }
                                    });
                                } else if let Some(st) = d.cleaning() {
                                    ui.add_enabled_ui(false, |ui| kit::button_ex(ui, Kind::Primary, "Нейросеть работает…", Some(Icon::Mic), big, bold(21.0), 20));
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        ui.add(egui::Spinner::new().size(16.0).color(wave::W_FRAME));
                                        ui.label(RichText::new(st).font(body(14.0)).color(MUTED()));
                                    });
                                    ui.ctx().request_repaint_after(Duration::from_millis(250));
                                } else {
                                    if kit::button_ex(ui, Kind::Primary, &format!("Записать — {who}"), Some(Icon::Mic), big, bold(21.0), 20).clicked() { act = Some((11, 0)); }
                                    ui.add_space(8.0);
                                    ui.label(RichText::new(format!("Момент {} из {}. Видео начнётся за секунду до него{}.", d.cur + 1, d.lines.len(), if !d.rec_bg { " — без звука" } else if d.clean.is_some() { " — с фоном ролика, без голосов" } else { " — с приглушённым звуком ролика" })).font(body(14.0)).color(MUTED()));
                                    let bg_lab = RichText::new(format!("{} Фон ролика при записи", if d.rec_bg { "☑" } else { "☐" })).font(body(13.0)).color(if d.rec_bg { TEXT_2() } else { MUTED() });
                                    if ui.add(egui::Button::new(bg_lab).frame(false)).on_hover_text("Музыка и звуки ролика играют, пока вы говорите. Потом программа сама вычитает их из записи. В наушниках — ещё чище.").clicked() { act = Some((42, 0)); }
                                    ui.add_space(10.0);
                                    ui.allocate_ui_with_layout(Vec2::new(620.0, 44.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                        ui.spacing_mut().item_spacing.x = 10.0;
                                        if kit::button_ex(ui, Kind::Ghost, "Послушать оригинал", Some(Icon::Play), Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some((2, d.cur)); }
                                        let busy = d.play_from.is_some();
                                        if kit::button_ex(ui, Kind::Ghost, if busy { "Остановить" } else { "Весь ролик" }, Some(if busy { Icon::Stop } else { Icon::Play }), Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some(if busy { (18, 0) } else { (17, 0) }); }
                                        if kit::button_ex(ui, Kind::Ghost, "Пропустить", None, Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some((13, 0)); }
                                    });
                                }
                            } else {
                                if kit::button_ex(ui, Kind::Primary, "Смотреть результат", Some(Icon::Play), big, bold(21.0), 20).clicked() { act = Some((14, 0)); }
                                ui.add_space(8.0);
                                ui.label(RichText::new("Все моменты записаны. Перезаписать любой можно справа.").font(body(14.0)).color(MUTED()));
                            }
                        }
                        DPhase::Final => {
                            let next = d.players.get((d.who + 1) % d.players.len().max(1)).cloned().unwrap_or_default();
                            ui.allocate_ui_with_layout(Vec2::new(620.0, 64.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                if kit::button_ex(ui, Kind::Primary, &format!("Следующий — {next}"), Some(Icon::Mic), Vec2::new(300.0, 60.0), bold(17.0), 16).on_hover_text("Этот же ролик, следующий игрок").clicked() { act = Some((43, 0)); }
                                if kit::button_ex(ui, Kind::Ghost, "Другой ролик", Some(Icon::Folder), Vec2::new(300.0, 60.0), semibold(16.0), 16).on_hover_text("К списку роликов").clicked() { act = Some((44, 0)); }
                            });
                            ui.add_space(10.0);
                            ui.allocate_ui_with_layout(Vec2::new(620.0, 44.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                if kit::button_ex(ui, Kind::Ghost, "Ещё раз", Some(Icon::Repeat), Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some((20, d.who)); }
                                ui.spacing_mut().item_spacing.x = 10.0;
                                if d.video {
                                    let busy = d.export.is_some();
                                    if kit::button_ex(ui, Kind::Ghost, if busy { "Собираю…" } else { "Сохранить видео" }, Some(Icon::Folder), Vec2::new(200.0, 42.0), semibold(14.0), 12).on_hover_text("Ролик с вашими голосами — mp4").clicked() && !busy { act = Some((31, 0)); }
                                } else if kit::button_ex(ui, Kind::Ghost, "Сохранить звук", Some(Icon::Folder), Vec2::new(200.0, 42.0), semibold(14.0), 12).clicked() { act = Some((15, 0)); }
                                if kit::button_ex(ui, Kind::Ghost, "Переозвучить", Some(Icon::Mic), Vec2::new(200.0, 42.0), semibold(14.0), 12).on_hover_text("Тот же игрок ещё раз").clicked() { act = Some((16, 0)); }
                            });
                        }
                        DPhase::Setup => {}
                    }
                });
            });
            // ---- right: lines / scores ----
            ui.allocate_ui_with_layout(Vec2::new(col_w, avail.y), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(col_w);
                let d = self.dub.as_mut().unwrap();
                if ph == DPhase::Final {
                    kit::card(ui, SURFACE(), |ui| {
                        ui.set_width(col_w - 40.0);
                        kit::section_label(ui, "ВЕРСИИ");
                        ui.add_space(6.0);
                        let rows: Vec<(usize, usize)> = (0..d.players.len()).map(|p| (p, d.dubbed(p))).filter(|r| r.1 > 0).collect();
                        for (p, n) in &rows {
                            let (r, _) = ui.allocate_exact_size(Vec2::new(col_w - 40.0, 50.0), egui::Sense::hover());
                            let pa = ui.painter();
                            if *p == d.who { pa.rect_filled(r.expand2(Vec2::new(8.0, 0.0)), CornerRadius::same(12), ACCENT_SOFT()); }
                            pa.circle_filled(Pos2::new(r.left() + 8.0, r.center().y), 6.0, pcol(*p));
                            pa.text(Pos2::new(r.left() + 24.0, r.center().y - 8.0), Align2::LEFT_CENTER, &d.players[*p], semibold(15.0), TEXT());
                            pa.text(Pos2::new(r.left() + 24.0, r.center().y + 10.0), Align2::LEFT_CENTER, format!("реплик: {n} из {}", d.lines.len()), body(12.0), MUTED());
                            let br = Rect::from_min_size(Pos2::new(r.right() - 120.0, r.center().y - 18.0), Vec2::new(120.0, 36.0));
                            let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(br).layout(egui::Layout::right_to_left(egui::Align::Center)));
                            if kit::button_ex(&mut cu, if *p == d.who { Kind::Primary } else { Kind::Ghost }, "Смотреть", Some(Icon::Play), Vec2::new(0.0, 36.0), bold(13.0), 10).clicked() { act = Some((20, *p)); }
                        }
                        ui.label(RichText::new("Чья версия смешнее — решайте сами!").font(body(12.0)).color(MUTED()));
                    });
                    ui.add_space(10.0);
                    let sub = if d.clean.is_some() { "Голоса убрала нейросеть — под озвучкой только музыка и звуки." } else { "Без дополнения «Нейросети» голоса оригинала только приглушаются." };
                    if kit::switch_row(ui, "Убирать голоса из ролика", sub, &mut d.devoice, true) { act = Some((20, d.who)); }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Звук ролика").font(body(13.0)).color(MUTED()));
                        for (lab, v) in [("выкл", 0.0f32), ("тихо", 0.35), ("громко", 0.8)] {
                            let on = (d.bg - v).abs() < 0.01;
                            if ui.add(egui::Button::new(RichText::new(lab).font(semibold(13.0)).color(if on { ON_ACCENT() } else { TEXT_2() })).fill(if on { ACCENT() } else { CHIP() }).corner_radius(8.0)).clicked() { d.bg = v; act = Some((20, d.who)); }
                        }
                    });
                    ui.add_space(8.0);
                    self.boris_below(ui, col_w);
                    return;
                }
                kit::card(ui, SURFACE(), |ui| {
                    ui.set_width(col_w - 40.0);
                    ui.horizontal(|ui| {
                        kit::section_label(ui, if ph == DPhase::Record { "РЕПЛИКИ" } else { "РЕПЛИКИ РОЛИКА" });
                        if ph == DPhase::Record {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let who = d.players.get(d.who).cloned().unwrap_or_default();
                                ui.label(RichText::new(format!("озвучивает {who}")).font(semibold(13.0)).color(pcol(d.who)));
                            });
                        }
                    });
                    if ph == DPhase::Edit {
                        if kit::link(ui, "Нарезать заново").on_hover_text("Найти моменты автоматически ещё раз (ручные правки пропадут)").clicked() { act = Some((6, 0)); }
                    }
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().max_height((avail.y - 120.0).max(200.0)).auto_shrink([false, true]).show(ui, |ui| {
                        for (i, l) in d.lines.iter().enumerate() {
                            let tk = d.take(d.who, i).is_some() && ph == DPhase::Record;
                            let cur = (ph == DPhase::Record && i == d.cur) || (ph == DPhase::Edit && d.sel == Some(i));
                            egui::Frame::new().fill(if cur { ACCENT_SOFT() } else { Color32::TRANSPARENT }).corner_radius(10.0).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    let (r, _) = ui.allocate_exact_size(Vec2::new(26.0, 26.0), egui::Sense::hover());
                                    let col = if ph == DPhase::Edit { AMBER() } else if tk { pcol(d.who) } else { CHIP() };
                                    ui.painter().circle_filled(r.center(), 11.0, col);
                                    ui.painter().text(r.center(), Align2::CENTER_CENTER, format!("{}", i + 1), bold(11.0), if col == CHIP() { TEXT_2() } else { Color32::BLACK });
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 0.0;
                                        let lab = ui.add(egui::Label::new(RichText::new(format!("{}–{}", fmt_time(l.start), fmt_time(l.end))).font(semibold(13.0)).color(if d.sel == Some(i) { ACCENT_TEXT() } else { TEXT() })).sense(egui::Sense::click()));
                                        if ph == DPhase::Edit && lab.on_hover_text("Изменить края момента").clicked() { act = Some((24, i)); }
                                        let secs = format!("{:.1} с", l.end - l.start).replace('.', ",");
                                        let st = if tk { "записано".to_string() } else if l.text.trim().is_empty() { secs } else {
                                            let t: String = l.text.trim().chars().take(34).collect();
                                            format!("{}{} · {secs}", t, if l.text.trim().chars().count() > 34 { "…" } else { "" })
                                        };
                                        ui.label(RichText::new(st).font(body(11.0)).color(if tk { OK_TEXT() } else { MUTED() }));
                                    });
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;
                                        if ph == DPhase::Edit {
                                            if kit::icon_button(ui, Icon::Trash, 30.0).on_hover_text("Убрать реплику").clicked() { act = Some((7, i)); }
                                            if i + 1 < d.lines.len() && kit::icon_button(ui, Icon::Expand, 30.0).on_hover_text("Склеить со следующей").clicked() { act = Some((8, i)); }
                                        } else if d.rec_since.is_none() {
                                            if kit::icon_button(ui, Icon::Mic, 30.0).on_hover_text("Записать эту реплику").clicked() { act = Some((9, i)); }
                                            if tk && kit::icon_button(ui, Icon::Headphones, 30.0).on_hover_text("Послушать свою запись").clicked() { act = Some((3, i)); }
                                        }
                                        if d.rec_since.is_none() && kit::icon_button(ui, Icon::Play, 30.0).on_hover_text("Оригинал").clicked() { act = Some((2, i)); }
                                    });
                                });
                            });
                        }
                    });
                });
                ui.add_space(8.0);
                self.boris_below(ui, col_w);
            });
        });
        let Some((a, i)) = act else { return };
        match a {
            2 => self.dub_preview(i),
            3 => { let tk = self.dub.as_ref().and_then(|d| d.take(d.who, i).cloned()); if let Some(t) = tk { self.game_player = audio::Player::from_samples(t.samples, t.sr, &None, 1.0).ok(); } }
            6 => if let Some(d) = &mut self.dub { d.resplit(); d.takes.clear(); },
            7 => if let Some(d) = &mut self.dub { if i < d.lines.len() { d.lines.remove(i); } d.sel = None; d.save(); d.takes.clear(); },
            8 => if let Some(d) = self.dub.as_mut().filter(|d| i + 1 < d.lines.len()) {
                let next = d.lines.remove(i + 1);
                let l = &mut d.lines[i];
                l.end = next.end;
                l.text = format!("{} {}", l.text, next.text).trim().to_string();
                d.save(); d.takes.clear();
            },
            9 => self.dub_goto(i),
            51 => if let Some(d) = &self.dub { d.save(); },
            22 => { self.dub_stop_play(); let c = self.dub.as_ref().map(|d| d.cur).unwrap_or(0); self.dub_goto(c); }
            23 => { let n = self.dub.as_ref().map(|d| d.next_open(d.cur)).unwrap_or(0); self.dub_goto(n); }
            24 => if let Some(d) = &mut self.dub { if i < d.lines.len() { d.sel = Some(i); d.cursor = d.lines[i].start; let (a, b) = (d.lines[i].start, d.lines[i].end); if a < d.view.0 || b > d.view.1 { let w = d.view.1 - d.view.0; d.view = ((a - w * 0.3).max(0.0), (a - w * 0.3).max(0.0) + w); } } },
            25 => {
                let r = self.dub.as_mut().and_then(|d| d.lines.get(i).map(|l| (l.start, l.end)).map(|(a, b)| { d.stop_at = Some(Instant::now() + Duration::from_secs_f32(b - a + 0.05)); d.play_from = Some((Instant::now(), a)); (d.piece(a, b), d.sr, a, d.video) }));
                if let Some((s, sr, a, video)) = r { self.game_player = audio::Player::from_samples(s, sr, &None, 1.0).ok(); if video { self.dub_video_at(a, true); } }
            }
            26 => self.dub_act_play_from_cursor(),
            27 => { let c = self.dub.as_ref().map(|d| d.cursor).unwrap_or(0.0); self.dub_stop_play(); self.dub_video_at(c, false); }
            39 => { // pause right where it is; ▶ goes on from here
                let at = self.dub.as_ref().and_then(|d| d.play_from.map(|(t0, f)| (f + t0.elapsed().as_secs_f32()).min(d.dur())));
                self.dub_stop_play();
                if let Some(t) = at { if let Some(d) = &mut self.dub { d.cursor = t; } self.dub_video_at(t, false); }
            }
            40 | 41 => {
                let to = self.dub.as_ref().map(|d| {
                    let c = d.play_from.map(|(t0, f)| f + t0.elapsed().as_secs_f32()).unwrap_or(d.cursor);
                    if a == 40 { d.lines.iter().rev().map(|l| l.start).find(|&s| s < c - 0.2).unwrap_or(0.0) } else { d.lines.iter().map(|l| l.start).find(|&s| s > c + 0.05).unwrap_or(c) }
                });
                if let Some(t) = to {
                    let was = self.dub.as_ref().map(|d| d.play_from.is_some()).unwrap_or(false);
                    if let Some(d) = &mut self.dub { d.cursor = t; d.sel = d.lines.iter().position(|l| (l.start - t).abs() < 0.01); }
                    if was { self.dub_stop_play(); self.dub_act_play_from_cursor(); } else { self.dub_video_at(t, false); }
                }
            }
            33 => if let Some(d) = &mut self.dub { // a new moment at the cursor, in the nearest free gap
                let dur = d.dur();
                let c = d.cursor;
                // gaps between moments (with a small margin), the one at or after the cursor first
                let mut edges = vec![0.0f32];
                for l in &d.lines { edges.push(l.start - 0.06); edges.push(l.end + 0.06); }
                edges.push(dur);
                let gaps: Vec<(f32, f32)> = edges.chunks(2).map(|g| (g[0].max(0.0), g[1].min(dur))).filter(|g| g.1 - g.0 >= 0.4).collect();
                let pick = gaps.iter().find(|g| c >= g.0 && c <= g.1).or_else(|| gaps.iter().find(|g| g.0 >= c)).or_else(|| gaps.last()).copied();
                match pick {
                    Some((g0, g1)) => {
                        let a = c.clamp(g0, (g1 - 0.4).max(g0));
                        let b = (a + 2.0).min(g1);
                        let i = d.lines.iter().position(|l| l.start > a).unwrap_or(d.lines.len());
                        d.lines.insert(i, Line::new(a, b));
                        d.sel = Some(i);
                        d.save(); d.takes.clear();
                    }
                    None => { self.say("Свободного места нет — сначала укоротите соседние моменты"); }
                }
            },
            34 | 35 | 36 | 37 => if let Some(d) = &mut self.dub {
                if i < d.lines.len() {
                    let lo = if i > 0 { d.lines[i - 1].end + 0.06 } else { 0.0 };
                    let hi = d.lines.get(i + 1).map(|n| n.start - 0.06).unwrap_or(d.dur());
                    let l = &mut d.lines[i];
                    match a {
                        34 => l.start = (l.start - 0.05).max(lo),
                        35 => l.start = (l.start + 0.05).min(l.end - 0.3),
                        36 => l.end = (l.end - 0.05).max(l.start + 0.3),
                        _ => l.end = (l.end + 0.05).min(hi),
                    }
                    d.cursor = if a < 36 { l.start } else { l.end };
                    d.save(); d.takes.clear();
                }
            },
            38 => if let Some(d) = &mut self.dub {
                if let Some(l) = d.lines.get(i) { let pad = ((l.end - l.start) * 0.25).max(0.3); d.view = ((l.start - pad).max(0.0), (l.end + pad).min(d.dur())); }
            },
            30 => if let Some(d) = &mut self.dub { // split the selected moment at the cursor
                if let Some(l) = d.lines.get(i).cloned() {
                    let c = d.cursor;
                    if c > l.start + 0.3 && c < l.end - 0.3 {
                        d.lines[i].end = c - 0.03;
                        // the words are shared out by time
                        let (t1, t2) = split_text(&l.text, (c - l.start) / (l.end - l.start));
                        d.lines[i].text = t1;
                        d.lines.insert(i + 1, Line { start: c + 0.03, end: l.end, text: t2 });
                        d.save(); d.takes.clear();
                    }
                }
            },
            31 => { // the result as a real video
                let job = self.dub.as_ref().and_then(|d| {
                    let scene = d.scene.clone()?;
                    let m = d.version(d.who);
                    let sr = d.sr;
                    let who = d.players.get(d.who).cloned().unwrap_or_default();
                    let out = done_dir(&d.root).join(format!("{} — озвучка {who} {}.mp4", scene_name(&scene), audio::stamp().replace(':', "-")));
                    Some(std::thread::spawn(move || {
                        let wav = std::env::temp_dir().join(format!("kj_dub_{}.wav", std::process::id()));
                        audio::write_wav(&wav, &m, sr)?;
                        let r = crate::ff::dub_video(&scene, &wav, &out).map(|_| out);
                        let _ = std::fs::remove_file(&wav);
                        r
                    }))
                });
                if let Some(j) = job { if let Some(d) = &mut self.dub { d.export = Some(j); } self.say("Собираю видео…"); }
            }
            10 => self.dub_begin(),
            32 => {
                self.dub_stop_play();
                if let Some(d) = &mut self.dub { if d.cut_mode { d.lines = std::mem::take(&mut d.lines_stash); d.cut_mode = false; } d.save(); d.phase = DPhase::Setup; d.scan(); }
                self.close_video = true;
            }
            11 => self.dub_start_rec(),
            12 => self.dub_stop_rec(),
            13 => { let n = self.dub.as_ref().map(|d| (d.cur + 1..d.lines.len()).find(|&k| d.take(d.who, k).is_none()).unwrap_or(d.lines.len())).unwrap_or(0); self.dub_goto(n); }
            14 => {
                // the premiere of this player's version
                let r = self.dub.as_ref().filter(|d| d.has_takes(d.who)).map(|d| {
                    let others = (0..d.players.len()).filter(|&p| p != d.who && d.has_takes(p)).count();
                    (d.who, d.players[d.who].clone(), others)
                });
                let Some((p, who, others)) = r else { self.say("Сначала запишите хотя бы одну реплику"); return; };
                self.dub_show(p);
                let k = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_millis() as usize).unwrap_or(0);
                let wow = ["Это шедевр!", "Учитесь, студенты!", "Сильно! Мощно!", "Оскар уже выехал!"][k % 4];
                self.boris.queue(if others > 0 { format!("Премьера! Версия {who}. {wow} А чья смешнее — решайте сами.") } else { format!("Премьера! Версия {who}. {wow}") });
            }
            43 => { // the next player, same clip
                self.dub_stop_play();
                if let Some(d) = &mut self.dub { d.next_who = (d.who + 1) % d.players.len().max(1); d.show_since = None; }
                self.dub_begin();
            }
            44 => { // another clip: back to the list, the next player already chosen
                self.dub_stop_play();
                if let Some(d) = &mut self.dub { d.next_who = (d.who + 1) % d.players.len().max(1); d.show_since = None; d.phase = DPhase::Setup; d.scan(); }
                self.close_video = true;
            }
            20 => self.dub_show(i),
            42 => if let Some(d) = &mut self.dub { d.rec_bg = !d.rec_bg; },
            21 => {
                self.dub_stop_play();
                if let Some(d) = &mut self.dub {
                    d.lines_stash = std::mem::take(&mut d.lines);
                    d.lines = vec![Line::new(0.0, d.dur())];
                    d.cut_mode = true; d.sel = Some(0); d.view = (0.0, d.dur()); d.cursor = 0.0;
                }
                self.boris.queue("Режем! Поставьте курсор, «Разрезать» — и раздвиньте края, чтобы выкинуть лишнее.".into());
            }
            45 => { // preview of the result: only the pieces, the video jumps over the gaps
                self.dub_stop_play();
                let r = self.dub.as_mut().map(|d| {
                    let mut p: Vec<(f32, f32)> = d.lines.iter().map(|l| (l.start, l.end)).collect();
                    p.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let parts: Vec<Vec<f32>> = p.iter().map(|&(a, b)| d.piece(a, b)).collect();
                    let audio = crate::apozh::glue(&parts, d.sr);
                    let first = p.first().map(|x| x.0).unwrap_or(0.0);
                    d.preview = Some((Instant::now(), p));
                    (audio, d.sr, first, d.video)
                });
                if let Some((s, sr, a, video)) = r { self.game_player = audio::Player::from_samples(s, sr, &None, 1.0).ok(); if video { self.dub_video_at(a, true); } }
            }
            46 | 47 => self.dub_save_cut(a == 46),
            48 => { // leave without saving
                self.dub_stop_play();
                if let Some(d) = &mut self.dub { d.lines = std::mem::take(&mut d.lines_stash); d.cut_mode = false; d.sel = None; d.preview = None; }
            }
            15 => {
                let r = self.dub.as_ref().map(|d| {
                    let m = d.version(d.who);
                    let name = d.scene.as_ref().map(|p| scene_name(p)).unwrap_or_default();
                    let who = d.players.get(d.who).cloned().unwrap_or_default();
                    let path = done_dir(&d.root).join(format!("{name} — озвучка {who} {}.wav", audio::stamp().replace(':', "-")));
                    audio::write_wav(&path, &m, d.sr).map(|_| path)
                });
                match r { Some(Ok(p)) => { self.say("Звук сохранён в «Озвучка → Готовые»"); show_in_folder(&p); } Some(Err(e)) => self.say(e), None => {} }
            }
            17 => self.dub_play_all(),
            18 => self.dub_stop_play(),
            16 => {
                self.game_player = None;
                self.dub_js("window.kj && kj(-1, false);");
                if let Some(d) = &mut self.dub { d.next_who = d.who; d.show_since = None; }
                self.dub_begin();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moments_follow_the_clip_name() {
        let d = std::env::temp_dir().join("kj_lines_names");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let scene = d.join("Птичка (2).mp4");
        std::fs::write(&scene, b"x").unwrap();
        // made by another copy: «Птичка.mp4.lines», with a player column and commas
        std::fs::write(d.join("Птичка.mp4.lines"), "1,5\t3,25\t0\n5.0\t7.5\t1\n").unwrap();
        let v = load_lines(&scene).unwrap();
        assert_eq!(v.len(), 2);
        assert!((v[0].start - 1.5).abs() < 1e-3 && (v[1].end - 7.5).abs() < 1e-3);
        assert!(lines_path(&scene).exists(), "adopted as the clip's own");
        // an old virtual trim shifts the moments to the file's time
        let s2 = d.join("Сцена.mp4");
        std::fs::write(&s2, b"x").unwrap();
        save_lines(&s2, &[Line::new(1.0, 2.0)]);
        std::fs::write(d.join("Сцена.mp4.trim"), "10.00 30.00\n").unwrap();
        upgrade_old_trim(&s2);
        let v = load_lines(&s2).unwrap();
        assert!((v[0].start - 11.0).abs() < 1e-3, "{}", v[0].start);
        assert!(!d.join("Сцена.mp4.trim").exists());
    }


    fn tone(secs: f32, sr: u32, amp: f32) -> Vec<f32> { (0..(secs * sr as f32) as usize).map(|i| (i as f32 * 0.07).sin() * amp).collect() }

    #[test]
    fn finds_lines_between_pauses() {
        let sr = 16000;
        let mut x = vec![0.001f32; sr as usize];
        x.extend(tone(1.5, sr, 0.4));
        x.extend(vec![0.001; (sr as f32 * 0.8) as usize]);
        x.extend(tone(2.0, sr, 0.4));
        x.extend(vec![0.001; sr as usize]);
        let s = segment(&x, sr, 0.5);
        assert_eq!(s.len(), 2, "{s:?}");
        assert!((s[0].0 - 1.0).abs() < 0.2 && (s[1].0 - 3.3).abs() < 0.2, "{s:?}");
    }

    #[test]
    fn splits_long_monologue() {
        let sr = 16000;
        let mut x = tone(5.0, sr, 0.4);
        x.extend(tone(0.1, sr, 0.05));
        x.extend(tone(5.0, sr, 0.4));
        let s = segment(&x, sr, 0.5);
        assert!(s.len() >= 2 && s.iter().all(|(a, b)| b - a <= 7.5), "{s:?}");
    }

    #[test]
    fn mixes_takes_in_place() {
        let orig = vec![0.5f32; 48000];
        let take = vec![0.25f32; 8000];
        let m = mix(&orig, 48000, 0.0, &[(0.5, &take, 16000)]);
        assert_eq!(m.len(), 48000);
        assert_eq!(m[100], 0.0);
        assert!((m[24000 + 100] - 0.25).abs() < 1e-3);
    }

    #[test]
    fn url_and_lines_roundtrip() {
        let u = file_url(Path::new("C:\\Users\\Макс\\Озвучка\\сцена 1.mp4"));
        assert!(u.starts_with("file:///C:/Users/") && !u.contains(' ') && u.ends_with(".mp4"), "{u}");
        let p = std::env::temp_dir().join("dub_test_scene.mp4");
        save_lines(&p, &[Line::new(1.0, 2.5)]);
        let v = load_lines(&p).unwrap();
        assert_eq!(v.len(), 1);
        assert!((v[0].end - 2.5).abs() < 1e-3);
    }


    #[test]
    fn devoice_uses_the_clean_sound_or_turns_down() {
        let sr = 1000;
        let mid = vec![0.5f32; 3000];
        let clean = vec![0.1f32; 3000];
        let out = devoice(&mid, sr, &[(1.0, 2.0)], Some(&clean));
        assert!((out[1500] - 0.1).abs() < 1e-6 && (out[100] - 0.5).abs() < 1e-6);
        let out = devoice(&mid, sr, &[(1.0, 2.0)], None);
        assert!((out[1500] - 0.1).abs() < 1e-6 && (out[100] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn contour_follows_pitch() {
        let sr = 16000;
        let x: Vec<f32> = (0..sr as usize).map(|i| (std::f32::consts::TAU * 220.0 * i as f32 / sr as f32).sin() * 0.3).collect();
        let c = contour(&x, sr);
        let m = median_note(&c).unwrap();
        assert!((m - 57.0).abs() < 0.3, "{m}"); // A3
    }


    #[test]
    fn moments_keep_their_text() {
        let v = vec![Line { start: 0.9, end: 4.7, text: "Привет, болото, ты кто такой?".into() }, Line::new(8.2, 11.1)];
        let dir = std::env::temp_dir().join(format!("kj_txt_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sc = dir.join("a.mp4");
        save_lines(&sc, &v);
        assert_eq!(load_lines(&sc).unwrap(), v);
        assert!(parse_lines("1.0\t2.0\t0\t1\n").unwrap()[0].text.is_empty());
        // subtitles next to a clip are its moments
        let sc2 = dir.join("Шрек.mp4");
        std::fs::write(dir.join("Шрек.srt"), "1\n00:00:01,000 --> 00:00:02,000\nЭто болото!\n").unwrap();
        assert_eq!(load_lines(&sc2).unwrap()[0].text, "Это болото!");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(split_text("раз два три четыре", 0.5), ("раз два".into(), "три четыре".into()));
        // a cut keeps the moments where they were in the picture
        let m = vec![Line { start: 2.0, end: 3.0, text: "а".into() }, Line::new(5.0, 6.0), Line::new(10.5, 11.5)];
        let v = move_moments(&m, &[(1.0, 4.0), (9.0, 12.0)]);
        assert_eq!(v, vec![Line { start: 1.0, end: 2.0, text: "а".into() }, Line::new(4.5, 5.5)]);
    }

    #[test]
    fn categories_and_random() {
        let root = std::env::temp_dir().join(format!("kj_cats_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(DONE)).unwrap();
        // the first time: the default categories appear, «Готовые» is not one
        assert_eq!(categories(&root), vec!["Фильмы", "Мультфильмы", "Мемы"]);
        // deleting them all doesn't bring them back
        for c in DEFAULT_CATS { std::fs::remove_dir(root.join(c)).unwrap(); }
        assert!(categories(&root).is_empty());
        std::fs::create_dir_all(root.join("Шрек")).unwrap();
        // a clip moves with its moments and its voiceless sound
        let clip = root.join("сцена.mp4");
        std::fs::write(&clip, b"x").unwrap();
        save_lines(&clip, &[Line::new(1.0, 2.0)]);
        std::fs::write(crate::separate::cache_path(&clip), b"bg").unwrap();
        let moved = move_scene(&clip, &root.join("Шрек")).unwrap();
        assert_eq!(cat_of(&root, &moved), "Шрек");
        assert!(!clip.exists() && lines_path(&moved).exists() && crate::separate::cache_path(&moved).exists());
        assert_eq!(cat_of(&root, &root.join("x.mp4")), "");
        let _ = std::fs::remove_dir_all(&root);
        // random: in range, never the one to avoid
        for _ in 0..200 {
            let k = random_pick(5, Some(2)).unwrap();
            assert!(k < 5 && k != 2);
        }
        assert_eq!(random_pick(1, Some(0)), Some(0));
        assert_eq!(random_pick(0, None), None);
    }

    #[test]
    fn moments_never_overlap() {
        let sr = 16000;
        let mut x = tone(5.0, sr, 0.4);
        x.extend(tone(0.1, sr, 0.05));
        x.extend(tone(5.0, sr, 0.4));
        x.extend(vec![0.001; 3000]);
        x.extend(tone(1.0, sr, 0.4));
        let s = segment(&x, sr, 0.5);
        for w in s.windows(2) { assert!(w[1].0 >= w[0].1, "{s:?}"); }
    }

    #[test]
    fn birds_are_not_lines() {
        let sr = 16000;
        // a bird: a whistle wobbling between 2 and 5 kHz
        let chirp = |secs: f32| -> Vec<f32> {
            let mut ph = 0f32;
            (0..(secs * sr as f32) as usize).map(|i| { let t = i as f32 / sr as f32; ph += (3500.0 + 1500.0 * (t * 9.0).sin()) / sr as f32; (std::f32::consts::TAU * ph).sin() * 0.4 }).collect()
        };
        let mut x = vec![0.001f32; sr as usize];
        x.extend(tone(1.5, sr, 0.3));          // voice
        x.extend(chirp(1.2));                  // bird, between the lines
        x.extend(vec![0.001; (sr as f32 * 0.6) as usize]);
        x.extend(chirp(0.8));                  // bird alone
        x.extend(vec![0.001; (sr as f32 * 0.6) as usize]);
        x.extend(tone(2.0, sr, 0.3));          // voice
        x.extend(vec![0.001; sr as usize]);
        let s = segment(&x, sr, 0.5);
        assert_eq!(s.len(), 2, "{s:?}");
        assert!(s[0].1 < 2.8, "the first line stops where the bird starts: {s:?}");
    }
}

/* ---------- demo stand for README screenshots (Linux only) ---------- */

/// Speech-like test sound: syllables with a wandering pitch (for screenshots and tests).
#[cfg(not(windows))]
pub fn demo_voice(secs: f32, sr: u32, seed: u32) -> Vec<f32> {
    let n = (secs * sr as f32) as usize;
    let mut ph = 0f32;
    let mut r = seed.wrapping_mul(2654435761) | 1;
    let mut rnd = move || { r ^= r << 13; r ^= r >> 17; r ^= r << 5; (r % 1000) as f32 / 1000.0 };
    let syl: Vec<(f32, f32, f32)> = (0..(secs * 4.0) as usize).map(|k| (k as f32 * 0.25 + rnd() * 0.05, 0.14 + rnd() * 0.08, 0.5 + rnd() * 0.5)).collect();
    (0..n).map(|i| {
        let t = i as f32 / sr as f32;
        let f0 = 170.0 + 40.0 * (t * 2.1 + seed as f32).sin() + 25.0 * (t * 5.3).sin();
        ph += f0 / sr as f32;
        let mut env = 0f32;
        for &(a, l, g) in &syl { if t >= a && t < a + l { env = env.max(g * (std::f32::consts::PI * (t - a) / l).sin()); } }
        let v: f32 = (1..6).map(|h| (std::f32::consts::TAU * ph * h as f32).sin() / h as f32).sum();
        v * env * 0.35
    }).collect()
}

#[cfg(not(windows))]
impl Dub {
    pub(crate) fn demo(players: Vec<String>, edit: bool) -> Self {
        let mut d = Dub::new();
        let sr = 16000u32;
        // a «scene»: four lines of talk over quiet music
        let mut x = vec![0f32; (sr as f32 * 24.0) as usize];
        for (i, v) in x.iter_mut().enumerate() { let t = i as f32 / sr as f32; *v = ((std::f32::consts::TAU * 110.0 * t).sin() + (std::f32::consts::TAU * 164.8 * t).sin()) * 0.02; }
        let mut lines = vec![];
        for (k, (a, l)) in [(1.2f32, 2.6f32), (5.0, 3.4), (9.6, 2.2), (13.0, 4.1), (18.4, 3.0)].iter().enumerate() {
            let v = demo_voice(*l, sr, k as u32 + 1);
            let s0 = (a * sr as f32) as usize;
            for (j, s) in v.iter().enumerate() { if s0 + j < x.len() { x[s0 + j] += s; } }
            lines.push(Line::new(a - 0.1, a + l + 0.1));
        }
        let said = ["Кто опять съел все сосиски?", "Это не я, это ёжик! Он с утра какой-то подозрительный.", "Ёжик спит.", "Значит, он ест во сне. Я видел, честное слово!", "Ладно, жарим зефир."];
        for (l, t) in lines.iter_mut().zip(said) { l.text = t.into(); }
        d.side = vec![0.0; x.len()];
        d.fullenv = wave::envelope(&x, sr, 24 * 100);
        d.peak_max = x.iter().fold(1e-4f32, |m, v| m.max(v.abs()));
        d.orig = x; d.sr = sr; d.video = false;
        d.scene = Some(PathBuf::from("Сцена у костра.mp4"));
        d.lines = lines;
        d.players = players;
        if std::env::var("KJ_DEMO").as_deref() == Ok("cut") {
            d.phase = DPhase::Edit;
            d.view = (0.0, 24.0);
            d.lines_stash = std::mem::take(&mut d.lines);
            d.lines = vec![Line::new(0.0, 8.6), Line::new(12.2, 24.0)];
            d.cut_mode = true;
            d.sel = Some(1);
            d.cursor = 16.0;
            return d;
        }
        if edit {
            d.phase = DPhase::Edit;
            d.view = (0.0, 24.0);
            d.sel = Some(1);
            d.cursor = 6.3;
            return d;
        }
        d.phase = DPhase::Record;
        d.takes = vec![vec![None; d.lines.len()]; d.players.len()];
        let take = Take { at: 0.2, samples: demo_voice(2.8, sr, 9), sr };
        d.takes[0][0] = Some(take);
        d.cur = 1;
        let (a, b) = d.span(1);
        let piece = d.piece(a, b);
        d.refenv = wave::envelope(&piece, sr, 500);
        d.refc = Some((1, a, contour(&piece, sr)));
        // the take in progress: about two thirds through
        let mine = demo_voice(2.4, sr, 5);
        let c = contour(&mine, sr);
        let t0 = d.lines[1].start + 0.15;
        d.live = c.iter().enumerate().filter_map(|(i, m)| m.map(|m| (t0 + i as f32 * 0.02, m + 0.6))).collect();
        let env = wave::envelope(&mine, sr, 120);
        d.live_lvl = env.rms.iter().enumerate().map(|(i, v)| (t0 + i as f32 * 0.02, v * 0.2)).collect();
        d.play_from = Some((Instant::now() - Duration::from_secs_f32(t0 + 2.4 - a), a));
        d
    }
}
