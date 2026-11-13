//! Microphone clean-up, applied before everything else:
//! 1. acoustic echo cancellation (SpeexDSP) — removes what the speakers play
//!    (music + our own monitored voice), using the system loopback as reference;
//! 2. noise suppression (RNNoise via `nnnoiseless`).

use crate::dsp::MonoResampler;
use nnnoiseless::DenoiseState;
use std::collections::VecDeque;
use std::os::raw::{c_int, c_void};

#[repr(C)] struct SpeexEchoState { _p: [u8; 0] }
#[repr(C)] struct SpeexPreprocessState { _p: [u8; 0] }

extern "C" {
    fn speex_echo_state_init(frame_size: c_int, filter_length: c_int) -> *mut SpeexEchoState;
    fn speex_echo_state_destroy(st: *mut SpeexEchoState);
    fn speex_echo_cancellation(st: *mut SpeexEchoState, rec: *const i16, play: *const i16, out: *mut i16);
    fn speex_echo_ctl(st: *mut SpeexEchoState, request: c_int, ptr: *mut c_void) -> c_int;
    fn speex_preprocess_state_init(frame_size: c_int, sampling_rate: c_int) -> *mut SpeexPreprocessState;
    fn speex_preprocess_state_destroy(st: *mut SpeexPreprocessState);
    fn speex_preprocess_run(st: *mut SpeexPreprocessState, x: *mut i16) -> c_int;
    fn speex_preprocess_ctl(st: *mut SpeexPreprocessState, request: c_int, ptr: *mut c_void) -> c_int;
}
const SPEEX_ECHO_SET_SAMPLING_RATE: c_int = 24;
const SPEEX_PREPROCESS_SET_DENOISE: c_int = 0;
const SPEEX_PREPROCESS_SET_AGC: c_int = 2;
const SPEEX_PREPROCESS_SET_DEREVERB: c_int = 8;
const SPEEX_PREPROCESS_SET_ECHO_SUPPRESS: c_int = 20;
const SPEEX_PREPROCESS_SET_ECHO_SUPPRESS_ACTIVE: c_int = 22;
const SPEEX_PREPROCESS_SET_ECHO_STATE: c_int = 24;

/// Speex echo canceller + residual-echo suppressor.
struct Aec { echo: *mut SpeexEchoState, pre: *mut SpeexPreprocessState }
// The raw pointers are only ever used from the thread that owns `MicCleaner`.
unsafe impl Send for Aec {}
impl Aec {
    fn new(frame: usize, sr: u32) -> Option<Self> {
        unsafe {
            // 250 ms tail: covers output/input buffering + room reverb
            let echo = speex_echo_state_init(frame as c_int, (sr as f32 * 0.25) as c_int);
            if echo.is_null() { return None; }
            let mut rate = sr as c_int;
            speex_echo_ctl(echo, SPEEX_ECHO_SET_SAMPLING_RATE, &mut rate as *mut _ as *mut c_void);
            let pre = speex_preprocess_state_init(frame as c_int, sr as c_int);
            if pre.is_null() { speex_echo_state_destroy(echo); return None; }
            let mut off: c_int = 0;
            speex_preprocess_ctl(pre, SPEEX_PREPROCESS_SET_DENOISE, &mut off as *mut _ as *mut c_void);
            speex_preprocess_ctl(pre, SPEEX_PREPROCESS_SET_AGC, &mut off as *mut _ as *mut c_void);
            speex_preprocess_ctl(pre, SPEEX_PREPROCESS_SET_DEREVERB, &mut off as *mut _ as *mut c_void);
            let mut sup: c_int = -40;
            speex_preprocess_ctl(pre, SPEEX_PREPROCESS_SET_ECHO_SUPPRESS, &mut sup as *mut _ as *mut c_void);
            let mut sup_a: c_int = -15; // gentle while the singer is active, so the voice survives
            speex_preprocess_ctl(pre, SPEEX_PREPROCESS_SET_ECHO_SUPPRESS_ACTIVE, &mut sup_a as *mut _ as *mut c_void);
            speex_preprocess_ctl(pre, SPEEX_PREPROCESS_SET_ECHO_STATE, echo as *mut c_void);
            Some(Self { echo, pre })
        }
    }
    fn run(&mut self, rec: &[i16], play: &[i16], out: &mut [i16]) {
        unsafe {
            speex_echo_cancellation(self.echo, rec.as_ptr(), play.as_ptr(), out.as_mut_ptr());
            speex_preprocess_run(self.pre, out.as_mut_ptr());
        }
    }
}
impl Drop for Aec {
    fn drop(&mut self) {
        unsafe { speex_preprocess_state_destroy(self.pre); speex_echo_state_destroy(self.echo); }
    }
}

