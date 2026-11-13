//! Signal processing: effects, pitch detection, spectra and scoring.

use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

/* ---------------- effects ---------------- */

struct Comb { buf: Vec<f32>, idx: usize, store: f32 }
impl Comb {
    fn new(len: usize) -> Self { Self { buf: vec![0.0; len.max(1)], idx: 0, store: 0.0 } }
    #[inline]
    fn process(&mut self, x: f32, fb: f32, damp: f32) -> f32 {
        let out = self.buf[self.idx];
        self.store = out * (1.0 - damp) + self.store * damp;
        self.buf[self.idx] = x + self.store * fb;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
}
struct Allpass { buf: Vec<f32>, idx: usize }
impl Allpass {
    fn new(len: usize) -> Self { Self { buf: vec![0.0; len.max(1)], idx: 0 } }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let b = self.buf[self.idx];
        let out = b - x;
        self.buf[self.idx] = x + b * 0.5;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
}

/// Stereo Freeverb-style reverb, ping-pong echo, rumble filter.
pub struct Effects {
    combs: [Vec<Comb>; 2],
    aps: [Vec<Allpass>; 2],
    echo: [Vec<f32>; 2],
    echo_idx: usize,
    echo_len: usize,
    hp_a: f32, hp_x: f32, hp_y: f32,
}
impl Effects {
    pub fn new(sr: f32) -> Self {
        let k = sr / 44100.0;
        let mk = |spread: usize| -> (Vec<Comb>, Vec<Allpass>) {
            ([1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617].iter().map(|&n| Comb::new(((n + spread) as f32 * k) as usize)).collect(),
             [556, 441, 341, 225].iter().map(|&n| Allpass::new(((n + spread) as f32 * k) as usize)).collect())
        };
        let (cl, al) = mk(0);
        let (cr, ar) = mk(23);
        let rc = 1.0 / (2.0 * std::f32::consts::PI * 90.0);
        let dt = 1.0 / sr;
        Self { combs: [cl, cr], aps: [al, ar], echo: [vec![0.0; sr as usize], vec![0.0; sr as usize]], echo_idx: 0,
               echo_len: (sr * 0.28) as usize, hp_a: rc / (rc + dt), hp_x: 0.0, hp_y: 0.0 }
    }
    /// Returns (left, right).
    #[inline]
    pub fn process(&mut self, x: f32, reverb: f32, echo: f32) -> (f32, f32) {
        // remove rumble / handling noise below ~90 Hz
        let y = self.hp_a * (self.hp_y + x - self.hp_x);
        self.hp_x = x; self.hp_y = y;
        let x = y;
        let mut wet = [0f32; 2];
        if reverb > 0.001 {
            let inp = x * 0.02;
            for c in 0..2 {
                let mut w = 0.0;
                for cb in &mut self.combs[c] { w += cb.process(inp, 0.84, 0.3); }
                for a in &mut self.aps[c] { w = a.process(w); }
                wet[c] = w;
            }
        }
        // ping-pong echo
        let n = self.echo[0].len();
        let read = (self.echo_idx + n - self.echo_len) % n;
        let (el, er) = (self.echo[0][read], self.echo[1][read]);
        self.echo[0][self.echo_idx] = x + er * 0.4;
        self.echo[1][self.echo_idx] = el * 0.4;
        self.echo_idx = (self.echo_idx + 1) % n;
        let dry = x * (1.0 - reverb * 0.3);
        let e = echo * 0.8;
        (dry + wet[0] * reverb * 3.0 + el * e, dry + wet[1] * reverb * 3.0 + er * e)
    }
}

/// Gentle limiter so loud singing never clips.
#[inline]
pub fn soft_clip(x: f32) -> f32 { if x.abs() < 0.7 { x } else { x.signum() * (0.7 + 0.3 * ((x.abs() - 0.7) / 0.3).tanh()) } }

/* ---------------- linear resampler ---------------- */

pub struct Resampler { ratio: f64, pos: f64, prev: [f32; 2] }
impl Resampler {
    pub fn new(from: u32, to: u32) -> Self { Self { ratio: from as f64 / to as f64, pos: 0.0, prev: [0.0; 2] } }
    /// Feed one stereo input frame, emit 0..n output frames.
    #[inline]
    pub fn push(&mut self, x: [f32; 2], out: &mut impl FnMut([f32; 2])) {
        if (self.ratio - 1.0).abs() < 1e-9 { out(x); return; }
        while self.pos < 1.0 {
            let t = self.pos as f32;
            out([self.prev[0] + (x[0] - self.prev[0]) * t, self.prev[1] + (x[1] - self.prev[1]) * t]);
            self.pos += self.ratio;
        }
        self.pos -= 1.0;
        self.prev = x;
    }
}

/// Mono linear resampler.
pub struct MonoResampler { ratio: f64, pos: f64, prev: f32 }
impl MonoResampler {
    pub fn new(from: u32, to: u32) -> Self { Self { ratio: from as f64 / to as f64, pos: 0.0, prev: 0.0 } }
    #[inline]
    pub fn push(&mut self, x: f32, out: &mut impl FnMut(f32)) {
        if (self.ratio - 1.0).abs() < 1e-9 { out(x); return; }
        while self.pos < 1.0 {
            out(self.prev + (x - self.prev) * self.pos as f32);
            self.pos += self.ratio;
        }
        self.pos -= 1.0;
        self.prev = x;
    }
}

/* ---------------- pitch (McLeod NSDF) ---------------- */

pub fn rms(buf: &[f32]) -> f32 {
    if buf.is_empty() { return 0.0; }
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

/// Returns (frequency, clarity) if a clear pitch is found.
pub fn pitch(buf: &[f32], sr: f32, min_clarity: f32) -> Option<(f32, f32)> {
    let n = buf.len();
    if rms(buf) < 0.004 { return None; }
    let lo = (sr / 1100.0) as usize;
    let hi = ((sr / 70.0) as usize).min(n / 2);
    let mut d = vec![0f32; hi + 2];
    // start at a tiny lag so the first positive lobe of high notes is seen
    for t in 2..=hi {
        let (mut r, mut m) = (0f32, 0f32);
        let a = &buf[..n - t];
        let b = &buf[t..];
        for i in 0..n - t { let (x, y) = (a[i], b[i]); r += x * y; m += x * x + y * y; }
        d[t] = if m > 0.0 { 2.0 * r / m } else { 0.0 };
    }
    let mut peaks = Vec::new();
    let (mut pos, mut mx, mut ix) = (false, -1f32, 0usize);
    for t in 3..hi {
        let v = d[t];
        if !pos && v > 0.0 && d[t - 1] <= 0.0 { pos = true; mx = -1.0; }
        if pos {
            if v > mx { mx = v; ix = t; }
            if v <= 0.0 { peaks.push(ix); pos = false; }
        }
    }
    if pos && ix > 0 { peaks.push(ix); }
    peaks.retain(|&p| p >= lo);
    let best = peaks.iter().map(|&p| d[p]).fold(0f32, f32::max);
    let &p = peaks.iter().find(|&&p| d[p] >= best * 0.9)?;
    if d[p] < min_clarity || p == 0 { return None; }
    let (a, b, c) = (d[p - 1], d[p], d[p + 1]);
    let den = 2.0 * (a - 2.0 * b + c);
    let sh = if den.abs() > 1e-9 { ((a - c) / den).clamp(-1.0, 1.0) } else { 0.0 };
    Some((sr / (p as f32 + sh), b))
}

pub fn to_midi(f: f32) -> f32 { 69.0 + 12.0 * (f / 440.0).log2() }

/* ---------------- spectrum ---------------- */

pub const FFT_N: usize = 4096;

pub struct Spectrum {
    fft: Arc<dyn Fft<f32>>,
    win: Vec<f32>,
    buf: Vec<Complex<f32>>,
    pub power: Vec<f32>, // FFT_N/2 bins
}
impl Spectrum {
    pub fn new() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT_N);
        let win = (0..FFT_N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT_N as f32).cos()).collect();
        Self { fft, win, buf: vec![Complex::new(0.0, 0.0); FFT_N], power: vec![0.0; FFT_N / 2] }
    }
    /// `samples` = latest FFT_N samples (shorter is zero-padded at the start).
    pub fn compute(&mut self, samples: &[f32]) {
        let off = FFT_N.saturating_sub(samples.len());
        let src = &samples[samples.len().saturating_sub(FFT_N)..];
        for i in 0..FFT_N {
            let s = if i >= off { src[i - off] } else { 0.0 };
            self.buf[i] = Complex::new(s * self.win[i], 0.0);
        }
        self.fft.process(&mut self.buf);
        for k in 0..FFT_N / 2 { self.power[k] = self.buf[k].norm_sqr(); }
    }
}

