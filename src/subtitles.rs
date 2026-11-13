//! Subtitles (.srt / .vtt) next to a clip become its moments, with the text for the prompter.

/// A subtitle: seconds and text.
#[derive(Clone, Debug, PartialEq)]
pub struct Phrase { pub start: f32, pub end: f32, pub text: String }

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
        // a cue that is only a tag like [музыка] or (смех) is not a line
        let only_tag = clean.starts_with(['[', '(', '♪']) && clean.ends_with([']', ')', '♪']);
        if b > a && !clean.is_empty() && !only_tag { out.push(Phrase { start: a, end: b, text: clean }); }
    }
    out.sort_by(|x, y| x.start.total_cmp(&y.start));
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
    }
}