/// RNNoise works on 480-sample frames at 48 kHz; resample around it if needed.
struct Denoiser {
    st: Box<DenoiseState<'static>>,
    up: Option<MonoResampler>,
    down: Option<MonoResampler>,
    buf: Vec<f32>,
    frame_out: Vec<f32>,
}
impl Denoiser {
    fn new(sr: u32) -> Self {
        let (up, down) = if sr == 48000 { (None, None) } else { (Some(MonoResampler::new(sr, 48000)), Some(MonoResampler::new(48000, sr))) };
        Self { st: DenoiseState::new(), up, down, buf: Vec::with_capacity(960), frame_out: vec![0.0; DenoiseState::FRAME_SIZE] }
    }
    fn push(&mut self, x: &[f32], out: &mut VecDeque<f32>) {
        let n = DenoiseState::FRAME_SIZE;
        for &s in x {
            match &mut self.up { Some(r) => { let b = &mut self.buf; r.push(s, &mut |y| b.push(y)); } None => self.buf.push(s) }
            while self.buf.len() >= n {
                let inp: Vec<f32> = self.buf.drain(..n).map(|v| v * 32768.0).collect();
                self.st.process_frame(&mut self.frame_out, &inp);
                for &y in &self.frame_out {
                    let y = y / 32768.0;
                    match &mut self.down { Some(r) => r.push(y, &mut |z| out.push_back(z)), None => out.push_back(y) }
                }
            }
        }
    }
}

pub struct MicCleaner {
    frame: usize,
    in_buf: Vec<f32>,
    out_q: VecDeque<f32>,
    ref_rs: MonoResampler,
    ref_q: VecDeque<f32>,
    max_ref: usize,
    aec: Option<Aec>,
    nr: Denoiser,
    active: bool,
    rec16: Vec<i16>, play16: Vec<i16>, out16: Vec<i16>,
}

impl MicCleaner {
    pub fn new(mic_sr: u32, ref_sr: u32) -> Self {
        let frame = (mic_sr / 100) as usize; // 10 ms
        Self {
            frame,
            in_buf: Vec::with_capacity(frame * 4),
            out_q: VecDeque::with_capacity(frame * 4),
            ref_rs: MonoResampler::new(ref_sr, mic_sr),
            ref_q: VecDeque::with_capacity(mic_sr as usize / 4),
            max_ref: (mic_sr as f32 * 0.12) as usize,
            aec: Aec::new(frame, mic_sr),
            nr: Denoiser::new(mic_sr),
            active: false,
            rec16: vec![0; frame], play16: vec![0; frame], out16: vec![0; frame],
        }
    }

