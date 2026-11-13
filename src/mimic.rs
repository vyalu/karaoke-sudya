//! «Пародист»: how close is an imitation to the original phrase?
//! Compares the *way* it is said, not the words:
//!  - intonation: pitch contour relative to each speaker's own voice,
//!  - rhythm: loudness envelope and voiced/unvoiced pattern, time-aligned with DTW,
//!  - tempo: overall duration.

use crate::dsp::{self, MonoResampler};

const SR: u32 = 16000;
const HOP: usize = 160; // 10 ms
const WIN: usize = 640; // 40 ms

#[derive(Clone, Copy, Debug, Default)]
pub struct MimicScore { pub total: u32, pub intonation: u32, pub rhythm: u32, pub tempo: u32 }

#[derive(Clone, Debug, Default)]
pub struct Features {
    pub energy: Vec<f32>,        // 0..1 (relative to the loudest frame)
    pub pitch: Vec<Option<f32>>, // semitones relative to the speaker's median
}
impl Features {
    pub fn len(&self) -> usize { self.energy.len() }
    pub fn voiced(&self) -> usize { self.pitch.iter().filter(|p| p.is_some()).count() }
    pub fn seconds(&self) -> f32 { self.len() as f32 * HOP as f32 / SR as f32 }
}

pub fn features(x: &[f32], sr: u32) -> Features {
    // to 16 kHz (speech is plenty there and pitch tracking gets cheap)
    let mut y = Vec::with_capacity(x.len() * SR as usize / sr.max(1) as usize + 8);
    let mut rs = MonoResampler::new(sr, SR);
    for &s in x { rs.push(s, &mut |v| y.push(v)); }
    if y.len() < WIN * 2 { return Features::default(); }
    let n = (y.len() - WIN) / HOP;
    let mut db = Vec::with_capacity(n);
    let mut pit: Vec<Option<f32>> = Vec::with_capacity(n);
    for i in 0..n {
        let w = &y[i * HOP..i * HOP + WIN];
        let r = dsp::rms(w);
        db.push(20.0 * (r + 1e-6).log10());
        pit.push(if r > 0.004 { dsp::pitch(w, SR as f32, 0.72).filter(|(f, _)| *f > 65.0 && *f < 600.0).map(|(f, _)| dsp::to_midi(f)) } else { None });
    }
    // trim silence at both ends
    let max_db = db.iter().cloned().fold(f32::MIN, f32::max);
    let thr = (max_db - 30.0).max(-52.0);
    let first = db.iter().position(|&d| d > thr).unwrap_or(0).saturating_sub(2);
    let last = (db.iter().rposition(|&d| d > thr).unwrap_or(n.saturating_sub(1)) + 3).min(n);
    if last <= first + 2 { return Features::default(); }
    let db = &db[first..last];
    let mut pit: Vec<Option<f32>> = pit[first..last].to_vec();
    // remove octave jumps and isolated blips, then median-smooth
    for i in 1..pit.len() {
        if let (Some(prev), Some(cur)) = (pit[i - 1], pit[i]) {
            let mut c = cur;
            while c - prev > 8.0 { c -= 12.0; }
            while prev - c > 8.0 { c += 12.0; }
            pit[i] = Some(c);
        }
    }
    for i in 1..pit.len().saturating_sub(1) {
        if pit[i].is_some() && pit[i - 1].is_none() && pit[i + 1].is_none() { pit[i] = None; }
    }
    let smooth: Vec<Option<f32>> = (0..pit.len()).map(|i| {
        let p = pit[i]?;
        let mut v: Vec<f32> = (i.saturating_sub(2)..(i + 3).min(pit.len())).filter_map(|k| pit[k]).collect();
        v.sort_by(|a, b| a.total_cmp(b));
        Some(if v.len() >= 3 { v[v.len() / 2] } else { p })
    }).collect();
    let mut voiced: Vec<f32> = smooth.iter().flatten().copied().collect();
    voiced.sort_by(|a, b| a.total_cmp(b));
    let med = if voiced.is_empty() { 0.0 } else { voiced[voiced.len() / 2] };
    Features {
        energy: db.iter().map(|&d| ((d - max_db) / 30.0 + 1.0).clamp(0.0, 1.0)).collect(),
        pitch: smooth.into_iter().map(|p| p.map(|m| (m - med).clamp(-12.0, 12.0))).collect(),
    }
}