/// Log-spaced bands 120 Hz .. 4 kHz.
pub fn band_edges_hz() -> Vec<(f32, f32)> {
    let mut v = Vec::new();
    let mut f = 120.0f32;
    while f < 4000.0 { let f2 = f * 1.19; v.push((f, f2)); f = f2; }
    v
}
pub fn band_powers(power: &[f32], sr: f32, edges: &[(f32, f32)], out: &mut [f32]) {
    let bin = sr / FFT_N as f32;
    for (b, &(lo, hi)) in edges.iter().enumerate() {
        let k0 = (lo / bin).round() as usize;
        let k1 = ((hi / bin).round() as usize).max(k0 + 1).min(power.len());
        out[b] = power[k0.min(power.len() - 1)..k1].iter().sum();
    }
}

/* ---------------- scoring ---------------- */

#[derive(Default, Clone)]
pub struct Scorer {
    pub sing_frames: u32,
    pub sum: f64,
    pub sing_time: f32,
    pub total_time: f32,
    prev: Option<f32>,
    smooth: Option<f32>,
}
impl Scorer {
    pub fn reset_pitch(&mut self) { self.prev = None; self.smooth = None; }
    /// Feed one detected pitch. `chroma` = music pitch-class profile (normalised), if music is heard.
    pub fn feed(&mut self, midi: f32, dt: f32, chroma: Option<&[f32; 12]>) -> f32 {
        let m = match self.smooth { Some(s) if (midi - s).abs() < 0.8 => s * 0.6 + midi * 0.4, _ => midi };
        self.smooth = Some(m);
        let dev = (m - m.round()).abs() * 100.0;
        let acc = if dev <= 12.0 { 1.0 } else { (1.0 - (dev - 12.0) / 33.0).max(0.0) };
        let stab = match self.prev { Some(p) => (1.0 - (m - p).abs() / 1.2).max(0.0), None => 1.0 };
        let q = match chroma {
            Some(c) => {
                let pc = ((m.round() as i32 % 12) + 12) % 12;
                let h = ((c[pc as usize] - 0.25) / 0.5).clamp(0.0, 1.0);
                0.40 * acc + 0.42 * h + 0.18 * stab
            }
            None => 0.72 * acc + 0.28 * stab,
        };
        self.sing_frames += 1;
        self.sum += q as f64;
        self.sing_time += dt;
        self.prev = Some(m);
        q
    }
    pub fn score(&self) -> u32 {
        if self.sing_frames == 0 { return 0; }
        let mean = (self.sum / self.sing_frames as f64) as f32;
        let mut sc = 100.0 * mean.powf(0.85);
        let ratio = if self.total_time > 0.0 { self.sing_time / self.total_time } else { 0.0 };
        if self.total_time > 20.0 && ratio < 0.2 { sc *= 0.6 + ratio * 2.0; }
        sc.round().clamp(0.0, 100.0) as u32
    }
}

