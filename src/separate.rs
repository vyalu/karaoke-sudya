//! Taking the voices out of a clip («Нейросети» add-on): kj-demucs.exe next to the program runs
//! Demucs (htdemucs, Meta, MIT) through demucs.cpp. The result — the clip's sound without
//! voices — is kept next to the clip as «<clip>.bg» (a WAV) so it is done once.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const MODEL_FILE: &str = "ggml-htdemucs-4s-f16.bin";
const SR: u32 = 44100;

pub fn exe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KJ_DEMUCS").map(PathBuf::from).filter(|p| p.exists()) { return Some(p); }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [dir.join("kj-demucs.exe"), dir.join("kj-demucs")].into_iter().find(|p| p.exists())
}

pub fn model() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KJ_DEMUCS_MODEL").map(PathBuf::from).filter(|p| p.exists()) { return Some(p); }
    let p = std::env::current_exe().ok()?.parent()?.join("models").join(MODEL_FILE);
    p.exists().then_some(p)
}

/// The tool and its model are both there.
pub fn available() -> bool { exe().is_some() && model().is_some() }

pub fn cache_path(scene: &Path) -> PathBuf {
    scene.with_extension(format!("{}.bg", scene.extension().and_then(|e| e.to_str()).unwrap_or("")))
}

/// The saved background, if it still fits the clip (same length — a cut clip makes it stale).
pub fn load_cached(scene: &Path, len: usize, sr: u32) -> Option<Vec<f32>> {
    let r = hound::WavReader::open(cache_path(scene)).ok()?;
    if r.spec().sample_rate != sr { return None; }
    let v: Vec<f32> = r.into_samples::<f32>().flatten().collect();
    (v.len().abs_diff(len) <= (sr / 20) as usize).then(|| { let mut v = v; v.resize(len, 0.0); v })
}

pub fn save_cache(scene: &Path, bg: &[f32], sr: u32) {
    let spec = hound::WavSpec { channels: 1, sample_rate: sr, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    if let Ok(mut w) = hound::WavWriter::create(cache_path(scene), spec) {
        for s in bg { let _ = w.write_sample(*s); }
        let _ = w.finalize();
    }
}

/// mid/side (as the program keeps a clip) → the background without voices, mid only, same rate.
/// `progress` gets 0..1.
pub fn background(mid: &[f32], side: &[f32], sr: u32, progress: &dyn Fn(f32)) -> Result<Vec<f32>, String> {
    let cli = exe().ok_or("нет kj-demucs.exe")?;
    let model = model().ok_or("нет модели отделения голосов (папка models рядом с программой)")?;
    let l: Vec<f32> = mid.iter().zip(side.iter().chain(std::iter::repeat(&0.0))).map(|(m, s)| m + s).collect();
    let r: Vec<f32> = mid.iter().zip(side.iter().chain(std::iter::repeat(&0.0))).map(|(m, s)| m - s).collect();
    let (l, r) = (crate::dub::resample(&l, sr, SR), crate::dub::resample(&r, sr, SR));
    let tmp = std::env::temp_dir().join(format!("kj_sep_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let (inp, out) = (tmp.join("in.f32"), tmp.join("out.f32"));
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&inp).map_err(|e| e.to_string())?);
        for i in 0..l.len() { f.write_all(&l[i].to_le_bytes()).and_then(|_| f.write_all(&r[i].to_le_bytes())).map_err(|e| e.to_string())?; }
    }
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
    // short clips: no point in more pieces than ~8 s each
    let threads = threads.min((l.len() / (SR as usize * 8)).max(1));
    let mut c = Command::new(cli);
    c.arg(&model).arg(&inp).arg(&out).arg(threads.to_string()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; c.creation_flags(0x0800_0000); }
    let mut child = c.spawn().map_err(|e| format!("Не удалось запустить отделение голосов: {e}"))?;
    // "[THREAD i] ( 12.345%) …" — the average over the threads
    let mut per = vec![0f32; threads];
    if let Some(so) = child.stdout.take() {
        for line in BufReader::new(so).lines().map_while(Result::ok) {
            let th = line.split("[THREAD ").nth(1).and_then(|x| x.split(']').next()).and_then(|x| x.trim().parse::<usize>().ok());
            let pc = line.split('(').nth(1).and_then(|x| x.split('%').next()).and_then(|x| x.trim().parse::<f32>().ok());
            if let (Some(t), Some(p)) = (th, pc) { if t < per.len() { per[t] = p / 100.0; progress(per.iter().sum::<f32>() / per.len() as f32); } }
        }
    }
    let mut err = String::new();
    if let Some(mut se) = child.stderr.take() { let _ = se.read_to_string(&mut err); }
    let st = child.wait().map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&out);
    let _ = std::fs::remove_dir_all(&tmp);
    if !st.success() { return Err(format!("Отделение голосов не удалось: {}", err.lines().last().unwrap_or("ошибка").trim())); }
    let bytes = bytes.map_err(|e| e.to_string())?;
    let mono: Vec<f32> = bytes.chunks_exact(8).map(|c| {
        let a = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        let b = f32::from_le_bytes([c[4], c[5], c[6], c[7]]);
        (a + b) * 0.5
    }).collect();
    let mut bg = crate::dub::resample(&mono, SR, sr);
    bg.resize(mid.len(), 0.0);
    Ok(bg)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Needs KJ_DEMUCS, KJ_DEMUCS_MODEL and a clip in KJ_SEP_SAMPLE; skipped otherwise.
    #[test]
    fn takes_voices_out() {
        let Ok(f) = std::env::var("KJ_SEP_SAMPLE") else { return };
        if !available() { return; }
        let (m, s, sr) = crate::audio::load_mid_side(Path::new(&f), 60.0).unwrap();
        let t0 = std::time::Instant::now();
        let bg = background(&m, &s, sr, &|p| eprint!("\r{:.0}%", p * 100.0)).unwrap();
        println!("\n{:.1} s for {:.1} s of sound", t0.elapsed().as_secs_f32(), m.len() as f32 / sr as f32);
        assert_eq!(bg.len(), m.len());
        crate::audio::write_wav(Path::new("/tmp/v2/bg.wav"), &bg, sr).unwrap();
    }
}
