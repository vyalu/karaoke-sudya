//! A small ffmpeg (LGPL, shipped next to the app) does the video work: cutting a clip to the
//! frame (re-encoded with Windows' own H.264 encoder) and putting the dub onto the picture.

use std::path::{Path, PathBuf};
use std::process::Command;

/// ffmpeg.exe next to the program (tests may point KJ_FFMPEG elsewhere).
pub fn exe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KJ_FFMPEG").map(PathBuf::from).filter(|p| p.exists()) { return Some(p); }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [dir.join("ffmpeg.exe"), dir.join("ffmpeg")].into_iter().find(|p| p.exists())
}

fn run(args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let ff = exe().ok_or("Не найден ffmpeg.exe — он должен лежать рядом с программой")?;
    let mut c = Command::new(ff);
    c.args(["-hide_banner", "-loglevel", "error", "-y"]).args(args);
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; c.creation_flags(0x0800_0000); } // no console window
    let out = c.output().map_err(|e| format!("Не удалось запустить ffmpeg: {e}"))?;
    if out.status.success() { Ok(()) } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("ffmpeg: {}", err.lines().last().unwrap_or("ошибка").trim()))
    }
}

/// Copy `a..b` seconds of a clip into `dst` (same container). The picture starts on the
/// nearest key frame at or before `a`; nothing is re-encoded.
fn cut(src: &Path, dst: &Path, a: f32, b: f32) -> Result<(), String> {
    let (ss, t) = (format!("{:.3}", a.max(0.0)), format!("{:.3}", (b - a).max(0.1)));
    let mp4 = dst.extension().and_then(|e| e.to_str()).map(|e| matches!(e.to_lowercase().as_str(), "mp4" | "m4v" | "mov")).unwrap_or(false);
    let mut args: Vec<&std::ffi::OsStr> = vec!["-ss".as_ref(), ss.as_ref(), "-i".as_ref(), src.as_os_str(), "-t".as_ref(), t.as_ref(),
        "-map".as_ref(), "0:v?".as_ref(), "-map".as_ref(), "0:a?".as_ref(), "-c".as_ref(), "copy".as_ref(), "-avoid_negative_ts".as_ref(), "make_zero".as_ref()];
    if mp4 { args.push(std::ffi::OsStr::new("-movflags")); args.push(std::ffi::OsStr::new("+faststart")); }
    args.push(dst.as_os_str());
    run(&args)
}

/// How a clip was cut.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Cut {
    /// to the frame and the sample: the new clip is exactly the sum of the pieces
    Exact,
    /// without re-encoding: pieces start on key frames, so the lengths are only roughly right
    KeyFrames,
}

/// The video encoder for exact cuts: Windows' own H.264 (Media Foundation; tests: KJ_VENC).
fn venc() -> String { std::env::var("KJ_VENC").unwrap_or_else(|_| "h264_mf".into()) }

/// Keep only `pieces` (seconds, in order) of a clip and glue them into `dst` (an .mp4).
/// Tries the exact way first (decode, trim, re-encode); if that can't be done here
/// (no H.264 encoder in this Windows), falls back to key-frame cuts.
/// `kbps` — the bitrate to keep the picture as good as the source's.
pub fn cut_pieces(src: &Path, dst: &Path, pieces: &[(f32, f32)], kbps: u32) -> Result<Cut, String> {
    if pieces.is_empty() { return Err("Нечего сохранять — не осталось ни одного куска".into()); }
    match cut_exact(src, dst, pieces, kbps) {
        Ok(()) => Ok(Cut::Exact),
        Err(_) => { let _ = std::fs::remove_file(dst); cut_keyframes(src, dst, pieces).map(|_| Cut::KeyFrames) }
    }
}

