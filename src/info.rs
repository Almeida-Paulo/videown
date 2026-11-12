//! Prévia de um link antes de baixar: título, miniatura e, se for playlist, a lista de itens.
//!
//! Usa `yt-dlp --flat-playlist`, que lê a playlist sem abrir cada vídeo, então é rápido.
//! Os campos saem com a conversão `j` (JSON), separados por tabulação, uma linha por item.

use crate::jobs::{friendly_error, validate_url};
use crate::util::{json_str, json_token};
use crate::Config;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

const MAX_ITEMS: usize = 300;
const MAX_CONCURRENT: usize = 3;
const TIMEOUT: Duration = Duration::from_secs(90);

const ENTRY: &str = "RR:E\t%(webpage_url,url)j\t%(title)j\t%(duration)j\t%(thumbnail,thumbnails.-1.url)j\t%(uploader,channel)j";
const PLAYLIST: &str = "playlist:RR:L\t%(title)j\t%(uploader,channel)j\t%(playlist_count)j\t%(thumbnails.-1.url)j";

#[derive(Default, Debug)]
pub struct Entry {
    pub url: String,
    pub title: String,
    pub duration: Option<u64>,
    pub thumbnail: String,
    pub uploader: String,
}

#[derive(Default, Debug)]
pub struct Preview {
    pub playlist: bool,
    pub title: String,
    pub uploader: String,
    pub thumbnail: String,
    pub count: Option<u64>,
    pub entries: Vec<Entry>,
}

fn safe_thumb(url: Option<String>) -> String {
    url.filter(|u| u.starts_with("https://") && u.len() < 2048).unwrap_or_default()
}

fn clip(s: Option<String>, max: usize) -> String {
    s.map(|s| s.chars().filter(|c| !c.is_control()).take(max).collect()).unwrap_or_default()
}

impl Preview {
    /// Monta a prévia a partir das linhas impressas pelo yt-dlp.
    pub fn parse<'a>(lines: impl Iterator<Item = &'a str>) -> Self {
        let mut p = Preview::default();
        for line in lines {
            let fields: Vec<&str> = line.split('\t').collect();
            let get = |i: usize| fields.get(i).and_then(|f| json_token(f));
            match fields.first().copied() {
                Some("RR:E") => {
                    let Some(url) = get(1).filter(|u| validate_url(u).is_ok()) else { continue };
                    p.entries.push(Entry {
                        url,
                        title: clip(get(2), 300),
                        duration: get(3).and_then(|d| d.parse::<f64>().ok()).map(|d| d.round() as u64),
                        thumbnail: safe_thumb(get(4)),
                        uploader: clip(get(5), 120),
                    });
                }
                Some("RR:L") => {
                    p.playlist = true;
                    p.title = clip(get(1), 300);
                    p.uploader = clip(get(2), 120);
                    p.count = get(3).and_then(|c| c.parse().ok());
                    p.thumbnail = safe_thumb(get(4));
                }
                _ => {}
            }
        }
        if !p.playlist {
            if let Some(e) = p.entries.first() {
                p.title = e.title.clone();
                p.uploader = e.uploader.clone();
                p.thumbnail = e.thumbnail.clone();
            }
        } else if p.thumbnail.is_empty() {
            p.thumbnail = p.entries.iter().map(|e| e.thumbnail.clone()).find(|t| !t.is_empty()).unwrap_or_default();
        }
        p
    }

    pub fn to_json(&self) -> String {
        let num = |n: Option<u64>| n.map_or("null".to_string(), |n| n.to_string());
        let entries: Vec<String> = self
            .entries
            .iter()
            .map(|e| {
                format!(
                    "{{\"url\":{},\"title\":{},\"duration\":{},\"thumbnail\":{},\"uploader\":{}}}",
                    json_str(&e.url),
                    json_str(&e.title),
                    num(e.duration),
                    json_str(&e.thumbnail),
                    json_str(&e.uploader)
                )
            })
            .collect();
        let count = self.count.or(Some(self.entries.len() as u64)).map(|c| c.max(self.entries.len() as u64));
        format!(
            "{{\"kind\":{},\"title\":{},\"uploader\":{},\"thumbnail\":{},\"count\":{},\"truncated\":{},\"entries\":[{}]}}",
            json_str(if self.playlist { "playlist" } else { "video" }),
            json_str(&self.title),
            json_str(&self.uploader),
            json_str(&self.thumbnail),
            num(count),
            self.playlist && self.entries.len() >= MAX_ITEMS,
            entries.join(",")
        )
    }
}