#[cfg(test)]
mod pitch_tests {
    use super::*;
    #[test]
    fn pitch_accuracy() {
        let sr = 48000.0f32;
        let mut worst = 0f32;
        for midi in [45.0f32, 52.0, 57.0, 60.0, 64.0, 67.0, 69.0, 72.0, 76.0, 79.0, 84.0] {
            let f = 440.0 * 2f32.powf((midi - 69.0) / 12.0);
            for off in [0usize, 333, 1000] {
                let buf: Vec<f32> = (0..2048).map(|n| {
                    let t = (n + off) as f32 / sr;
                    (1..7).map(|h| (std::f32::consts::TAU * f * h as f32 * t).sin() * 0.2 / h as f32).sum()
                }).collect();
                let p = pitch(&buf, sr, 0.5).map(|x| x.0).unwrap_or(0.0);
                let cents = 1200.0 * (p / f).log2();
                worst = worst.max(cents.abs());
                if cents.abs() > 5.0 { println!("midi {midi} f {f:.1}: got {p:.1} ({cents:+.1} cents)"); }
            }
        }
        println!("worst error: {worst:.1} cents");
        assert!(worst < 5.0);
    }
}

/* ---------- auto-tune: pulls the voice you hear to the nearest note ---------- */

/// 0 — off, 1 — soft (partial, gliding), 2 — «робот» (instant, all the way).
/// Pitch is found every 10 ms on a 4× decimated copy; a two-tap delay-line shifter
/// (crossfaded sine windows, ~30 ms) moves it. Only the monitor hears it — the score never does.
pub struct AutoTune {
    sr: f32,
    buf: Vec<f32>, w: usize,
    win: f32, phase: f32,
    ratio: f32, want: f32, wet: f32,
    dec: Vec<f32>, acc: f32, acc_n: usize,
    since: usize,
}

