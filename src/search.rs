//! YouTube song search (no API key): fetches the public results page and
//! reads the embedded `ytInitialData` JSON.

use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub duration: String,
    /// length in seconds; None for Shorts (≤ 60 s, length not shown in search)
    #[allow(dead_code)] // parsed and tested; not shown in the UI
    pub secs: Option<u32>,
}

/// "1:02:03" → 3723
pub fn parse_duration(d: &str) -> Option<u32> {
    let mut t = 0u32;
    for part in d.trim().split(':') { t = t * 60 + part.trim().parse::<u32>().ok()?; }
    Some(t)
}

pub fn watch_url(id: &str) -> String { format!("https://www.youtube.com/watch?v={id}") }

pub fn search_url(query: &str) -> String {
    format!("https://www.youtube.com/results?search_query={}", encode(&with_karaoke(query)))
}

pub fn with_karaoke(q: &str) -> String {
    let l = q.to_lowercase();
    if l.contains("караоке") || l.contains("karaoke") || l.contains("минус") { q.trim().to_string() } else { format!("{} караоке", q.trim()) }
}

pub fn encode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            b' ' => o.push('+'),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// Extract the JSON object that follows `ytInitialData`.
pub fn extract_initial_data(html: &str) -> Option<&str> {
    let key = html.find("ytInitialData")?;
    let start = key + html[key..].find('{')?;
    let bytes = html.as_bytes();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, &c) in bytes[start..].iter().enumerate() {
        if in_str {
            if esc { esc = false; } else if c == b'\\' { esc = true; } else if c == b'"' { in_str = false; }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => { depth -= 1; if depth == 0 { return Some(&html[start..start + i + 1]); } }
            _ => {}
        }
    }
    None
}

fn text(v: &Value) -> Option<String> {
    if let Some(s) = v.get("simpleText").and_then(|s| s.as_str()) { return Some(s.to_string()); }
    let runs = v.get("runs")?.as_array()?;
    Some(runs.iter().filter_map(|r| r.get("text").and_then(|t| t.as_str())).collect::<String>())
}

fn collect(v: &Value, out: &mut Vec<Video>, shorts: bool) {
    match v {
        Value::Object(map) => {
            if let Some(vr) = map.get("videoRenderer") {
                if let Some(id) = vr.get("videoId").and_then(|x| x.as_str()) {
                    if !out.iter().any(|x| x.id == id) {
                        let title = vr.get("title").and_then(text).unwrap_or_default();
                        let channel = vr.get("ownerText").and_then(text)
                            .or_else(|| vr.get("longBylineText").and_then(text)).unwrap_or_default();
                        let duration = vr.get("lengthText").and_then(text).unwrap_or_default();
                        if !title.is_empty() && !duration.is_empty() {
                            let secs = parse_duration(&duration);
                            out.push(Video { id: id.to_string(), title, channel, duration, secs });
                        }
                    }
                }
            }
            if shorts {
                // classic Shorts shelf item
                if let Some(r) = map.get("reelItemRenderer") {
                    if let Some(id) = r.get("videoId").and_then(|x| x.as_str()) {
                        let title = r.get("headline").and_then(text).unwrap_or_default();
                        if !title.is_empty() && !out.iter().any(|x| x.id == id) {
                            out.push(Video { id: id.to_string(), title, channel: String::new(), duration: "Shorts".into(), secs: None });
                        }
                    }
                }
                // newer Shorts lockup
                if let Some(r) = map.get("shortsLockupViewModel") {
                    let id = r.pointer("/onTap/innertubeCommand/reelWatchEndpoint/videoId").and_then(|x| x.as_str());
                    let title = r.pointer("/overlayMetadata/primaryText/content").and_then(|x| x.as_str()).unwrap_or("");
                    if let Some(id) = id {
                        if !title.is_empty() && !out.iter().any(|x| x.id == id) {
                            out.push(Video { id: id.to_string(), title: title.to_string(), channel: String::new(), duration: "Shorts".into(), secs: None });
                        }
                    }
                }
            }
            for (k, x) in map { if k != "videoRenderer" { collect(x, out, shorts); } }
        }
        Value::Array(a) => for x in a { collect(x, out, shorts); },
        _ => {}
    }
}

pub fn parse_results(html: &str) -> Result<Vec<Video>, String> { parse_results_ex(html, false) }

pub fn parse_results_ex(html: &str, shorts: bool) -> Result<Vec<Video>, String> {
    let json = extract_initial_data(html).ok_or("YouTube вернул неожиданную страницу")?;
    let v: Value = serde_json::from_str(json).map_err(|e| format!("Не удалось разобрать ответ YouTube: {e}"))?;
    let mut out = Vec::new();
    collect(&v, &mut out, shorts);
    out.truncate(24);
    Ok(out)
}