/// trim + concat in one filter graph, picture and sound re-encoded.
fn cut_exact(src: &Path, dst: &Path, pieces: &[(f32, f32)], kbps: u32) -> Result<(), String> {
    let mut g = String::new();
    for (k, &(a, b)) in pieces.iter().enumerate() {
        g.push_str(&format!("[0:v:0]trim=start={a:.4}:end={b:.4},setpts=PTS-STARTPTS[v{k}];[0:a:0]atrim=start={a:.4}:end={b:.4},asetpts=PTS-STARTPTS[a{k}];"));
    }
    for k in 0..pieces.len() { g.push_str(&format!("[v{k}][a{k}]")); }
    g.push_str(&format!("concat=n={}:v=1:a=1[v][a]", pieces.len()));
    let br = format!("{}k", kbps.clamp(1500, 20000));
    let enc = venc();
    run(&["-i".as_ref(), src.as_os_str(), "-filter_complex".as_ref(), g.as_ref(), "-map".as_ref(), "[v]".as_ref(), "-map".as_ref(), "[a]".as_ref(),
          // concat counts in microseconds; the encoder gets the source's own time base
          "-c:v".as_ref(), enc.as_ref(), "-enc_time_base:v".as_ref(), "demux".as_ref(), "-b:v".as_ref(), br.as_ref(), "-c:a".as_ref(), "aac".as_ref(), "-b:a".as_ref(), "192k".as_ref(),
          "-movflags".as_ref(), "+faststart".as_ref(), dst.as_os_str()])
}

