//! Audio engine: microphone → effects → speakers, system loopback capture,
//! voice/music separation and scoring in a background thread.

use crate::dsp::{self, Effects, Resampler, Scorer, Spectrum, FFT_N};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/* ---------- shared parameters (GUI → audio) ---------- */

pub struct AtomicF32(AtomicU32);
impl AtomicF32 {
    pub fn new(v: f32) -> Self { Self(AtomicU32::new(v.to_bits())) }
    pub fn get(&self) -> f32 { f32::from_bits(self.0.load(Ordering::Relaxed)) }
    pub fn set(&self, v: f32) { self.0.store(v.to_bits(), Ordering::Relaxed) }
}

pub struct Params {
    pub monitor: AtomicF32,   // voice volume to output (0..1.5)
    pub reverb: AtomicF32,    // 0..1
    pub echo: AtomicF32,      // 0..1
    pub mon_eff: AtomicF32,   // effective monitor gain used by output callback
    pub aec: AtomicBool,      // remove speaker sound from the mic
    pub denoise: AtomicBool,  // RNNoise
    pub mic_gain: AtomicF32,  // input gain, 0.25..4.0
    pub autotune: std::sync::atomic::AtomicU8, // 0 off, 1 soft, 2 «робот» — the monitor only
}
impl Default for Params {
    fn default() -> Self {
        Self { monitor: AtomicF32::new(0.4), reverb: AtomicF32::new(0.25), echo: AtomicF32::new(0.1), mon_eff: AtomicF32::new(0.0), aec: AtomicBool::new(true), denoise: AtomicBool::new(false), mic_gain: AtomicF32::new(1.0), autotune: std::sync::atomic::AtomicU8::new(0) }
    }
}

/* ---------- shared state (audio → GUI) ---------- */

#[derive(Clone, Copy, PartialEq)]
pub enum Phase { Waiting, Calibrating, Singing }

#[derive(Clone)]
pub struct Status {
    pub phase: Phase,
    pub cal_left: f32,
    pub elapsed: f32,
    pub mic_level: f32,
    pub voice_level: f32,
    pub voice_on: bool,
    pub music_heard: bool,
    pub toast: Option<String>,
    pub finished: bool,
    /// recent singing quality 0..1 (smoothed over ~1.5 s)
    pub quality: f32,
    /// seconds since a voice was last heard (while singing)
    pub silent_for: f32,
    /// the note being sung right now (MIDI, fractional); 0 = none
    pub pitch: f32,
    /// how loud the music is relative to its loudest part, 0..1
    pub music_level: f32,
}
impl Default for Status {
    fn default() -> Self {
        Self { phase: Phase::Calibrating, cal_left: 0.0, elapsed: 0.0, mic_level: 0.0, voice_level: 0.0, voice_on: false, music_heard: false, toast: None, finished: false, quality: 0.0, silent_for: 0.0, pitch: 0.0, music_level: 0.0 }
    }
}

#[allow(dead_code)]
pub struct Outcome { pub score: u32, pub recording: Option<PathBuf>, pub sang_secs: f32, pub longest_note: f32, pub best_start: Option<f32> }

/* ---------- ring buffers shared with callbacks ---------- */

#[derive(Default)]
struct Rings {
    mic_an: VecDeque<f32>,   // cleaned mic mono @ mic rate (pitch)
    mic_raw: VecDeque<f32>,  // raw mic mono @ mic rate (voice/music separation)
    mon_out: VecDeque<[f32; 2]>,  // processed stereo voice @ out rate (to play)
    mon_an: VecDeque<f32>,   // what we actually played @ out rate (analysis)
    loop_an: VecDeque<f32>,  // system loopback mono @ out rate (analysis)
    aec_ref: VecDeque<f32>,  // system loopback mono @ out rate (echo canceller reference)
    rec_voice: VecDeque<f32>,
    rec_loop: VecDeque<f32>,
    last_loop: Option<Instant>,
}
fn push_capped(q: &mut VecDeque<f32>, x: f32, cap: usize) {
    if q.len() >= cap { q.pop_front(); }
    q.push_back(x);
}

pub struct EngineConfig {
    pub speakers_mode: bool,
    pub input_name: Option<String>,
    pub output_name: Option<String>,
    pub record_dir: PathBuf,
    /// wait for music to start before calibrating; stop automatically when it ends
    pub auto: bool,
}

pub struct Engine {
    _streams: Vec<cpal::Stream>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<Outcome>>,
    pub status: Arc<Mutex<Status>>,
}

pub fn list_devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let ins = host.input_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default();
    let outs = host.output_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default();
    (ins, outs)
}

/// Name of the Windows default output device (where the in-app video plays).
pub fn default_output_name() -> Option<String> {
    #[cfg(not(windows))]
    if let Ok(v) = std::env::var("KARAOKE_DEMO_DEFOUT") { return Some(v); }
    cpal::default_host().default_output_device().and_then(|d| d.name().ok())
}

fn find(devs: Result<impl Iterator<Item = cpal::Device>, cpal::DevicesError>, name: &Option<String>) -> Option<cpal::Device> {
    let n = name.as_ref()?;
    devs.ok()?.find(|d| d.name().map(|x| &x == n).unwrap_or(false))
}

fn mono_frames<T>(data: &[T], ch: usize, mut f: impl FnMut(f32))
where T: SizedSample, f32: FromSample<T> {
    let ch = ch.max(1);
    for fr in data.chunks(ch) {
        let s: f32 = fr.iter().map(|&x| f32::from_sample(x)).sum::<f32>() / ch as f32;
        f(s);
    }
}