struct Slot<'a>(&'a AtomicUsize);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct Info {
    cfg: Arc<Config>,
    active: AtomicUsize,
}

impl Info {
    pub fn new(cfg: Arc<Config>) -> Self {
        Info { cfg, active: AtomicUsize::new(0) }
    }

    pub fn fetch(&self, url: &str) -> Result<Preview, String> {
        let url = url.trim();
        validate_url(url).map_err(String::from)?;
        if self.active.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT {
            self.active.fetch_sub(1, Ordering::SeqCst);
            return Err("Muitas prévias ao mesmo tempo. Tente de novo em instantes.".into());
        }
        let _slot = Slot(&self.active);

        let mut child = Command::new(&self.cfg.ytdlp)
            .args(self.cfg.base_args())
            .args(["--flat-playlist", "--yes-playlist", "--no-warnings", "--playlist-items"])
            .arg(format!("1:{MAX_ITEMS}"))
            .args(["--print", ENTRY, "--print", PLAYLIST, "--", url])
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("Não foi possível iniciar o yt-dlp: {e}"))?;
        let pgid = child.id() as i32;

        let (tx, rx) = mpsc::channel::<(bool, String)>();
        let streams: [(bool, Option<Box<dyn Read + Send>>); 2] = [
            (true, child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>)),
            (false, child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)),
        ];
        for (is_out, stream) in streams {
            let Some(stream) = stream else { continue };
            let tx = tx.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                let mut buf = Vec::new();
                while reader.read_until(b'\n', &mut buf).map(|n| n > 0).unwrap_or(false) {
                    let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                    buf.clear();
                    if tx.send((is_out, line)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        let deadline = Instant::now() + TIMEOUT;
        let mut out = Vec::new();
        let mut last_error = String::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left.max(Duration::from_millis(1))) {
                Ok((true, line)) => out.push(line),
                Ok((false, line)) => {
                    if line.starts_with("ERROR:") {
                        last_error = line;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    unsafe { kill(-pgid, 9) };
                    let _ = child.wait();
                    return Err("O site demorou demais para responder.".into());
                }
            }
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        let preview = Preview::parse(out.iter().map(String::as_str));
        if preview.entries.is_empty() {
            return Err(if !last_error.is_empty() {
                friendly_error(&last_error)
            } else if status.success() {
                "Nenhum vídeo encontrado neste link.".into()
            } else {
                format!("Não foi possível ler este link ({status}).")
            });
        }
        Ok(preview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_playlist() {
        let out = [
            "RR:E\t\"https://www.youtube.com/watch?v=a\"\t\"Primeiro \\\"vídeo\\\"\"\t391.0\t\"https://i.ytimg.com/a.jpg\"\t\"Blender\"",
            "RR:E\t\"https://www.youtube.com/watch?v=b\"\t\"[Private video]\"\tNA\tNA\tnull",
            "RR:E\t\"file:///etc/passwd\"\t\"x\"\tNA\tNA\tNA",
            "RR:L\t\"Official Blender Open Movies\"\t\"Blender\"\t17\t\"https://i.ytimg.com/pl.jpg\"",
        ];
        let p = Preview::parse(out.into_iter());
        assert!(p.playlist);
        assert_eq!(p.title, "Official Blender Open Movies");
        assert_eq!(p.count, Some(17));
        assert_eq!(p.entries.len(), 2);
        assert_eq!(p.entries[0].title, "Primeiro \"vídeo\"");
        assert_eq!(p.entries[0].duration, Some(391));
        assert_eq!(p.entries[1].duration, None);
        assert!(p.to_json().contains("\"kind\":\"playlist\""));
    }

    #[test]
    fn parses_a_single_video() {
        let out = ["RR:E\t\"https://www.youtube.com/watch?v=a\"\t\"Sintel\"\t888\t\"https://i.ytimg.com/s.jpg\"\t\"Blender\""];
        let p = Preview::parse(out.into_iter());
        assert!(!p.playlist);
        assert_eq!(p.title, "Sintel");
        assert_eq!(p.thumbnail, "https://i.ytimg.com/s.jpg");
        assert!(p.to_json().contains("\"count\":1"));
    }
}