/// Compare an imitation with the reference.
pub fn compare(reference: &Features, attempt: &Features) -> MimicScore {
    let (n, m) = (reference.len(), attempt.len());
    if n < 20 || m < 20 || attempt.voiced() < 12 { return MimicScore::default(); }
    // tempo
    let ratio = m as f32 / n as f32;
    let tempo = (-ratio.ln().abs() * 2.2).exp();
    // DTW with a band around the diagonal
    let band = ((n.max(m) as f32 * 0.35) as usize).max(n.abs_diff(m) + 5);
    let cost = |i: usize, j: usize| -> f32 {
        let e = (reference.energy[i] - attempt.energy[j]).abs();
        let v = match (reference.pitch[i], attempt.pitch[j]) {
            (Some(a), Some(b)) => (a - b).abs().min(12.0) / 8.0,
            (None, None) => 0.0,
            _ => 0.45,
        };
        e + v
    };
    let inf = f32::INFINITY;
    let mut d = vec![inf; (n + 1) * (m + 1)];
    let idx = |i: usize, j: usize| i * (m + 1) + j;
    d[idx(0, 0)] = 0.0;
    for i in 1..=n {
        let jc = (i as f32 * m as f32 / n as f32) as usize;
        let (j0, j1) = (jc.saturating_sub(band).max(1), (jc + band).min(m));
        for j in j0..=j1 {
            let best = d[idx(i - 1, j - 1)].min(d[idx(i - 1, j)] + 0.05).min(d[idx(i, j - 1)] + 0.05);
            if best.is_finite() { d[idx(i, j)] = cost(i - 1, j - 1) + best; }
        }
    }
    // walk the path back and gather the pieces
    let (mut i, mut j) = (n, m);
    if !d[idx(n, m)].is_finite() { return MimicScore { tempo: (tempo * 100.0) as u32, ..Default::default() }; }
    let (mut e_sum, mut steps, mut p_sum, mut p_n, mut v_agree) = (0f32, 0usize, 0f32, 0usize, 0usize);
    while i > 0 && j > 0 {
        let (a, b) = (i - 1, j - 1);
        e_sum += (reference.energy[a] - attempt.energy[b]).abs();
        match (reference.pitch[a], attempt.pitch[b]) {
            (Some(x), Some(y)) => { p_sum += (x - y).abs().min(12.0); p_n += 1; v_agree += 1; }
            (None, None) => v_agree += 1,
            _ => {}
        }
        steps += 1;
        let diag = d[idx(i - 1, j - 1)];
        let up = d[idx(i - 1, j)] + 0.05;
        let left = d[idx(i, j - 1)] + 0.05;
        if diag <= up && diag <= left { i -= 1; j -= 1; } else if up <= left { i -= 1; } else { j -= 1; }
    }
    let steps = steps.max(1) as f32;
    let e_avg = e_sum / steps;
    let agree = v_agree as f32 / steps;
    let rhythm = ((-e_avg / 0.22).exp() * 0.6 + agree * 0.4).clamp(0.0, 1.0);
    let intonation = if p_n >= 10 { (-(p_sum / p_n as f32) / 2.6).exp() } else { 0.0 };
    let raw = 0.45 * intonation + 0.35 * rhythm + 0.20 * tempo;
    // spread the scale a little: a decent imitation ≈ 65–85
    let total = (100.0 * raw.powf(1.25)).round().clamp(0.0, 100.0) as u32;
    MimicScore { total, intonation: (intonation * 100.0).round() as u32, rhythm: (rhythm * 100.0).round() as u32, tempo: (tempo * 100.0).round() as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Speech-like test signal: syllables with a pitch contour.
    /// `syl` = (start, length) in seconds; `contour(t01)` = semitones over base.
    fn speech(sr: u32, base_hz: f32, stretch: f32, syl: &[(f32, f32)], contour: impl Fn(f32) -> f32, seed: u32) -> Vec<f32> {
        let total = (syl.last().map(|s| s.0 + s.1).unwrap_or(1.0) * stretch + 0.6) * sr as f32;
        let mut out = vec![0f32; total as usize];
        let mut r = seed | 1;
        let mut noise = || { r ^= r << 13; r ^= r >> 17; r ^= r << 5; (r as f32 / u32::MAX as f32 - 0.5) * 0.004 };
        for o in out.iter_mut() { *o = noise(); }
        let end = syl.last().map(|s| s.0 + s.1).unwrap_or(1.0);
        let mut ph = 0f32;
        for &(st, len) in syl {
            let (a, b) = (((st * stretch + 0.3) * sr as f32) as usize, (((st + len) * stretch + 0.3) * sr as f32) as usize);
            for k in a..b.min(out.len()) {
                let t_orig = ((k as f32 / sr as f32) - 0.3) / stretch;
                let f = base_hz * 2f32.powf(contour(t_orig / end) / 12.0);
                ph = (ph + f / sr as f32) % 1.0;
                let env = ((k - a) as f32 / (sr as f32 * 0.02)).min(1.0) * ((b - k) as f32 / (sr as f32 * 0.03)).min(1.0);
                let v: f32 = (1..8).map(|h| (std::f32::consts::TAU * ph * h as f32).sin() / h as f32).sum();
                out[k] += v * 0.2 * env;
            }
        }
        out
    }

    #[test]
    fn imitation_scores_make_sense() {
        let sr = 48000;
        // «Вы кто такие? Я вас не звал…»: rises to a question, pause, falls
        let syl = [(0.0, 0.18), (0.24, 0.16), (0.44, 0.2), (0.7, 0.28), (1.35, 0.15), (1.55, 0.18), (1.78, 0.14), (1.98, 0.35)];
        let contour = |t: f32| if t < 0.45 { t * 14.0 } else { 6.0 - (t - 0.45) * 16.0 };
        let reference = features(&speech(sr, 115.0, 1.0, &syl, contour, 1), sr);
        // good imitation: higher voice, a bit slower, same melody & rhythm
        let good = features(&speech(sr, 190.0, 1.12, &syl, |t| contour(t) + 0.4 * (t * 40.0).sin(), 2), sr);
        // monotone: right rhythm, flat pitch
        let flat = features(&speech(sr, 150.0, 1.0, &syl, |_| 0.0, 3), sr);
        // wrong: different rhythm, opposite melody, much faster
        let syl2 = [(0.0, 0.1), (0.12, 0.1), (0.25, 0.1), (0.38, 0.1), (0.5, 0.1)];
        let wrong = features(&speech(sr, 140.0, 1.0, &syl2, |t| 6.0 - t * 12.0, 4), sr);
        // silence / noise
        let quiet = features(&vec![0.001f32; sr as usize * 2], sr);

        let s_good = compare(&reference, &good);
        let s_flat = compare(&reference, &flat);
        let s_wrong = compare(&reference, &wrong);
        let s_quiet = compare(&reference, &quiet);
        println!("ref {:.2}s voiced {}", reference.seconds(), reference.voiced());
        println!("good  {:?}", s_good);
        println!("flat  {:?}", s_flat);
        println!("wrong {:?}", s_wrong);
        println!("quiet {:?}", s_quiet);
        assert!(s_good.total >= 70, "good imitation too low");
        assert!(s_good.total > s_flat.total + 10, "intonation must matter");
        assert!(s_flat.total > s_wrong.total, "rhythm must matter");
        assert!(s_wrong.total < 45);
        assert_eq!(s_quiet.total, 0);
    }
}