/// Stream-copy cuts glued by the concat demuxer (no re-encoding, key-frame accurate only).
fn cut_keyframes(src: &Path, dst: &Path, pieces: &[(f32, f32)]) -> Result<(), String> {
    if pieces.len() == 1 { return cut(src, dst, pieces[0].0, pieces[0].1); }
    let ext = dst.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_string();
    let tmp = std::env::temp_dir().join(format!("kj_cut_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let mut list = String::new();
    let r = (|| {
        for (k, &(a, b)) in pieces.iter().enumerate() {
            let part = tmp.join(format!("part{k}.{ext}"));
            cut(src, &part, a, b)?;
            list.push_str(&format!("file '{}'\n", part.to_string_lossy().replace('\\', "/").replace('\'', "'\\''")));
        }
        let lf = tmp.join("list.txt");
        std::fs::write(&lf, &list).map_err(|e| e.to_string())?;
        let mp4 = matches!(ext.to_lowercase().as_str(), "mp4" | "m4v" | "mov");
        let mut args: Vec<&std::ffi::OsStr> = vec!["-f".as_ref(), "concat".as_ref(), "-safe".as_ref(), "0".as_ref(), "-i".as_ref(), lf.as_os_str(), "-c".as_ref(), "copy".as_ref()];
        if mp4 { args.push(std::ffi::OsStr::new("-movflags")); args.push(std::ffi::OsStr::new("+faststart")); }
        args.push(dst.as_os_str());
        run(&args)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

/// The finished dub: the clip's picture with `audio` (a WAV) as its only sound → mp4.
pub fn dub_video(video: &Path, audio: &Path, dst: &Path) -> Result<(), String> {
    run(&["-i".as_ref(), video.as_os_str(), "-i".as_ref(), audio.as_os_str(),
          "-map".as_ref(), "0:v:0".as_ref(), "-map".as_ref(), "1:a:0".as_ref(),
          "-c:v".as_ref(), "copy".as_ref(), "-c:a".as_ref(), "aac".as_ref(), "-b:a".as_ref(), "192k".as_ref(),
          "-shortest".as_ref(), "-movflags".as_ref(), "+faststart".as_ref(), dst.as_os_str()])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Uses the minimal ffmpeg built for Linux (KJ_FFMPEG) and the system ffmpeg to make a clip.
    #[test]
    fn cut_and_dub() {
        if std::env::var_os("KJ_FFMPEG").is_none() { eprintln!("KJ_FFMPEG not set — skipped"); return; }
        let d = std::env::temp_dir().join("kj_ff_test");
        let _ = std::fs::create_dir_all(&d);
        let src = d.join("frag.mp4");
        let ok = std::process::Command::new("ffmpeg").args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=duration=8:size=320x180:rate=25",
            "-f", "lavfi", "-i", "sine=frequency=300:duration=8", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "25", "-bf", "2", "-c:a", "aac",
            "-movflags", "frag_keyframe+empty_moov+default_base_moof", "-shortest"]).arg(&src).status().map(|s| s.success()).unwrap_or(false);
        if !ok { eprintln!("no system ffmpeg — skipped"); return; }
        let cut = d.join("cut (обрезка).mp4");
        super::cut(&src, &cut, 2.4, 6.0).unwrap();
        let (s, sr) = crate::audio::load_mono(&cut, 60.0).unwrap();
        let secs = s.len() as f32 / sr as f32;
        assert!(secs > 3.4 && secs < 4.2, "{secs}"); // starts on the key frame at 2.0
        let wav = d.join("voice.wav");
        crate::audio::write_wav(&wav, &vec![0.1f32; 16000 * 3], 16000).unwrap();
        let out = d.join("dubbed.mp4");
        super::dub_video(&cut, &wav, &out).unwrap();
        let (s, sr) = crate::audio::load_mono(&out, 60.0).unwrap();
        assert!((s.len() as f32 / sr as f32 - 3.0).abs() < 0.2);
        assert!(Path::new(&out).metadata().unwrap().len() > 10_000);
    }

    #[test]
    fn keeps_only_the_pieces() {
        if std::env::var_os("KJ_FFMPEG").is_none() { return; }
        let d = std::env::temp_dir().join("kj_ff_pieces");
        let _ = std::fs::create_dir_all(&d);
        let src = d.join("src.mp4");
        let ok = std::process::Command::new("ffmpeg").args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=duration=12:size=320x180:rate=25",
            "-f", "lavfi", "-i", "anoisesrc=d=12:c=pink:a=0.3", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "25", "-c:a", "aac", "-shortest"]).arg(&src).status().map(|s| s.success()).unwrap_or(false);
        if !ok { return; }
        let out = d.join("кусочки 'тест'.mp4");
        let pieces = [(1.0, 3.0), (6.0, 8.0), (10.0, 11.0)];
        // exact: the native mpeg4 encoder stands in for Windows' H.264 here
        std::env::set_var("KJ_VENC", "mpeg4");
        assert_eq!(super::cut_pieces(&src, &out, &pieces, 2000).unwrap(), super::Cut::Exact);
        let (s, sr) = crate::audio::load_mono(&out, 60.0).unwrap();
        let secs = s.len() as f32 / sr as f32;
        assert!((secs - 5.0).abs() < 0.06, "{secs}");
        // the new sound is exactly the pieces, sample for sample (so the voiceless copy cut the
        // same way lines up with it)
        let (orig, osr) = crate::audio::load_mono(&src, 60.0).unwrap();
        assert_eq!(osr, sr);
        let glued: Vec<f32> = pieces.iter().flat_map(|&(a, b)| orig[(a * sr as f32) as usize..(b * sr as f32) as usize].to_vec()).collect();
        let lag = (-2048i32..=2048).max_by_key(|&l| {
            (0..sr as usize).step_by(7).map(|i| { let j = i as i32 + l; if j < 0 || j as usize >= s.len() { 0.0 } else { glued[i] * s[j as usize] } }).sum::<f32>().mul_add(1e6, 0.0) as i64
        }).unwrap();
        println!("lag {lag} samples, len {} vs {}", s.len(), glued.len());
        assert!(lag.abs() <= 2, "lag {lag}");
        // no such encoder (a Windows without H.264): key frames, roughly the same
        std::env::set_var("KJ_VENC", "no_such_encoder");
        assert_eq!(super::cut_pieces(&src, &out, &pieces, 2000).unwrap(), super::Cut::KeyFrames);
        let (s, sr) = crate::audio::load_mono(&out, 60.0).unwrap();
        let secs = s.len() as f32 / sr as f32;
        assert!((secs - 5.0).abs() < 0.4, "{secs}"); // 2 + 2 + 1 s (key frames every second)
    }
}