impl Engine {
    pub fn start(cfg: EngineConfig, params: Arc<Params>) -> Result<Engine, String> {
        let host = cpal::default_host();
        let input = find(host.input_devices(), &cfg.input_name)
            .or_else(|| host.default_input_device())
            .ok_or("Микрофон не найден")?;
        let output = find(host.output_devices(), &cfg.output_name)
            .or_else(|| host.default_output_device())
            .ok_or("Устройство вывода звука не найдено")?;

        let in_sup = input.default_input_config().map_err(|e| format!("Микрофон: {e}"))?;
        let out_sup = output.default_output_config().map_err(|e| format!("Вывод: {e}"))?;
        let in_cfg: cpal::StreamConfig = in_sup.config();
        let out_cfg: cpal::StreamConfig = out_sup.config();
        let in_sr = in_cfg.sample_rate.0;
        let out_sr = out_cfg.sample_rate.0;

        let rings = Arc::new(Mutex::new(Rings::default()));
        let recording = Arc::new(AtomicBool::new(true));
        let err = |e: cpal::StreamError| eprintln!("stream error: {e}");
        let mut streams = Vec::new();

        /* --- microphone --- */
        {
            let rings = rings.clone();
            let params = params.clone();
            let rec = recording.clone();
            let ch = in_cfg.channels as usize;
            let mut fx = Effects::new(in_sr as f32);
            let mut tune = dsp::AutoTune::new(in_sr as f32);
            let mut cleaner = crate::cleanup::MicCleaner::new(in_sr, out_sr);
            let speakers = cfg.speakers_mode;
            let mut mono: Vec<f32> = Vec::with_capacity(4096);
            let mut cleaned: Vec<f32> = Vec::with_capacity(8192);
            let mut rs = Resampler::new(in_sr, out_sr);
            let cap_play = (out_sr as f32 * 0.25) as usize;
            let max_lat = (out_sr as f32 * 0.045) as usize;
            let tgt_lat = (out_sr as f32 * 0.015) as usize;
            let cap_rec = out_sr as usize * 10;
            macro_rules! mic_stream { ($t:ty) => {
                input.build_input_stream(&in_cfg, move |data: &[$t], _: &cpal::InputCallbackInfo| {
                    let (rv, ec) = (params.reverb.get(), params.echo.get());
                    let recording = rec.load(Ordering::Relaxed);
                    let (use_aec, use_nr) = (speakers && params.aec.load(Ordering::Relaxed), params.denoise.load(Ordering::Relaxed));
                    mono.clear();
                    let gain = params.mic_gain.get();
                    mono_frames(data, ch, |s| mono.push(dsp::soft_clip(s * gain)));
                    let mut r = rings.lock().unwrap();
                    for &s in &mono { push_capped(&mut r.mic_raw, s, 16384); }
                    let mut refq = std::mem::take(&mut r.aec_ref);
                    cleaned.clear();
                    cleaner.process(&mono, &mut refq, use_aec, use_nr, &mut |s| cleaned.push(s));
                    r.aec_ref = refq;
                    let at = params.autotune.load(Ordering::Relaxed);
                    for &s in &cleaned {
                        // the judge listens to the voice as it is; auto-tune only colours what you hear
                        push_capped(&mut r.mic_an, s, 16384);
                        let (l, rr) = fx.process(tune.process(s, at), rv, ec);
                        rs.push([l, rr], &mut |z| {
                            if r.mon_out.len() >= cap_play { r.mon_out.pop_front(); }
                            r.mon_out.push_back(z);
                            if recording { push_capped(&mut r.rec_voice, (z[0] + z[1]) * 0.5, cap_rec); }
                        });
                    }
                    // keep monitor latency low (device clocks drift)
                    if r.mon_out.len() > max_lat { let n = r.mon_out.len() - tgt_lat; r.mon_out.drain(..n); }
                }, err, None)
            }}
            let s = match in_sup.sample_format() {
                cpal::SampleFormat::F32 => mic_stream!(f32),
                cpal::SampleFormat::I16 => mic_stream!(i16),
                cpal::SampleFormat::U16 => mic_stream!(u16),
                cpal::SampleFormat::I32 => mic_stream!(i32),
                f => return Err(format!("Формат микрофона не поддерживается: {f:?}")),
            }.map_err(|e| format!("Не удалось открыть микрофон: {e}"))?;
            streams.push(s);
        }

        /* --- speakers (voice monitor) --- */
        {
            let rings = rings.clone();
            let params = params.clone();
            let ch = out_cfg.channels as usize;
            let mut g = 0f32;
            macro_rules! out_stream { ($t:ty) => {
                output.build_output_stream(&out_cfg, move |data: &mut [$t], _: &cpal::OutputCallbackInfo| {
                    let tgt = params.mon_eff.get();
                    let mut r = rings.lock().unwrap();
                    for fr in data.chunks_mut(ch.max(1)) {
                        g += (tgt - g) * 0.002;
                        let z = r.mon_out.pop_front().unwrap_or([0.0; 2]);
                        let (l, rr) = (dsp::soft_clip(z[0] * g), dsp::soft_clip(z[1] * g));
                        let m = (l + rr) * 0.5;
                        match fr.len() {
                            1 => fr[0] = <$t>::from_sample(m),
                            _ => for (i, o) in fr.iter_mut().enumerate() {
                                *o = <$t>::from_sample(match i { 0 => l, 1 => rr, _ => m });
                            },
                        }
                        push_capped(&mut r.mon_an, m, 16384);
                    }
                }, err, None)
            }}
            let s = match out_sup.sample_format() {
                cpal::SampleFormat::F32 => out_stream!(f32),
                cpal::SampleFormat::I16 => out_stream!(i16),
                cpal::SampleFormat::U16 => out_stream!(u16),
                cpal::SampleFormat::I32 => out_stream!(i32),
                f => return Err(format!("Формат вывода не поддерживается: {f:?}")),
            }.map_err(|e| format!("Не удалось открыть вывод звука: {e}"))?;
            streams.push(s);
        }

        /* --- system loopback (what the speakers play: YouTube + our voice) --- */
        let mut loopback_ok = false;
        {
            let rings = rings.clone();
            let rec = recording.clone();
            let ch = out_cfg.channels as usize;
            let cap_rec = out_sr as usize * 10;
            macro_rules! loop_stream { ($t:ty) => {
                output.build_input_stream(&out_cfg, move |data: &[$t], _: &cpal::InputCallbackInfo| {
                    let recording = rec.load(Ordering::Relaxed);
                    let mut r = rings.lock().unwrap();
                    r.last_loop = Some(Instant::now());
                    mono_frames(data, ch, |s| {
                        push_capped(&mut r.loop_an, s, 16384);
                        push_capped(&mut r.aec_ref, s, 24000);
                        if recording { push_capped(&mut r.rec_loop, s, cap_rec); }
                    });
                }, err, None)
            }}
            let s = match out_sup.sample_format() {
                cpal::SampleFormat::F32 => loop_stream!(f32).ok(),
                cpal::SampleFormat::I16 => loop_stream!(i16).ok(),
                _ => None,
            };
            if let Some(s) = s { streams.push(s); loopback_ok = true; }
        }

        for s in &streams { s.play().map_err(|e| format!("Не удалось запустить звук: {e}"))?; }

        let status = Arc::new(Mutex::new(Status {
            phase: if cfg.auto && loopback_ok { Phase::Waiting } else { Phase::Calibrating },
            ..Default::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let (status, stop, rings, params, rec) = (status.clone(), stop.clone(), rings.clone(), params.clone(), recording.clone());
            let speakers = cfg.speakers_mode;
            let auto = cfg.auto;
            let dir = cfg.record_dir.clone();
            std::thread::spawn(move || analysis(rings, params, status, stop, rec, in_sr, out_sr, speakers, loopback_ok, auto, dir))
        };
        Ok(Engine { _streams: streams, stop, worker: Some(worker), status })
    }

    pub fn finish(mut self) -> Outcome {
        self.stop.store(true, Ordering::Relaxed);
        let out = self.worker.take().and_then(|w| w.join().ok()).unwrap_or(Outcome { score: 0, recording: None, sang_secs: 0.0, longest_note: 0.0, best_start: None });
        out
    }
}

/* ---------- recording player ---------- */

/// Plays mono audio (a recording or a synthesized sound) through the default output,
/// with pause, seek and an optional end point.
pub struct Player {
    _stream: cpal::Stream,
    pos: Arc<std::sync::atomic::AtomicUsize>,
    seek: Arc<std::sync::atomic::AtomicUsize>,
    end: Arc<std::sync::atomic::AtomicUsize>,
    paused: Arc<AtomicBool>,
    len: usize,
    sr: u32,
}
impl Player {
    pub fn open(path: &std::path::Path, output_name: &Option<String>) -> Result<Player, String> {
        let mut r = hound::WavReader::open(path).map_err(|e| format!("Не удалось открыть запись: {e}"))?;
        let spec = r.spec();
        let ch = spec.channels.max(1) as usize;
        let raw: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => {
                let k = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
                r.samples::<i32>().filter_map(|x| x.ok()).map(|x| x as f32 * k).collect()
            }
            hound::SampleFormat::Float => r.samples::<f32>().filter_map(|x| x.ok()).collect(),
        };
        let mono: Vec<f32> = raw.chunks(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect();
        Self::from_samples(mono, spec.sample_rate, output_name, 1.0)
    }

    pub fn from_samples(mono: Vec<f32>, sr: u32, output_name: &Option<String>, volume: f32) -> Result<Player, String> {
        use std::sync::atomic::AtomicUsize;
        let data: Arc<Vec<i16>> = Arc::new(mono.iter().map(|v| (v * volume * 32767.0).clamp(-32768.0, 32767.0) as i16).collect());
        if data.len() < 2 { return Err("Запись пустая".into()); }
        let host = cpal::default_host();
        let dev = find(host.output_devices(), output_name).or_else(|| host.default_output_device()).ok_or("Устройство вывода звука не найдено")?;
        let sup = dev.default_output_config().map_err(|e| format!("Вывод: {e}"))?;
        let cfg: cpal::StreamConfig = sup.config();
        let step = sr as f64 / cfg.sample_rate.0 as f64;
        let och = cfg.channels as usize;
        let pos = Arc::new(AtomicUsize::new(0));
        let seek = Arc::new(AtomicUsize::new(usize::MAX));
        let end = Arc::new(AtomicUsize::new(usize::MAX));
        let paused = Arc::new(AtomicBool::new(false));
        let err = |e: cpal::StreamError| eprintln!("player: {e}");
        macro_rules! st { ($t:ty) => {{
            let (data, pos, seek, end, paused) = (data.clone(), pos.clone(), seek.clone(), end.clone(), paused.clone());
            let mut f = 0f64;
            dev.build_output_stream(&cfg, move |out: &mut [$t], _: &cpal::OutputCallbackInfo| {
                let sk = seek.swap(usize::MAX, Ordering::Relaxed);
                if sk != usize::MAX { f = sk as f64; }
                let n = data.len();
                let stop_at = end.load(Ordering::Relaxed).min(n - 1);
                for fr in out.chunks_mut(och.max(1)) {
                    if !paused.load(Ordering::Relaxed) && f >= stop_at as f64 && stop_at < n - 1 {
                        paused.store(true, Ordering::Relaxed);
                        end.store(usize::MAX, Ordering::Relaxed);
                    }
                    let pz = paused.load(Ordering::Relaxed);
                    let v = if pz || f >= (n - 1) as f64 { 0.0 } else {
                        let i = f as usize; let t = (f - i as f64) as f32;
                        let a = data[i] as f32; let b = data[i + 1] as f32;
                        f += step;
                        (a + (b - a) * t) / 32768.0
                    };
                    for o in fr.iter_mut() { *o = <$t>::from_sample(v); }
                }
                pos.store(f as usize, Ordering::Relaxed);
            }, err, None)
        }}}
        let stream = match sup.sample_format() {
            cpal::SampleFormat::F32 => st!(f32),
            cpal::SampleFormat::I16 => st!(i16),
            cpal::SampleFormat::U16 => st!(u16),
            cpal::SampleFormat::I32 => st!(i32),
            f => return Err(format!("Формат вывода не поддерживается: {f:?}")),
        }.map_err(|e| format!("Не удалось открыть вывод звука: {e}"))?;
        stream.play().map_err(|e| format!("Не удалось запустить звук: {e}"))?;
        let len = data.len();
        Ok(Player { _stream: stream, pos, seek, end, paused, len, sr })
    }
    pub fn position(&self) -> f32 { self.pos.load(Ordering::Relaxed).min(self.len) as f32 / self.sr as f32 }
    pub fn duration(&self) -> f32 { self.len as f32 / self.sr as f32 }
    pub fn paused(&self) -> bool { self.paused.load(Ordering::Relaxed) }
    pub fn finished(&self) -> bool { self.pos.load(Ordering::Relaxed) + 2 >= self.len }
    pub fn toggle(&self) {
        if self.finished() { self.seek.store(0, Ordering::Relaxed); self.paused.store(false, Ordering::Relaxed); return; }
        self.paused.store(!self.paused(), Ordering::Relaxed);
    }
    pub fn seek_frac(&self, f: f32) {
        self.end.store(usize::MAX, Ordering::Relaxed);
        self.seek.store(((f.clamp(0.0, 1.0)) * self.len as f32) as usize, Ordering::Relaxed);
    }
    /// Play only [from, from+secs) and then pause.
    pub fn play_range(&self, from: f32, secs: f32) {
        let a = ((from.max(0.0)) * self.sr as f32) as usize;
        let b = (((from + secs).max(0.0)) * self.sr as f32) as usize;
        self.end.store(b.min(self.len), Ordering::Relaxed);
        self.seek.store(a.min(self.len), Ordering::Relaxed);
        self.paused.store(false, Ordering::Relaxed);
    }
}

/// Cut [from, from+secs) of a WAV into a new file next to it.
pub fn save_clip(src: &std::path::Path, from: f32, secs: f32, dst: &std::path::Path) -> Result<(), String> {
    let mut r = hound::WavReader::open(src).map_err(|e| e.to_string())?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let a = (from.max(0.0) * spec.sample_rate as f32) as usize * ch;
    let n = (secs * spec.sample_rate as f32) as usize * ch;
    let mut w = hound::WavWriter::create(dst, spec).map_err(|e| e.to_string())?;
    let fade = (spec.sample_rate as usize / 4) * ch; // 250 ms fade in/out
    for (k, x) in r.samples::<i32>().skip(a).take(n).enumerate() {
        let x = x.map_err(|e| e.to_string())? as f32;
        let g = (k as f32 / fade as f32).min(1.0) * ((n.saturating_sub(k)) as f32 / fade as f32).min(1.0);
        w.write_sample((x * g) as i32).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

/* ---------- simple recorders (mini-game) ---------- */

/// Records the microphone on demand.
pub struct Recorder { _stream: cpal::Stream, buf: Arc<Mutex<Vec<f32>>>, on: Arc<AtomicBool>, level: Arc<AtomicF32>, pub sr: u32 }
impl Recorder {
    /// Microphone (with the user's mic gain).
    pub fn mic(name: &Option<String>, params: Arc<Params>) -> Result<Recorder, String> {
        let host = cpal::default_host();
        let dev = find(host.input_devices(), name).or_else(|| host.default_input_device()).ok_or("Микрофон не найден")?;
        let sup = dev.default_input_config().map_err(|e| format!("Микрофон: {e}"))?;
        Self::build(dev, sup, Some(params))
    }
    fn build(dev: cpal::Device, sup: cpal::SupportedStreamConfig, params: Option<Arc<Params>>) -> Result<Recorder, String> {
        let cfg: cpal::StreamConfig = sup.config();
        let sr = cfg.sample_rate.0;
        let ch = cfg.channels as usize;
        let max = sr as usize * 30;
        let buf = Arc::new(Mutex::new(Vec::<f32>::new()));
        let on = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicF32::new(0.0));
        let err = |e: cpal::StreamError| eprintln!("recorder: {e}");
        macro_rules! st { ($t:ty) => {{
            let (buf, on, level, params) = (buf.clone(), on.clone(), level.clone(), params.clone());
            let mut held = 0f32;
            dev.build_input_stream(&cfg, move |data: &[$t], _: &cpal::InputCallbackInfo| {
                let g = params.as_ref().map(|p| p.mic_gain.get()).unwrap_or(1.0);
                let rec = on.load(Ordering::Relaxed);
                let (mut s, mut n) = (0f32, 0usize);
                let mut b = if rec { buf.lock().ok() } else { None };
                mono_frames(data, ch, |x| {
                    let y = x * g;
                    s += y * y; n += 1;
                    if let Some(b) = b.as_mut() { if b.len() < max { b.push(y); } }
                });
                let r = if n > 0 { (s / n as f32).sqrt() } else { 0.0 };
                held = (held * 0.85).max(r);
                level.set(held);
            }, err, None)
        }}}
        let stream = match sup.sample_format() {
            cpal::SampleFormat::F32 => st!(f32),
            cpal::SampleFormat::I16 => st!(i16),
            cpal::SampleFormat::U16 => st!(u16),
            cpal::SampleFormat::I32 => st!(i32),
            f => return Err(format!("Формат не поддерживается: {f:?}")),
        }.map_err(|e| format!("Не удалось начать запись: {e}"))?;
        stream.play().map_err(|e| format!("Не удалось начать запись: {e}"))?;
        Ok(Recorder { _stream: stream, buf, on, level, sr })
    }
    pub fn begin(&self) { if let Ok(mut b) = self.buf.lock() { b.clear(); } self.on.store(true, Ordering::Relaxed); }
    pub fn end(&self) -> Vec<f32> { self.on.store(false, Ordering::Relaxed); self.buf.lock().map(|mut b| std::mem::take(&mut *b)).unwrap_or_default() }
    pub fn level(&self) -> f32 { self.level.get() }
}

/* ---------- live pitch monitor (voice test) ---------- */

/// Listens to the microphone and reports the current note and loudness.
pub struct PitchMonitor { _stream: cpal::Stream, midi: Arc<AtomicF32>, level: Arc<AtomicF32>, stop: Arc<AtomicBool> }
impl PitchMonitor {
    pub fn start(name: &Option<String>, params: Arc<Params>) -> Result<PitchMonitor, String> {
        let host = cpal::default_host();
        let dev = find(host.input_devices(), name).or_else(|| host.default_input_device()).ok_or("Микрофон не найден")?;
        let sup = dev.default_input_config().map_err(|e| format!("Микрофон: {e}"))?;
        let cfg: cpal::StreamConfig = sup.config();
        let sr = cfg.sample_rate.0;
        let ch = cfg.channels as usize;
        let buf = Arc::new(Mutex::new(VecDeque::<f32>::with_capacity(8192)));
        let err = |e: cpal::StreamError| eprintln!("pitch monitor: {e}");
        macro_rules! st { ($t:ty) => {{
            let (buf, params) = (buf.clone(), params.clone());
            dev.build_input_stream(&cfg, move |data: &[$t], _: &cpal::InputCallbackInfo| {
                let g = params.mic_gain.get();
                if let Ok(mut b) = buf.lock() { mono_frames(data, ch, |x| push_capped(&mut b, x * g, 8192)); }
            }, err, None)
        }}}
        let stream = match sup.sample_format() {
            cpal::SampleFormat::F32 => st!(f32),
            cpal::SampleFormat::I16 => st!(i16),
            cpal::SampleFormat::U16 => st!(u16),
            cpal::SampleFormat::I32 => st!(i32),
            f => return Err(format!("Формат микрофона не поддерживается: {f:?}")),
        }.map_err(|e| format!("Не удалось открыть микрофон: {e}"))?;
        stream.play().map_err(|e| format!("Не удалось запустить микрофон: {e}"))?;
        let (midi, level, stop) = (Arc::new(AtomicF32::new(0.0)), Arc::new(AtomicF32::new(0.0)), Arc::new(AtomicBool::new(false)));
        {
            let (buf, midi, level, stop) = (buf.clone(), midi.clone(), level.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(25));
                    let v: Vec<f32> = match buf.lock() { Ok(b) if b.len() >= 2048 => b.iter().rev().take(2048).rev().copied().collect(), _ => continue };
                    level.set(dsp::rms(&v));
                    midi.set(dsp::pitch(&v, sr as f32, 0.85).filter(|(f, _)| *f > 60.0 && *f < 1300.0).map(|(f, _)| dsp::to_midi(f)).unwrap_or(0.0));
                }
            });
        }
        Ok(PitchMonitor { _stream: stream, midi, level, stop })
    }
    /// Current note (MIDI, fractional) or None.
    pub fn midi(&self) -> Option<f32> { let m = self.midi.get(); if m > 0.0 { Some(m) } else { None } }
    pub fn level(&self) -> f32 { self.level.get() }
}
impl Drop for PitchMonitor { fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); } }

