//! Crowd sounds, synthesized (no audio files): applause, a sympathetic «ооо»,
//! and an excited whistle.

pub const SR: u32 = 44100;

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 17; self.0 ^= self.0 << 5; self.0 as f32 / u32::MAX as f32 }
    fn bi(&mut self) -> f32 { self.next() * 2.0 - 1.0 }
}

/// Band-pass biquad (constant skirt gain), RBJ cookbook.
struct Biquad { b0: f32, b2: f32, a1: f32, a2: f32, x1: f32, x2: f32, y1: f32, y2: f32 }
impl Biquad {
    fn bandpass(f: f32, q: f32) -> Self {
        let w = std::f32::consts::TAU * f / SR as f32;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self { b0: alpha / a0, b2: -alpha / a0, a1: -2.0 * w.cos() / a0, a2: (1.0 - alpha) / a0, x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1; self.x1 = x; self.y2 = self.y1; self.y1 = y;
        y
    }
}

fn normalize(v: &mut [f32], peak: f32) {
    let m = v.iter().fold(0f32, |a, x| a.max(x.abs()));
    if m > 0.0 { for x in v.iter_mut() { *x *= peak / m; } }
}

/// Applause: many people clapping. `crowd` ≈ how many (5 polite … 40 ovation).
/// Dense, soft-edged claps + a room reverb, so no single clap pops out.
pub fn applause(secs: f32, crowd: usize, seed: u32) -> Vec<f32> {
    let n = (secs * SR as f32) as usize;
    let mut dry = vec![0f32; n];
    let mut r = Rng(seed | 1);
    let people = crowd * 6 + 20;
    let attack = (SR as f32 * 0.0015) as usize;
    for _ in 0..people {
        let rate = 3.0 + r.next() * 3.5;
        let mut t = r.next() * 0.35;
        // a person's hands: one main resonance + a softer low «body»
        let tone = 700.0 + r.next() * 1300.0;
        let loud = 0.3 + r.next() * 0.7;
        let tau = SR as f32 * (0.006 + r.next() * 0.008);
        let len = (tau * 5.0) as usize;
        while t < secs {
            let start = (t * SR as f32) as usize;
            let mut bp = Biquad::bandpass(tone * (0.9 + r.next() * 0.2), 1.4);
            let mut body = Biquad::bandpass(tone * 0.45, 2.0);
            let g = loud * (0.6 + r.next() * 0.4);
            for k in 0..len.min(n.saturating_sub(start)) {
                let a = if k < attack { 0.5 - 0.5 * (std::f32::consts::PI * k as f32 / attack as f32).cos() } else { 1.0 };
                let env = a * (-((k.saturating_sub(attack)) as f32) / tau).exp();
                let x = r.bi();
                dry[start + k] += (bp.run(x) + 0.5 * body.run(x)) * env * g;
            }
            t += (1.0 / rate) * (0.75 + r.next() * 0.5);
        }
    }
    // soften the top end (a real hall swallows the clicks)
    let mut lp = 0f32;
    let k_lp = 1.0 - (-std::f32::consts::TAU * 5000.0 / SR as f32).exp();
    for x in dry.iter_mut() { lp += (*x - lp) * k_lp; *x = lp; }
    // small-hall reverb: 4 combs in parallel → 2 allpasses (Schroeder)
    let mut wet = vec![0f32; n];
    for (d, fb) in [(1557usize, 0.80f32), (1617, 0.79), (1491, 0.81), (1422, 0.78)] {
        let mut buf = vec![0f32; d];
        let mut idx = 0;
        let mut damp = 0f32;
        for i in 0..n {
            let y = buf[idx];
            damp = y * 0.55 + damp * 0.45;
            buf[idx] = dry[i] + damp * fb;
            idx = (idx + 1) % d;
            wet[i] += y * 0.25;
        }
    }
    for d in [556usize, 441] {
        let mut buf = vec![0f32; d];
        let mut idx = 0;
        for x in wet.iter_mut() {
            let b = buf[idx];
            let y = -*x * 0.5 + b;
            buf[idx] = *x + b * 0.5;
            idx = (idx + 1) % d;
            *x = y;
        }
    }
    let mut out: Vec<f32> = dry.iter().zip(&wet).map(|(d, w)| d * 0.7 + w * 0.6).collect();
    // swell in, fade out
    for (i, x) in out.iter_mut().enumerate() {
        let t = i as f32 / SR as f32;
        *x *= (t / 0.35).min(1.0) * ((secs - t) / 1.2).clamp(0.0, 1.0);
    }
    // level by loudness, not by the loudest clap; tame any leftover peak
    let rms = (out.iter().map(|x| x * x).sum::<f32>() / n.max(1) as f32).sqrt();
    if rms > 0.0 { for x in out.iter_mut() { *x = (*x * 0.14 / rms * 1.6).tanh() / 1.6; } }
    out
}

/// Sympathetic crowd «ооо» with a falling pitch.
pub fn aww(secs: f32, seed: u32) -> Vec<f32> {
    let n = (secs * SR as f32) as usize;
    let mut out = vec![0f32; n];
    let mut r = Rng(seed | 1);
    for _ in 0..14 {
        let f0 = 130.0 + r.next() * 170.0;
        let vib = 4.5 + r.next() * 2.0;
        let delay = r.next() * 0.15;
        let mut ph = 0f32;
        // vowel «о/у»: formants ~420 and ~820 Hz
        let (mut f1, mut f2) = (Biquad::bandpass(420.0, 6.0), Biquad::bandpass(820.0, 7.0));
        for i in 0..n {
            let t = i as f32 / SR as f32 - delay;
            if t < 0.0 { continue; }
            let glide = 1.0 - 0.18 * (t / secs);
            let f = f0 * glide * (1.0 + 0.008 * (std::f32::consts::TAU * vib * t).sin());
            ph = (ph + f / SR as f32) % 1.0;
            let saw = 2.0 * ph - 1.0;
            let v = f1.run(saw) * 1.0 + f2.run(saw) * 0.5;
            let env = (t / 0.18).min(1.0) * ((secs - delay - t) / 0.6).clamp(0.0, 1.0);
            out[i] += v * env;
        }
    }
    normalize(&mut out, 0.45);
    out
}

/// Two excited whistles (for top scores).
pub fn whistle(seed: u32) -> Vec<f32> {
    let secs = 1.6;
    let n = (secs * SR as f32) as usize;
    let mut out = vec![0f32; n];
    let mut r = Rng(seed | 1);
    for w in 0..2 {
        let start = w as f32 * 0.55 + r.next() * 0.1;
        let (lo, hi) = (1500.0 + r.next() * 300.0, 2600.0 + r.next() * 400.0);
        let mut ph = 0f32;
        for i in 0..n {
            let t = i as f32 / SR as f32 - start;
            if !(0.0..0.75).contains(&t) { continue; }
            // up-sweep, then a quick drop
            let k = t / 0.75;
            let f = if k < 0.6 { lo + (hi - lo) * (k / 0.6) } else { hi - (hi - lo) * 0.5 * ((k - 0.6) / 0.4) };
            let f = f * (1.0 + 0.01 * (std::f32::consts::TAU * 9.0 * t).sin());
            ph = (ph + f / SR as f32) % 1.0;
            let env = (t / 0.04).min(1.0) * ((0.75 - t) / 0.12).clamp(0.0, 1.0);
            out[i] += (std::f32::consts::TAU * ph).sin() * env * 0.5 + r.bi() * env * 0.02;
        }
    }
    normalize(&mut out, 0.4);
    out
}

/// The reaction of the hall to a final score.
pub fn for_score(score: u32, seed: u32) -> Option<Vec<f32>> {
    match score {
        0 => None,
        90.. => { let mut a = applause(4.5, 40, seed); for (x, y) in a.iter_mut().zip(whistle(seed ^ 77)) { *x += y; } Some(a) }
        75.. => Some(applause(3.5, 28, seed)),
        55.. => Some(applause(2.5, 9, seed)),
        _ => Some(aww(1.8, seed)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rms(v: &[f32]) -> f32 { (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt() }
    #[test]
    fn sounds_are_sane() {
        for (name, v) in [("applause", applause(3.0, 20, 5)), ("aww", aww(1.8, 5)), ("whistle", whistle(5))] {
            let peak = v.iter().fold(0f32, |a, x| a.max(x.abs()));
            println!("{name}: {:.1}s rms {:.3} peak {:.2}", v.len() as f32 / SR as f32, rms(&v), peak);
            assert!(peak <= 0.61 && peak > 0.3);
            assert!(rms(&v) > 0.02);
            assert!(v.iter().all(|x| x.is_finite()));
        }
        assert!(for_score(0, 1).is_none());
    }

    #[test]
    fn applause_has_no_pops() {
        let a = super::applause(2.5, 20, 9);
        // judge the steady middle part: peak vs loudness (crest factor)
        let m = &a[a.len() / 5..a.len() / 2];
        let rms = (m.iter().map(|x| x * x).sum::<f32>() / m.len() as f32).sqrt();
        let peak = m.iter().fold(0f32, |p, x| p.max(x.abs()));
        println!("applause rms {rms:.3} peak {peak:.3} crest {:.2}", peak / rms);
        assert!(peak / rms < 4.0, "claps pop out: crest {}", peak / rms);
        assert!(rms > 0.08);
    }
}