impl AutoTune {
    pub fn new(sr: f32) -> Self {
        Self { sr, buf: vec![0.0; 8192], w: 0, win: (sr * 0.03).round(), phase: 0.0, ratio: 1.0, want: 1.0, wet: 0.0, dec: Vec::with_capacity(1024), acc: 0.0, acc_n: 0, since: 0 }
    }

    fn tap(&self, delay: f32) -> f32 {
        let m = self.buf.len() - 1;
        let p = self.w as f32 - 1.0 - delay;
        let i = p.floor();
        let f = p - i;
        let i = (i as isize).rem_euclid(self.buf.len() as isize) as usize;
        self.buf[i & m] * (1.0 - f) + self.buf[(i + 1) & m] * f
    }

    pub fn process(&mut self, x: f32, mode: u8) -> f32 {
        let m = self.buf.len() - 1;
        self.buf[self.w & m] = x;
        self.w = (self.w + 1) & m;
        if mode == 0 { self.dec.clear(); self.wet = 0.0; self.ratio = 1.0; return x; }
        // a 4× decimated history for pitch detection (42 ms at 12 kHz)
        self.acc += x; self.acc_n += 1;
        if self.acc_n == 4 {
            if self.dec.len() >= 512 { self.dec.remove(0); }
            self.dec.push(self.acc / 4.0);
            self.acc = 0.0; self.acc_n = 0;
        }
        self.since += 1;
        if self.since >= (self.sr * 0.01) as usize && self.dec.len() >= 512 {
            self.since = 0;
            self.want = match pitch(&self.dec, self.sr / 4.0, 0.85) {
                Some((f0, _)) if f0 > 75.0 && f0 < 1000.0 => {
                    let midi = to_midi(f0);
                    let diff = midi.round() - midi; // −0.5 … 0.5 semitones
                    let amount = if mode == 1 { 0.65 } else { 1.0 };
                    2f32.powf(diff * amount / 12.0)
                }
                _ => 1.0,
            };
        }
        // how fast the correction follows (per sample)
        let k = if mode == 1 { 0.0012 } else { 0.02 };
        self.ratio += (self.want - self.ratio) * k;
        let need = (self.ratio - 1.0).abs() > 0.0015;
        self.wet += ((if need { 1.0 } else { 0.0 }) - self.wet) * 0.003;
        // two read taps half a window apart, sliding at (1 − ratio)
        self.phase = (self.phase + (1.0 - self.ratio)).rem_euclid(self.win);
        let d1 = self.phase;
        let d2 = (self.phase + self.win * 0.5) % self.win;
        let g1 = (std::f32::consts::PI * d1 / self.win).sin();
        let g2 = (std::f32::consts::PI * d2 / self.win).sin();
        let shifted = self.tap(d1) * g1 + self.tap(d2) * g2;
        // when nothing needs fixing: the same latency, one clean copy
        let dry = self.tap(self.win * 0.5);
        dry * (1.0 - self.wet) + shifted * self.wet * 0.85
    }
}

#[cfg(test)]
mod autotune_tests {
    use super::*;

    fn run(f: f32, mode: u8) -> f32 {
        let sr = 48000.0;
        let mut t = AutoTune::new(sr);
        let out: Vec<f32> = (0..(sr * 1.5) as usize).map(|i| t.process((std::f32::consts::TAU * f * i as f32 / sr).sin() * 0.4, mode)).collect();
        // measure the pitch of the last part, on a 4× decimated copy like the detector
        let tail = &out[out.len() - 8192..];
        let dec: Vec<f32> = tail.chunks(4).map(|c| c.iter().sum::<f32>() / 4.0).collect();
        pitch(&dec[..1024], sr / 4.0, 0.5).map(|p| p.0).unwrap_or(0.0)
    }

    #[test]
    fn pulls_to_the_nearest_note() {
        // 452 Hz is between A4 (440) and A♯4 (466.2), closer to A4
        let off = run(452.0, 0);
        let hard = run(452.0, 2);
        let soft = run(452.0, 1);
        println!("off {off:.1}, soft {soft:.1}, hard {hard:.1}");
        assert!((off - 452.0).abs() < 3.0);
        assert!((hard - 440.0).abs() < 3.0, "hard {hard}");
        assert!(soft < 449.0 && soft > 440.0, "soft {soft}");
        // a note that is already right stays right
        assert!((run(440.0, 2) - 440.0).abs() < 2.0);
    }
}