/// Length of a WAV file in seconds (from its header).
pub fn wav_seconds(path: &std::path::Path) -> Option<f32> {
    let r = hound::WavReader::open(path).ok()?;
    let s = r.spec();
    Some(r.duration() as f32 / s.sample_rate as f32)
}

/* ---------- microphone test ---------- */

pub struct MicTest { _stream: cpal::Stream, level: Arc<AtomicF32>, peak: Arc<AtomicF32> }
impl MicTest {
    pub fn start(name: &Option<String>, params: Arc<Params>) -> Result<MicTest, String> {
        let host = cpal::default_host();
        let dev = find(host.input_devices(), name).or_else(|| host.default_input_device()).ok_or("Микрофон не найден")?;
        let sup = dev.default_input_config().map_err(|e| format!("Микрофон: {e}"))?;
        let cfg: cpal::StreamConfig = sup.config();
        let ch = cfg.channels as usize;
        let level = Arc::new(AtomicF32::new(0.0));
        let peak_out = Arc::new(AtomicF32::new(0.0));
        let err = |e: cpal::StreamError| eprintln!("mic test: {e}");
        macro_rules! st { ($t:ty) => {{
            let (lv, pk, params) = (level.clone(), peak_out.clone(), params.clone());
            let (mut held, mut hold_pk) = (0f32, 0f32);
            dev.build_input_stream(&cfg, move |data: &[$t], _: &cpal::InputCallbackInfo| {
                let g = params.mic_gain.get();
                let (mut s, mut n, mut mx) = (0f32, 0usize, 0f32);
                mono_frames(data, ch, |x| { let y = x * g; s += y * y; n += 1; mx = mx.max(y.abs()); });
                let r = if n > 0 { (s / n as f32).sqrt() } else { 0.0 };
                held = (held * 0.9).max(r);
                hold_pk = (hold_pk * 0.97).max(mx);
                lv.set(held);
                pk.set(hold_pk);
            }, err, None)
        }}}
        let stream = match sup.sample_format() {
            cpal::SampleFormat::F32 => st!(f32),
            cpal::SampleFormat::I16 => st!(i16),
            cpal::SampleFormat::U16 => st!(u16),
            cpal::SampleFormat::I32 => st!(i32),
            f => return Err(format!("Формат микрофона не поддерживается: {f:?}")),
        }.map_err(|e| format!("Не удалось открыть микрофон: {e}"))?;
        stream.play().map_err(|e| format!("Не удалось запустить микрофон: {e}"))?;
        Ok(MicTest { _stream: stream, level, peak: peak_out })
    }
    pub fn level(&self) -> f32 { self.level.get() }
    /// Recent peak (after gain); ≥ 1.0 means the signal clips.
    pub fn peak(&self) -> f32 { self.peak.get() }
}