    /// `ref_src` = loopback samples (at the speaker rate) that arrived since the last call.
    pub fn process(&mut self, input: &[f32], ref_src: &mut VecDeque<f32>, use_aec: bool, use_nr: bool, out: &mut impl FnMut(f32)) {
        let use_aec = use_aec && self.aec.is_some();
        if !use_aec && !use_nr {
            if self.active { self.active = false; self.in_buf.clear(); self.out_q.clear(); self.ref_q.clear(); }
            ref_src.clear();
            for &s in input { out(s); }
            return;
        }
        self.active = true;
        // reference → mic rate
        {
            let q = &mut self.ref_q;
            for s in ref_src.drain(..) { self.ref_rs.push(s, &mut |y| q.push_back(y)); }
            if q.len() > self.max_ref { let n = q.len() - self.frame * 3; q.drain(..n); }
        }
        self.in_buf.extend_from_slice(input);
        let f = self.frame;
        let mut done = 0;
        while self.in_buf.len() - done >= f {
            let block = &self.in_buf[done..done + f];
            let mut cleaned: Vec<f32>;
            if use_aec {
                for i in 0..f {
                    self.rec16[i] = (block[i] * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    let r = self.ref_q.pop_front().unwrap_or(0.0);
                    self.play16[i] = (r * 32767.0).clamp(-32768.0, 32767.0) as i16;
                }
                if let Some(a) = &mut self.aec { a.run(&self.rec16, &self.play16, &mut self.out16); }
                cleaned = self.out16.iter().map(|&v| v as f32 / 32768.0).collect();
            } else {
                cleaned = block.to_vec();
            }
            if use_nr { self.nr.push(&cleaned, &mut self.out_q); } else { self.out_q.extend(cleaned.drain(..)); }
            done += f;
        }
        self.in_buf.drain(..done);
        for s in self.out_q.drain(..) { out(s); }
    }
}

/// After the fact: remove a known sound (what we played through the speakers) from a recording.
/// Both start at the same moment; the echo may come up to ~250 ms later.
pub fn remove_known(mic: &[f32], mic_sr: u32, played: &[f32], played_sr: u32) -> Vec<f32> {
    let mut c = MicCleaner::new(mic_sr, played_sr);
    let mut out = Vec::with_capacity(mic.len());
    let chunk = (mic_sr / 100) as usize;
    let mut refq = VecDeque::new();
    let mut fed = 0usize;
    for (k, block) in mic.chunks(chunk).enumerate() {
        // the matching stretch of what was played (at its own rate)
        let upto = (((k + 1) * chunk) as f64 * played_sr as f64 / mic_sr as f64) as usize;
        while fed < upto { refq.push_back(played.get(fed).copied().unwrap_or(0.0)); fed += 1; }
        c.process(block, &mut refq, true, false, &mut |v| out.push(v));
    }
    out.resize(mic.len(), 0.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn energy(x: &[f32]) -> f32 { x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32 }

    /// Music played by the speakers reaches the mic delayed and filtered:
    /// the echo canceller must remove most of it.
    #[test]
    fn removes_speaker_music() {
        let sr = 48000u32;
        let mut c = MicCleaner::new(sr, sr);
        let hz = |m: f32| 440.0 * 2f32.powf((m - 69.0) / 12.0);
        let mut seed = 1u32;
        let total = sr as usize * 10;
        let music: Vec<f32> = (0..total).map(|n| {
            let t = n as f32 / sr as f32;
            let ch = [[60., 64., 67.], [57., 60., 64.], [53., 57., 60.], [55., 59., 62.]][((t / 2.0) as usize) % 4];
            let mut m = 0.0;
            for k in ch { for h in 1..5 { m += (std::f32::consts::TAU * hz(k) * h as f32 * t).sin() * 0.06 / h as f32; } }
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            m + (seed as f32 / u32::MAX as f32 - 0.5) * 0.02 // a bit of "drums"
        }).collect();
        // room: 35 ms delay, 0.4 gain, plus a reflection
        let d1 = (sr as f32 * 0.035) as usize; let d2 = (sr as f32 * 0.061) as usize;
        let mic: Vec<f32> = (0..total).map(|n| {
            let a = if n >= d1 { music[n - d1] * 0.4 } else { 0.0 };
            let b = if n >= d2 { music[n - d2] * 0.15 } else { 0.0 };
            a + b
        }).collect();
        let mut out = Vec::new();
        let mut refq = VecDeque::new();
        let chunk = 480;
        for i in (0..total).step_by(chunk) {
            refq.extend(music[i..i + chunk].iter().copied());
            c.process(&mic[i..i + chunk], &mut refq, true, false, &mut |s| out.push(s));
        }
        let tail = |x: &[f32]| x[x.len() - sr as usize * 3..].to_vec();
        let erle = 10.0 * (energy(&tail(&mic)) / energy(&tail(&out)).max(1e-12)).log10();
        println!("echo reduction after convergence: {erle:.1} dB");
        assert!(erle > 15.0, "ERLE too low: {erle}");
    }

    /// Clean singing must pass through both stages with its pitch intact.
    #[test]
    fn keeps_voice() {
        let sr = 48000u32;
        let mut c = MicCleaner::new(sr, sr);
        let f0 = 330.0;
        let total = sr as usize * 4;
        let mut ph = 0f32;
        let voice: Vec<f32> = (0..total).map(|n| {
            let t = n as f32 / sr as f32;
            let vib = 1.0 + 0.004 * (std::f32::consts::TAU * 5.5 * t).sin();
            ph += f0 * vib / sr as f32;
            (1..8).map(|h| (std::f32::consts::TAU * ph * h as f32).sin() * 0.25 / h as f32).sum::<f32>()
        }).collect();
        for (aec, nr) in [(true, false), (false, true)] {
            let mut c2 = MicCleaner::new(sr, sr);
            let mut o = Vec::new(); let mut q = VecDeque::new();
            for i in (0..total).step_by(480) { c2.process(&voice[i..i + 480], &mut q, aec, nr, &mut |s| o.push(s)); }
            let seg = &o[o.len() - 4096..];
            println!("aec={aec} nr={nr}: pitch {:?}, level {:.1} dB", crate::dsp::pitch(&seg[..2048], sr as f32, 0.5), 10.0 * (energy(seg) / energy(&voice[voice.len() - 4096..])).log10());
        }
        let mut out = Vec::new();
        let mut refq = VecDeque::new();
        for i in (0..total).step_by(480) { c.process(&voice[i..i + 480], &mut refq, true, true, &mut |s| out.push(s)); }
        let seg = &out[out.len() - 4096..];
        let p = crate::dsp::pitch(&seg[..2048], sr as f32, 0.8).map(|x| x.0);
        let keep = 10.0 * (energy(seg) / energy(&voice[voice.len() - 4096..])).log10();
        println!("voice level change: {keep:.1} dB, pitch {p:?}");
        assert!(keep > -12.0);
        assert!(p.map(|f| (f - f0).abs() < 8.0).unwrap_or(false));
    }

    #[test]
    fn suppresses_noise() {
        let sr = 48000u32;
        let mut c = MicCleaner::new(sr, sr);
        let mut seed = 7u32;
        let noise: Vec<f32> = (0..sr as usize * 3).map(|_| { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); (seed as f32 / u32::MAX as f32 - 0.5) * 0.05 }).collect();
        let mut out = Vec::new(); let mut q = VecDeque::new();
        for i in (0..noise.len()).step_by(480) { c.process(&noise[i..i + 480], &mut q, false, true, &mut |s| out.push(s)); }
        let red = 10.0 * (energy(&noise[sr as usize..]) / energy(&out[sr as usize..]).max(1e-12)).log10();
        println!("noise reduction: {red:.1} dB");
        assert!(red > 4.0);
    }

    /// Singer over speaker music (double talk): music goes, voice stays.
    #[test]
    fn voice_over_music() {
        let sr = 48000u32;
        let mut c = MicCleaner::new(sr, sr);
        let hz = |m: f32| 440.0 * 2f32.powf((m - 69.0) / 12.0);
        let total = sr as usize * 12;
        let (mut pm, mut pv) = ([0f32; 3], 0f32);
        let mut music = Vec::with_capacity(total);
        let mut voice = Vec::with_capacity(total);
        for n in 0..total {
            let t = n as f32 / sr as f32;
            let ch = [[60., 64., 67.], [57., 60., 64.], [53., 57., 60.], [55., 59., 62.]][((t / 2.0) as usize) % 4];
            let mut m = 0.0;
            for k in 0..3 { pm[k] += hz(ch[k]) / sr as f32; for h in 1..5 { m += (std::f32::consts::TAU * pm[k] * h as f32).sin() * 0.06 / h as f32; } }
            music.push(m);
            let mut v = 0.0;
            if t > 6.0 { pv += 392.0 / sr as f32; for h in 1..7 { v += (std::f32::consts::TAU * pv * h as f32).sin() * 0.15 / h as f32; } }
            voice.push(v);
        }
        let d1 = (sr as f32 * 0.04) as usize;
        let mic: Vec<f32> = (0..total).map(|n| voice[n] + if n >= d1 { music[n - d1] * 0.45 } else { 0.0 }).collect();
        let mut out = Vec::new(); let mut q = VecDeque::new();
        for i in (0..total).step_by(480) { q.extend(music[i..i + 480].iter().copied()); c.process(&mic[i..i + 480], &mut q, true, false, &mut |s| out.push(s)); }
        let seg = &out[out.len() - 2048..];
        let p = crate::dsp::pitch(seg, sr as f32, 0.8).map(|x| x.0);
        let before = crate::dsp::pitch(&mic[mic.len() - 2048..], sr as f32, 0.8).map(|x| x.0);
        let vkeep = 10.0 * (energy(seg) / energy(&voice[voice.len() - 2048..])).log10();
        println!("double talk: pitch before {before:?}, after {p:?}, voice level {vkeep:.1} dB");
        assert!(p.map(|f| (f - 392.0).abs() < 6.0).unwrap_or(false));
        assert!(vkeep > -8.0);
    }

    #[test]
    fn removes_known_background_from_a_short_take() {
        let sr = 48000u32;
        let n = sr as usize * 5;
        let mut seed = 7u32;
        let music: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / sr as f32;
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (std::f32::consts::TAU * 220.0 * t).sin() * 0.1 + (std::f32::consts::TAU * 330.0 * t).sin() * 0.08 + (seed as f32 / u32::MAX as f32 - 0.5) * 0.03
        }).collect();
        // the voice comes in the second half
        let voice: Vec<f32> = (0..n).map(|i| if i > n / 2 { (std::f32::consts::TAU * 180.0 * i as f32 / sr as f32).sin() * 0.2 } else { 0.0 }).collect();
        let d = (sr as f32 * 0.07) as usize;
        let mic: Vec<f32> = (0..n).map(|i| voice[i] + if i >= d { music[i - d] * 0.4 } else { 0.0 }).collect();
        let out = remove_known(&mic, sr, &music, sr);
        let part = |x: &[f32], a: f32, b: f32| x[(a * sr as f32) as usize..(b * sr as f32) as usize].to_vec();
        // background alone (after a short settling): strongly reduced
        let red = 10.0 * (energy(&part(&mic, 1.0, 2.4)) / energy(&part(&out, 1.0, 2.4)).max(1e-12)).log10();
        // the voice survives
        let keep = energy(&part(&out, 3.0, 4.8)) / energy(&part(&voice, 3.0, 4.8));
        println!("background down {red:.1} dB, voice kept {:.0}%", keep * 100.0);
        assert!(red > 10.0, "{red}");
        assert!(keep > 0.4, "{keep}");
    }
}
