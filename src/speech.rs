//! Speech recognition for «Озвучка» («Нейросети» add-on): whisper.cpp (whisper-cli.exe next
//! to the program) with the Whisper base model finds every phrase of a clip — start, end, text.
//! Also reads subtitles (.srt / .vtt), which work without the add-on.

use std::path::PathBuf;
#[cfg(test)]
use std::path::Path;
use std::process::Command;

pub const MODEL_FILE: &str = "ggml-base.bin";

/// A recognised phrase: seconds and text.
#[derive(Clone, Debug, PartialEq)]
pub struct Phrase { pub start: f32, pub end: f32, pub text: String }

/// whisper-cli next to the program (tests may point KJ_WHISPER elsewhere).
pub fn exe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KJ_WHISPER").map(PathBuf::from).filter(|p| p.exists()) { return Some(p); }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [dir.join("whisper-cli.exe"), dir.join("whisper-cli")].into_iter().find(|p| p.exists())
}

/// The model, in «models» next to the program (it comes with the «Нейросети» add-on).
pub fn model() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KJ_WHISPER_MODEL").map(PathBuf::from).filter(|p| p.exists()) { return Some(p); }
    let p = std::env::current_exe().ok()?.parent()?.join("models").join(MODEL_FILE);
    p.exists().then_some(p)
}

/// The tool and its model are both there.
pub fn available() -> bool { exe().is_some() && model().is_some() }

/// Find the phrases in a sound (any rate; mono). Runs whisper-cli on a 16 kHz WAV.
pub fn transcribe(x: &[f32], sr: u32) -> Result<Vec<Phrase>, String> {
    let cli = exe().ok_or("Не найден whisper-cli.exe — он ставится с дополнением «Нейросети»")?;
    let model = model().ok_or("Не найдена модель распознавания (папка models рядом с программой)")?;
    let tmp = std::env::temp_dir().join(format!("kj_speech_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let wav = tmp.join("in.wav");
    let x16 = crate::dub::resample(x, sr, 16000);
    crate::audio::write_wav(&wav, &x16, 16000)?;
    let out = tmp.join("out");
    let threads = std::thread::available_parallelism().map(|n| n.get().min(8)).unwrap_or(4).to_string();
    let mut c = Command::new(cli);
    c.args(["-m"]).arg(&model).args(["-f"]).arg(&wav)
        .args(["-l", "ru", "-t", &threads, "-ml", "80", "-sow", "-osrt", "-np", "-of"]).arg(&out);
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; c.creation_flags(0x0800_0000); }
    let res = c.output().map_err(|e| format!("Не удалось запустить распознавание: {e}"))?;
    let srt = std::fs::read_to_string(tmp.join("out.srt"));
    let _ = std::fs::remove_dir_all(&tmp);
    if !res.status.success() {
        let err = String::from_utf8_lossy(&res.stderr);
        return Err(format!("Распознавание не удалось: {}", err.lines().last().unwrap_or("ошибка").trim()));
    }
    Ok(merge_short(parse_subtitles(&srt.map_err(|e| e.to_string())?)))
}

/// SRT or WebVTT → phrases. Tolerant: numbers, styles and empty cues are skipped.
pub fn parse_subtitles(t: &str) -> Vec<Phrase> {
    fn ts(s: &str) -> Option<f32> {
        // 00:01:02,345 / 01:02.345 / 00:01:02.345
        let s = s.trim().replace(',', ".");
        let parts: Vec<&str> = s.split(':').collect();
        let (h, m, sec) = match parts.as_slice() {
            [h, m, s] => (h.parse::<f32>().ok()?, m.parse::<f32>().ok()?, s.parse::<f32>().ok()?),
            [m, s] => (0.0, m.parse::<f32>().ok()?, s.parse::<f32>().ok()?),
            _ => return None,
        };
        Some(h * 3600.0 + m * 60.0 + sec)
    }
    let t = t.replace("\r\n", "\n");
    let mut out = Vec::new();
    for block in t.split("\n\n") {
        let lines: Vec<&str> = block.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        let Some(k) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let mut it = lines[k].split("-->");
        let (Some(a), Some(b)) = (it.next().and_then(ts), it.next().and_then(|x| ts(x.split_whitespace().next().unwrap_or("")))) else { continue };
        let text: String = lines[k + 1..].join(" ");
        // strip <i>…</i>, {\an8} and the like
        let mut clean = String::new();
        let mut skip = 0;
        for ch in text.chars() {
            match ch { '<' | '{' => skip += 1, '>' | '}' if skip > 0 => skip -= 1, _ if skip == 0 => clean.push(ch), _ => {} }
        }
        let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
        // whisper marks music / non-speech like [музыка] or (смех)
        let only_tag = clean.starts_with(['[', '(', '♪']) && clean.ends_with([']', ')', '♪']);
        if b > a && !clean.is_empty() && !only_tag { out.push(Phrase { start: a, end: b, text: clean }); }
    }
    out.sort_by(|x, y| x.start.total_cmp(&y.start));
    out
}

/// Glue phrases that are too short to dub on their own to the previous one when they nearly touch.
pub fn merge_short(v: Vec<Phrase>) -> Vec<Phrase> {
    let mut out: Vec<Phrase> = Vec::new();
    for p in v {
        if let Some(last) = out.last_mut() {
            let short = p.end - p.start < 0.6 || last.end - last.start < 0.6;
            // whisper cut a sentence in the middle (length limit): put it back together
            let unfinished = !last.text.trim_end().ends_with(['.', '!', '?', '…', '»', '"']);
            if (short || unfinished) && p.start - last.end < 0.25 && p.end - last.start < 10.0 {
                last.end = p.end;
                last.text = format!("{} {}", last.text, p.text);
                continue;
            }
        }
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_srt_and_vtt() {
        let srt = "1\r\n00:00:01,000 --> 00:00:03,500\r\n<i>Ты кто такой?</i>\r\n\r\n2\r\n00:00:04,000 --> 00:00:04,300\r\nА?\r\n\r\n3\r\n00:00:05,000 --> 00:00:06,000\r\n[музыка]\r\n";
        let v = parse_subtitles(srt);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], Phrase { start: 1.0, end: 3.5, text: "Ты кто такой?".into() });
        let vtt = "WEBVTT\n\n00:10.500 --> 00:12.000 align:start\nПривет, {\\an8}болото!\n";
        let v = parse_subtitles(vtt);
        assert_eq!(v.len(), 1);
        assert!((v[0].start - 10.5).abs() < 1e-3 && v[0].text == "Привет, болото!");
        // a tiny phrase sticks to its neighbour
        let m = merge_short(parse_subtitles(srt));
        assert_eq!(m.len(), 2);
        let m = merge_short(vec![Phrase { start: 0.0, end: 2.0, text: "Раз".into() }, Phrase { start: 2.1, end: 2.4, text: "два".into() }]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].text, "Раз два");
    }

    /// Needs whisper-cli (KJ_WHISPER) and the model (KJ_WHISPER_MODEL); skipped otherwise.
    #[test]
    fn transcribes_speech() {
        if !available() { eprintln!("no whisper — skipped"); return; }
        let Ok(wav) = std::env::var("KJ_WHISPER_SAMPLE") else { return };
        let (x, sr) = crate::audio::load_mono(Path::new(&wav), 60.0).unwrap();
        let v = transcribe(&x, sr).unwrap();
        println!("{v:#?}");
        assert!(!v.is_empty());
    }
}