/* ---------- analysis thread ---------- */

fn take_into(src: &mut VecDeque<f32>, dst: &mut VecDeque<f32>, cap: usize) -> usize {
    let n = src.len();
    dst.extend(src.drain(..));
    while dst.len() > cap { dst.pop_front(); }
    n
}

#[allow(clippy::too_many_arguments)]
fn analysis(
    rings: Arc<Mutex<Rings>>, params: Arc<Params>, status: Arc<Mutex<Status>>, stop: Arc<AtomicBool>,
    recording: Arc<AtomicBool>, in_sr: u32, out_sr: u32, speakers: bool, loopback_ok: bool, auto: bool, dir: PathBuf,
) -> Outcome {
    let cal_secs = if speakers { 4.0 } else { 1.5 };
    let hop = (in_sr as usize / 47).max(256); // ~21 ms
    let edges = dsp::band_edges_hz();
    let nb = edges.len();

    let (mut mic, mut lp, mut mon) = (VecDeque::new(), VecDeque::new(), VecDeque::new());
    let mut raw = VecDeque::new();
    let mut new_mic = 0usize;
    let (mut sp_mic, mut sp_loop, mut sp_mon) = (Spectrum::new(), Spectrum::new(), Spectrum::new());
    let (mut pm, mut pl, mut pmon) = (vec![0f32; nb], vec![0f32; nb], vec![0f32; nb]);
    let (mut sm_mic, mut sm_ref) = (vec![0f32; nb], vec![0f32; nb]);
    let (mut h, mut floor) = (vec![0f32; nb], vec![1e-9f32; nb]);
    let (mut cal_m, mut cal_r) = (vec![0f64; nb], vec![0f64; nb]);
    let mut cal_frames = 0u32;
    let mut chroma = [0f32; 12];
    let mut scorer = Scorer::default();
    let mut phase = if auto && loopback_ok { Phase::Waiting } else { Phase::Calibrating };
    let mut cal_start = Instant::now();
    let t0 = Instant::now();
    let mut wait_ok = 0f32;
    let (mut silent, mut ref_peak) = (0f32, 0f32);
    let mut finished = false;
    let (mut mic_silent, mut warned_mic) = (0f32, false);
    let (mut quality, mut no_voice) = (0f32, 0f32);
    let (mut clip_hits, mut warned_clip) = (0u32, false);
    let mut music_peak = 0f32;
    let mut qbins: Vec<f32> = Vec::new();          // singing quality per 0.1 s since start (≈ recording time)
    let (mut hold_ref, mut hold_t, mut longest) = (0f32, 0f32, 0f32);
    let mut sing_start: Option<Instant> = None;
    let mut mon_scale = 1.0f32;
    let mut howl: Option<(f32, Instant)> = None;
    let mut last_toast_howl = Instant::now() - Duration::from_secs(60);

    // recording
    let _ = std::fs::create_dir_all(&dir);
    let rec_path = dir.join(format!("karaoke_{}.wav", stamp()));
    let spec = hound::WavSpec { channels: 1, sample_rate: out_sr, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut writer = hound::WavWriter::create(&rec_path, spec).ok();
    let mut wrote = 0usize;

    let mut tick_rec = |r: &mut Rings, flush: bool, writer: &mut Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>| {
        let Some(w) = writer.as_mut() else { r.rec_voice.clear(); r.rec_loop.clear(); return; };
        let loop_stale = r.last_loop.map(|t| t.elapsed() > Duration::from_millis(120)).unwrap_or(true);
        let mut n = r.rec_voice.len().min(r.rec_loop.len());
        if (loop_stale || flush) && r.rec_voice.len() > n { n = r.rec_voice.len(); }
        for _ in 0..n {
            let v = r.rec_voice.pop_front().unwrap_or(0.0);
            let l = r.rec_loop.pop_front().unwrap_or(0.0);
            let s = (v + l * 0.8).clamp(-1.0, 1.0);
            let _ = w.write_sample((s * 32767.0) as i16);
        }
        wrote += n;
    };

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(8));
        let mut toast = None;
        {
            let mut r = rings.lock().unwrap();
            new_mic += take_into(&mut r.mic_an, &mut mic, FFT_N);
            take_into(&mut r.mic_raw, &mut raw, FFT_N);
            take_into(&mut r.mon_an, &mut mon, FFT_N);
            let stale = r.last_loop.map(|t| t.elapsed() > Duration::from_millis(150)).unwrap_or(true);
            if stale { r.loop_an.clear(); lp.clear(); } else { take_into(&mut r.loop_an, &mut lp, FFT_N); }
            tick_rec(&mut r, false, &mut writer);
        }
        if new_mic < hop || mic.len() < 2048 { continue; }
        let dt = new_mic as f32 / in_sr as f32;
        new_mic = 0;

        let micv: Vec<f32> = mic.iter().copied().collect();
        let tail = &micv[micv.len() - 2048..];
        // separation & level use the raw mic (proven against speaker bleed);
        // pitch uses the cleaned one (echo-cancelled / denoised)
        let rawv: Vec<f32> = if raw.len() >= 2048 { raw.iter().copied().collect() } else { micv.clone() };
        let mic_rms = dsp::rms(&rawv[rawv.len() - 2048..]);
        let peak = rawv[rawv.len() - 2048..].iter().fold(0f32, |m, x| m.max(x.abs()));
        if peak > 0.9 { clip_hits += 1; } else if clip_hits > 0 && phase != Phase::Singing { clip_hits -= 1; }

        // spectra
        sp_mic.compute(&rawv);
        dsp::band_powers(&sp_mic.power, in_sr as f32, &edges, &mut pm);
        let music_on = loopback_ok && lp.len() >= 1024;
        if music_on {
            let lv: Vec<f32> = lp.iter().copied().collect();
            let mv: Vec<f32> = mon.iter().copied().collect();
            sp_loop.compute(&lv);
            sp_mon.compute(&mv);
            dsp::band_powers(&sp_loop.power, out_sr as f32, &edges, &mut pl);
            dsp::band_powers(&sp_mon.power, out_sr as f32, &edges, &mut pmon);
            // music-only reference = loopback minus our own voice output
            for b in 0..nb { pl[b] = (pl[b] - pmon[b]).max(0.0); }
            // chroma of the music
            let bin = out_sr as f32 / FFT_N as f32;
            let mut cur = [0f32; 12];
            let (k0, k1) = ((65.0 / bin) as usize, (2100.0 / bin) as usize);
            for k in k0.max(1)..k1.min(FFT_N / 2) {
                let p = (sp_loop.power[k] - sp_mon.power[k]).max(0.0);
                let pc = ((dsp::to_midi(k as f32 * bin).round() as i32 % 12) + 12) % 12;
                cur[pc as usize] += p;
            }
            for i in 0..12 { chroma[i] = chroma[i] * 0.93 + cur[i] * 0.07; }
        } else {
            pl.iter_mut().for_each(|x| *x = 0.0);
            for c in chroma.iter_mut() { *c *= 0.97; }
        }
        for b in 0..nb {
            sm_mic[b] = sm_mic[b] * 0.35 + pm[b] * 0.65;
            sm_ref[b] = sm_ref[b] * 0.8 + pl[b] * 0.2; // smoothing covers speaker→mic delay
        }
        let cmax = chroma.iter().cloned().fold(0f32, f32::max);
        let chroma_n: Option<[f32; 12]> = if music_on && cmax > 1e-12 { let mut c = chroma; c.iter_mut().for_each(|x| *x /= cmax); Some(c) } else { None };

        let ref_sum: f32 = sm_ref.iter().sum();
        music_peak = (music_peak * 0.9995).max(ref_sum);
        let mut cur_pitch = 0f32;
        let mut st = Status { phase, music_heard: music_on, mic_level: (mic_rms * 4.0).min(1.0), ..Default::default() };

        match phase {
            Phase::Waiting => {
                // wait until the song actually starts playing
                let lrms = if music_on { dsp::rms(&lp.iter().rev().take(2048).copied().collect::<Vec<_>>()) } else { 0.0 };
                if lrms > 0.004 { wait_ok += dt; } else { wait_ok = 0.0; }
                if wait_ok > 0.6 { phase = Phase::Calibrating; cal_start = Instant::now(); st.phase = phase; }
            }
            Phase::Calibrating => {
                let elapsed = cal_start.elapsed().as_secs_f32();
                for b in 0..nb { cal_m[b] += sm_mic[b] as f64; cal_r[b] += sm_ref[b] as f64; }
                cal_frames += 1;
                st.cal_left = (cal_secs - elapsed).max(0.0);
                if elapsed >= cal_secs {
                    for b in 0..nb {
                        let m = (cal_m[b] / cal_frames as f64) as f32;
                        let r = (cal_r[b] / cal_frames as f64) as f32;
                        floor[b] = m.max(1e-9);
                        h[b] = if r > 1e-12 { m / r } else { 0.0 };
                    }
                    phase = Phase::Singing;
                    sing_start = Some(Instant::now());
                    st.phase = phase;
                }
            }
            Phase::Singing => {
                scorer.total_time += dt;
                if mic_rms < 0.0008 { mic_silent += dt; } else { mic_silent = 0.0; }
                if clip_hits > 25 && !warned_clip {
                    warned_clip = true;
                    toast = Some("Микрофон перегружен — убавьте «Громкость микрофона» в «Звук».".to_string());
                }
                if mic_silent > 6.0 && !warned_mic {
                    warned_mic = true;
                    toast = Some("Микрофон молчит! Проверьте его в «⚙ Звук → Проверить микрофон» или выберите другой.".to_string());
                }
                // auto-stop: the music has been silent for a while
                if auto && loopback_ok {
                    ref_peak = ref_peak.max(ref_sum);
                    if !music_on || ref_sum < ref_peak * 0.003 { silent += dt; } else { silent = 0.0; }
                    let singing_for = sing_start.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
                    if silent > 6.0 && singing_for > 15.0 { finished = true; }
                }
                // how much of the mic signal is NOT the music
                let (mut tot, mut res) = (0f32, 0f32);
                for b in 0..nb {
                    let est = if speakers {
                        1.7 * if music_on { (h[b] * sm_ref[b]).max(floor[b] * 0.15) } else { floor[b] }
                    } else { floor[b] * 1.5 };
                    tot += sm_mic[b];
                    res += (sm_mic[b] - est).max(0.0);
                }
                let dom = if tot > 0.0 { res / tot } else { 0.0 };
                let voice_on = dom > if speakers { 0.42 } else { 0.3 } && mic_rms > 0.003;
                st.voice_on = voice_on;
                st.voice_level = if voice_on { (0.3 + dom * 0.7).min(1.0) } else { dom * 0.25 };

                // follow music loudness changes while not singing
                if speakers && music_on && !voice_on {
                    for b in 0..nb {
                        if sm_ref[b] > 1e-12 {
                            let r = sm_mic[b] / sm_ref[b];
                            if h[b] == 0.0 { h[b] = r; } else if r < h[b] * 3.0 { h[b] = h[b] * 0.985 + r * 0.015; }
                        }
                    }
                }

                let p = if voice_on { dsp::pitch(tail, in_sr as f32, if speakers { 0.75 } else { 0.82 }) } else { None };
                match p {
                    Some((f, _)) if f > 70.0 && f < 1100.0 => {
                        let m = dsp::to_midi(f);
                        let q = scorer.feed(m, dt, chroma_n.as_ref());
                        cur_pitch = m;
                        let bin = (t0.elapsed().as_secs_f32() * 10.0) as usize;
                        if qbins.len() <= bin { qbins.resize(bin + 1, 0.0); }
                        qbins[bin] += q * dt;
                        if hold_t > 0.0 && (m - hold_ref).abs() < 0.6 { hold_t += dt; } else { hold_ref = m; hold_t = dt; }
                        longest = longest.max(hold_t);
                        quality = if quality == 0.0 { q } else { quality * 0.93 + q * 0.07 };
                        no_voice = 0.0;
                        // feedback (howl) guard
                        if mic_rms > 0.3 {
                            match howl {
                                Some((hm, t)) if (m - hm).abs() < 0.25 => {
                                    if t.elapsed().as_secs_f32() > 1.5 && mon_scale > 0.2 {
                                        mon_scale *= 0.5;
                                        howl = Some((m, Instant::now()));
                                        if last_toast_howl.elapsed().as_secs() > 5 {
                                            toast = Some("Похоже на свист: громкость голоса в колонках уменьшена. Отодвиньте микрофон от колонок.".to_string());
                                            last_toast_howl = Instant::now();
                                        }
                                    }
                                }
                                _ => howl = Some((m, Instant::now())),
                            }
                        } else { howl = None; }
                    }
                    _ => { scorer.reset_pitch(); howl = None; no_voice += dt; if no_voice > 0.15 { hold_t = 0.0; } }
                }
            }
        }
        st.elapsed = sing_start.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
        let mon = if phase == Phase::Singing { params.monitor.get() * mon_scale } else { 0.0 };
        params.mon_eff.set(mon);
        st.toast = toast;
        st.finished = finished;
        st.quality = quality;
        st.silent_for = no_voice;
        st.pitch = cur_pitch;
        st.music_level = if music_on && music_peak > 1e-12 { (ref_sum / music_peak).sqrt().clamp(0.0, 1.0) } else { 0.0 };
        if let Ok(mut s) = status.lock() {
            let keep = s.toast.take();
            *s = st;
            if s.toast.is_none() { s.toast = keep; }
        }
    }

    params.mon_eff.set(0.0);
    recording.store(false, Ordering::Relaxed);
    {
        let mut r = rings.lock().unwrap();
        tick_rec(&mut r, true, &mut writer);
    }
    let mut rec_out = None;
    if let Some(w) = writer.take() {
        let _ = w.finalize();
        if wrote > out_sr as usize { rec_out = Some(rec_path.clone()); } else { let _ = std::fs::remove_file(&rec_path); }
    }
    // the best 15 seconds of singing
    let best_start = best_window(&qbins, 150).map(|b| (b as f32 / 10.0 - 0.5).max(0.0));
    Outcome { score: scorer.score(), recording: rec_out, sang_secs: scorer.sing_time, longest_note: longest, best_start }
}