#[cfg(windows)]
fn agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(tls))
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36")
        .build())
}

#[cfg(windows)]
pub fn search(query: &str) -> Result<Vec<Video>, String> {
    let url = format!("{}&hl=ru&gl=RU", search_url(query));
    let resp = agent()?.get(&url)
        .set("Accept-Language", "ru-RU,ru;q=0.9,en;q=0.6")
        .set("Cookie", "CONSENT=YES+cb; SOCS=CAI")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(c, _) => format!("YouTube ответил ошибкой {c}"),
            ureq::Error::Transport(_) => "Нет связи с YouTube. Проверьте интернет.".to_string(),
        })?;
    let mut body = String::new();
    use std::io::Read;
    resp.into_reader().take(8 << 20).read_to_string(&mut body).map_err(|e| e.to_string())?;
    let res = parse_results(&body)?;
    if res.is_empty() { return Err("Ничего не найдено".into()); }
    Ok(res)
}

#[cfg(windows)]
pub fn thumbnail(id: &str) -> Result<Vec<u8>, String> {
    let resp = agent()?.get(&format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg")).call().map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    use std::io::Read;
    resp.into_reader().take(2 << 20).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

#[cfg(not(windows))]
pub fn search(_q: &str) -> Result<Vec<Video>, String> {
    // preview build only (the shipped app is Windows)
    std::thread::sleep(std::time::Duration::from_millis(300));
    Ok((0..6).map(|i| Video { id: format!("demo{i}"), title: ["Кино — Группа крови (караоке версия, оригинальный минус)", "Группа крови — караоке", "КИНО - Группа крови | Караоке с текстом", "Группа крови (Karaoke)", "Кино караоке хиты", "Цой — Группа крови минусовка"][i].into(), channel: ["Караоке Клуб", "Sing Along RU", "Минусовки", "Karaoke Hits", "Хиты", "Минус"][i].into(), duration: ["4:47", "4:50", "4:45", "4:44", "1:02:11", "4:46"][i].into(), secs: None }).collect())
}
#[cfg(not(windows))]
pub fn thumbnail(_id: &str) -> Result<Vec<u8>, String> { Err("n/a".into()) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_sample_page() {
        let html = r#"<html><script>var ytInitialData = {"contents":{"twoColumnSearchResultsRenderer":{"primaryContents":{"sectionListRenderer":{"contents":[{"itemSectionRenderer":{"contents":[
            {"videoRenderer":{"videoId":"abc123","title":{"runs":[{"text":"Кино — Группа крови (караоке) \"}{\""}]},"ownerText":{"runs":[{"text":"Караоке Клуб"}]},"lengthText":{"simpleText":"4:45"}}},
            {"adSlotRenderer":{}},
            {"shelfRenderer":{"content":{"verticalListRenderer":{"items":[{"videoRenderer":{"videoId":"xyz789","title":{"simpleText":"Песня 2"},"longBylineText":{"runs":[{"text":"Канал"}]},"lengthText":{"simpleText":"3:10"}}}]}}}},
            {"videoRenderer":{"videoId":"live1","title":{"runs":[{"text":"Стрим"}]}}}
        ]}}]}}}}};</script></html>"#;
        let r = parse_results(html).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].secs, Some(285));
        assert_eq!(parse_duration("1:02:03"), Some(3723));
        let shorts_html = r#"<script>var ytInitialData = {"a":[{"reelItemRenderer":{"videoId":"s1","headline":{"simpleText":"Мем короткий"}}},
            {"shortsLockupViewModel":{"onTap":{"innertubeCommand":{"reelWatchEndpoint":{"videoId":"s2"}}},"overlayMetadata":{"primaryText":{"content":"Ещё мем"}}}},
            {"videoRenderer":{"videoId":"v1","title":{"simpleText":"Длинное"},"lengthText":{"simpleText":"0:09"}}}]};</script>"#;
        let r2 = parse_results_ex(shorts_html, true).unwrap();
        assert_eq!(r2.len(), 3);
        assert!(r2.iter().any(|v| v.id == "s2" && v.secs.is_none()));
        assert_eq!(parse_results_ex(shorts_html, false).unwrap().len(), 1);
        assert_eq!(r[0].id, "abc123");
        assert!(r[0].title.contains("Группа крови"));
        assert_eq!(r[0].channel, "Караоке Клуб");
        assert_eq!(r[1].duration, "3:10");
        assert_eq!(with_karaoke("Кино"), "Кино караоке");
        assert!(search_url("a b").ends_with("a+b+%D0%BA%D0%B0%D1%80%D0%B0%D0%BE%D0%BA%D0%B5"));
    }
}