/// Index of the window of `w` bins with the largest sum (None if nothing sung).
pub fn best_window(bins: &[f32], w: usize) -> Option<usize> {
    if bins.iter().all(|&x| x <= 0.0) { return None; }
    if bins.len() <= w { return Some(0); }
    let mut sum: f32 = bins[..w].iter().sum();
    let (mut best, mut at) = (sum, 0);
    for i in w..bins.len() {
        sum += bins[i] - bins[i - w];
        if sum > best { best = sum; at = i + 1 - w; }
    }
    Some(at)
}

pub fn stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let local = secs + crate::utc_offset_secs();
    let days = local.div_euclid(86400);
    let tod = local.rem_euclid(86400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}_{:02}-{:02}-{:02}", tod / 3600, tod % 3600 / 60, tod % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// voice: None = silent, Some((detune_cents, out_of_key))
    fn sim(voice: Option<(f32, bool)>, speakers: bool) -> u32 { sim_ex(voice, speakers, false) }
    fn sim_ex(voice: Option<(f32, bool)>, speakers: bool, aec: bool) -> u32 {
        let rings = Arc::new(Mutex::new(Rings::default()));
        let params = Arc::new(Params::default());
        let status = Arc::new(Mutex::new(Status::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let rec = Arc::new(AtomicBool::new(true));
        let dir = std::env::temp_dir().join(format!("kt_{}", rand_id()));
        let sr = 48000u32;
        let w = { let (r, p, s, st, rc, d) = (rings.clone(), params.clone(), status.clone(), stop.clone(), rec.clone(), dir.clone());
            std::thread::spawn(move || analysis(r, p, s, st, rc, sr, sr, speakers, true, false, d)) };
        let chords: [[f32; 3]; 4] = [[60., 64., 67.], [57., 60., 64.], [53., 57., 60.], [55., 59., 62.]];
        let hz = |m: f32| 440.0 * 2f32.powf((m - 69.0) / 12.0);
        let start = Instant::now();
        let mut n = 0u64;
        let mut delay = VecDeque::from(vec![0f32; 480]);
        let (mut ph_m, mut ph_v) = ([0f32; 3], 0f32);
        let mut seed = 12345u32;
        let mut cleaner = crate::cleanup::MicCleaner::new(sr, sr);
        let (mut pend, mut refq) = (Vec::new(), VecDeque::new());
        while start.elapsed().as_secs_f32() < 13.0 {
            std::thread::sleep(Duration::from_millis(10));
            let target = (start.elapsed().as_secs_f64() * sr as f64) as u64;
            let mut r = rings.lock().unwrap();
            while n < target {
                let t = n as f32 / sr as f32;
                let ch = chords[((t / 2.0) as usize) % 4];
                let mut music = 0.0;
                for i in 0..3 {
                    ph_m[i] += hz(ch[i]) / sr as f32;
                    for h in 1..5 { music += (2.0 * std::f32::consts::PI * ph_m[i] * h as f32).sin() * 0.05 / h as f32; }
                }
                let mut v = 0.0;
                if let Some((det, ook)) = voice {
                    if t > 5.0 && (t * 2.0).fract() < 0.85 {
                        let note = ch[((t * 2.0) as usize) % 3] + if ook { 1.0 } else { 0.0 } + det / 100.0;
                        let vib = 1.0 + 0.003 * (2.0 * std::f32::consts::PI * 5.0 * t).sin();
                        ph_v += hz(note) * vib / sr as f32;
                        for h in 1..7 { v += (2.0 * std::f32::consts::PI * ph_v * h as f32).sin() * 0.2 / h as f32; }
                    }
                }
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (seed as f32 / u32::MAX as f32 - 0.5) * 0.002;
                delay.push_back(music);
                let bleed = delay.pop_front().unwrap();
                let mic = if speakers { 0.35 * bleed } else { 0.0 } + v + noise;
                pend.push(mic);
                push_capped(&mut r.mic_raw, mic, 16384);
                refq.push_back(music);
                push_capped(&mut r.loop_an, music, 16384);
                r.rec_voice.push_back(v); r.rec_loop.push_back(music);
                n += 1;
            }
            let mut outv = Vec::new();
            cleaner.process(&pend, &mut refq, aec, false, &mut |x| outv.push(x));
            pend.clear();
            for x in outv { push_capped(&mut r.mic_an, x, 16384); }
            r.last_loop = Some(Instant::now());
        }
        stop.store(true, Ordering::Relaxed);
        let out = w.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
        out.score
    }
    fn rand_id() -> u128 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() }

    #[test]
    fn auto_start_stop() {
        let rings = Arc::new(Mutex::new(Rings::default()));
        let params = Arc::new(Params::default());
        let status = Arc::new(Mutex::new(Status::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let rec = Arc::new(AtomicBool::new(true));
        let dir = std::env::temp_dir().join(format!("kt_{}", rand_id()));
        let sr = 48000u32;
        let w = { let (r, p, s, st, rc, d) = (rings.clone(), params.clone(), status.clone(), stop.clone(), rec.clone(), dir.clone());
            std::thread::spawn(move || analysis(r, p, s, st, rc, sr, sr, true, true, true, d)) };
        let hz = |m: f32| 440.0 * 2f32.powf((m - 69.0) / 12.0);
        let start = Instant::now();
        let (mut n, mut ph, mut pv) = (0u64, 0f32, 0f32);
        let mut log = Vec::new();
        let mut finished_at = None;
        while start.elapsed().as_secs_f32() < 34.0 {
            std::thread::sleep(Duration::from_millis(10));
            let el = start.elapsed().as_secs_f32();
            let target = (start.elapsed().as_secs_f64() * sr as f64) as u64;
            let music_playing = el > 1.5 && el < 22.0;
            let mut r = rings.lock().unwrap();
            while n < target {
                let t = n as f32 / sr as f32;
                let mut m = 0.0;
                if music_playing { ph += hz(60.0) / sr as f32; for h in 1..4 { m += (6.2832 * ph * h as f32).sin() * 0.1 / h as f32; } }
                let mut v = 0.0;
                if t > 8.0 && t < 20.0 { pv += hz(72.0) / sr as f32; for h in 1..6 { v += (6.2832 * pv * h as f32).sin() * 0.2 / h as f32; } }
                push_capped(&mut r.mic_an, 0.3 * m + v, 16384);
                if music_playing { push_capped(&mut r.loop_an, m, 16384); }
                n += 1;
            }
            if music_playing { r.last_loop = Some(Instant::now()); }
            drop(r);
            let s = status.lock().unwrap().clone();
            let ph_name = match s.phase { Phase::Waiting => "wait", Phase::Calibrating => "cal", Phase::Singing => "sing" };
            if log.last().map(|(p, _): &(&str, f32)| *p != ph_name).unwrap_or(true) { log.push((ph_name, el)); }
            if s.finished && finished_at.is_none() { finished_at = Some(el); break; }
        }
        stop.store(true, Ordering::Relaxed);
        let out = w.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
        println!("phases: {log:?}\nfinished at: {finished_at:?}, score {}", out.score);
        assert!(finished_at.is_some());
        assert!(out.score > 70);
    }

    #[test]
    fn scenarios() {
        let cases: Vec<(&str, Option<(f32, bool)>, bool)> = vec![
            ("колонки: чисто", Some((0.0, false)), true),
            ("колонки: фальшь 40ц", Some((40.0, false)), true),
            ("колонки: мимо тональности", Some((0.0, true)), true),
            ("колонки: молчит", None, true),
            ("наушники: чисто", Some((0.0, false)), false),
        ];
        let mut hs: Vec<_> = cases.into_iter().map(|(n, v, s)| std::thread::spawn(move || (n, sim(v, s)))).collect();
        for (n, v) in [("колонки+эхоподавление: чисто", Some((0.0, false))), ("колонки+эхоподавление: фальшь 40ц", Some((40.0, false))),
                       ("колонки+эхоподавление: молчит", None)] {
            hs.push(std::thread::spawn(move || (n, sim_ex(v, true, true))));
        }
        for h in hs { let (n, s) = h.join().unwrap(); println!("{n}: {s}"); }
    }
}

#[cfg(test)]
mod window_tests {
    #[test]
    fn best_window_finds_peak() {
        let mut b = vec![0.1f32; 600];
        for x in &mut b[300..450] { *x = 1.0; }
        assert_eq!(super::best_window(&b, 150), Some(300));
        assert_eq!(super::best_window(&[0.0; 10], 150), None);
        assert_eq!(super::best_window(&[0.5; 10], 150), Some(0));
    }
}

/* ---------- sound files for the mini-game library ---------- */

/// Decode a sound file (wav, mp3, ogg, flac, m4a) to mono. Long files are cut to `max_secs`.
pub fn load_mono(path: &std::path::Path, max_secs: f32) -> Result<(Vec<f32>, u32), String> {
    let (m, _, sr) = load_mid_side(path, max_secs)?;
    Ok((m, sr))
}

/// Decode to mid (L+R)/2 and side (L−R)/2. A mono file has an all-zero side.
pub fn load_mid_side(path: &std::path::Path, max_secs: f32) -> Result<(Vec<f32>, Vec<f32>, u32), String> {
    use symphonia::core::{audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint};
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(f), Default::default());
    let mut hint = Hint::new();
    if let Some(e) = path.extension().and_then(|e| e.to_str()) { hint.with_extension(e); }
    let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default()).map_err(|e| format!("Формат не распознан: {e}"))?;
    let mut fmt = probed.format;
    // a video file has several tracks: take the first one we can decode as sound
    let codecs = symphonia::default::get_codecs();
    let mut found = None;
    let mut last_err = String::from("В файле нет звука");
    for t in fmt.tracks() {
        if t.codec_params.sample_rate.is_none() && t.codec_params.channels.is_none() { continue; }
        match codecs.make(&t.codec_params, &DecoderOptions::default()) {
            Ok(d) => { found = Some((t.id, d, t.codec_params.sample_rate.unwrap_or(0))); break; }
            Err(e) => last_err = format!("Кодек звука не поддерживается: {e}"),
        }
    }
    let (tid, mut dec, mut sr) = found.ok_or(last_err)?;
    let mut out: Vec<f32> = Vec::new();
    let mut side: Vec<f32> = Vec::new();
    let mut buf: Option<SampleBuffer<f32>> = None;
    while let Ok(pkt) = fmt.next_packet() {
        if pkt.track_id() != tid { continue; }
        let Ok(d) = dec.decode(&pkt) else { continue };
        let spec = *d.spec();
        sr = spec.rate;
        let ch = spec.channels.count().max(1);
        if buf.as_ref().map(|b| b.capacity() < d.capacity() * ch).unwrap_or(true) { buf = Some(SampleBuffer::new(d.capacity() as u64, spec)); }
        let b = buf.as_mut().unwrap();
        b.copy_interleaved_ref(d);
        out.extend(b.samples().chunks(ch).map(|c| c.iter().sum::<f32>() / ch as f32));
        side.extend(b.samples().chunks(ch).map(|c| if c.len() >= 2 { (c[0] - c[1]) * 0.5 } else { 0.0 }));
        if sr > 0 && out.len() as f32 > max_secs * sr as f32 { break; }
    }
    if sr == 0 || out.len() < 1000 { return Err("Файл пустой или не читается".into()); }
    out.truncate((max_secs * sr as f32) as usize);
    side.truncate(out.len());
    Ok((out, side, sr))
}

/// Save mono samples as a 16-bit WAV.
pub fn write_wav(path: &std::path::Path, s: &[f32], sr: u32) -> Result<(), String> {
    if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
    let spec = hound::WavSpec { channels: 1, sample_rate: sr, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for &x in s { w.write_sample((x * 32767.0).clamp(-32768.0, 32767.0) as i16).map_err(|e| e.to_string())?; }
    w.finalize().map_err(|e| e.to_string())
}

#[cfg(test)]
mod file_tests {
    #[test]
    fn wav_roundtrip() {
        let p = std::env::temp_dir().join("karaoke_rt.wav");
        let s: Vec<f32> = (0..16000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        super::write_wav(&p, &s, 16000).unwrap();
        let (r, sr) = super::load_mono(&p, 20.0).unwrap();
        assert_eq!(sr, 16000);
        assert_eq!(r.len(), s.len());
        assert!((r[100] - s[100]).abs() < 0.001);
    }
}
