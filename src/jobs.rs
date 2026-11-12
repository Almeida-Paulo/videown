//! Fila de downloads: guarda os pedidos, roda o yt-dlp, acompanha o progresso e apaga o que venceu.

use crate::storage::{Delivery, Storage};
use crate::util;
use crate::Config;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
const SIGKILL: i32 = 9;
const MAX_WAITING: usize = 50;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed,
    Canceled,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Canceled => "canceled",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [Status::Queued, Status::Running, Status::Done, Status::Failed, Status::Canceled]
            .into_iter()
            .find(|v| v.as_str() == s)
    }

    fn finished(self) -> bool {
        matches!(self, Status::Done | Status::Failed | Status::Canceled)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Best,
    P1080,
    P720,
    P480,
    Mp3,
    M4a,
}

impl Format {
    const ALL: [Format; 6] = [Format::Best, Format::P1080, Format::P720, Format::P480, Format::Mp3, Format::M4a];

    pub fn as_str(self) -> &'static str {
        match self {
            Format::Best => "best",
            Format::P1080 => "1080",
            Format::P720 => "720",
            Format::P480 => "480",
            Format::Mp3 => "mp3",
            Format::M4a => "m4a",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.as_str() == s)
    }

    fn args(self) -> Vec<&'static str> {
        let video = |sort: &'static str| vec!["-S", sort, "--merge-output-format", "mp4"];
        match self {
            Format::Best => video("res,ext:mp4:m4a"),
            Format::P1080 => video("res:1080,ext:mp4:m4a"),
            Format::P720 => video("res:720,ext:mp4:m4a"),
            Format::P480 => video("res:480,ext:mp4:m4a"),
            Format::Mp3 => vec!["-f", "ba/b", "-x", "--audio-format", "mp3", "--audio-quality", "0"],
            Format::M4a => vec!["-f", "ba[ext=m4a]/ba/b", "-x", "--audio-format", "m4a"],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub url: String,
    pub format: Format,
    pub status: Status,
    pub title: String,
    pub thumbnail: String,
    pub stage: String,
    pub downloaded: u64,
    pub total: u64,
    pub speed: f64,
    pub eta: i64,
    pub file_name: String,
    pub size: u64,
    pub created: u64,
    pub finished: u64,
    pub error: String,
}

impl Job {
    fn new(url: &str, format: Format) -> Self {
        Job {
            id: util::random_hex(8),
            url: url.into(),
            format,
            status: Status::Queued,
            title: String::new(),
            thumbnail: String::new(),
            stage: String::new(),
            downloaded: 0,
            total: 0,
            speed: 0.0,
            eta: -1,
            file_name: String::new(),
            size: 0,
            created: util::now(),
            finished: 0,
            error: String::new(),
        }
    }

    fn reset_progress(&mut self) {
        self.stage.clear();
        self.downloaded = 0;
        self.total = 0;
        self.speed = 0.0;
        self.eta = -1;
    }

    fn to_json(&self, retention_secs: u64) -> String {
        let expires = if self.status.finished() { self.finished + retention_secs } else { 0 };
        format!(
            concat!(
                "{{\"id\":{},\"url\":{},\"format\":{},\"status\":{},\"title\":{},\"thumbnail\":{},",
                "\"stage\":{},\"downloaded\":{},\"total\":{},\"speed\":{},\"eta\":{},\"file_name\":{},",
                "\"size\":{},\"created\":{},\"finished\":{},\"expires\":{},\"error\":{}}}"
            ),
            util::json_str(&self.id),
            util::json_str(&self.url),
            util::json_str(self.format.as_str()),
            util::json_str(self.status.as_str()),
            util::json_str(&self.title),
            util::json_str(&self.thumbnail),
            util::json_str(&self.stage),
            self.downloaded,
            self.total,
            self.speed.round() as u64,
            self.eta,
            util::json_str(&self.file_name),
            self.size,
            self.created,
            self.finished,
            expires,
            util::json_str(&self.error),
        )
    }

    /// Formato `chave=valor`, uma linha cada; `\` e quebras de linha são escapados.
    fn to_meta(&self) -> String {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "");
        [
            ("id", self.id.clone()),
            ("url", self.url.clone()),
            ("format", self.format.as_str().into()),
            ("status", self.status.as_str().into()),
            ("title", self.title.clone()),
            ("thumbnail", self.thumbnail.clone()),
            ("file_name", self.file_name.clone()),
            ("size", self.size.to_string()),
            ("created", self.created.to_string()),
            ("finished", self.finished.to_string()),
            ("error", self.error.clone()),
        ]
        .iter()
        .map(|(k, v)| format!("{k}={}\n", esc(v)))
        .collect()
    }

    fn from_meta(text: &str) -> Option<Self> {
        let unesc = |s: &str| {
            let mut out = String::new();
            let mut chars = s.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next() {
                        Some('n') => out.push('\n'),
                        Some(other) => out.push(other),
                        None => {}
                    }
                } else {
                    out.push(c);
                }
            }
            out
        };
        let map: HashMap<&str, String> = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k, unesc(v)))
            .collect();
        let num = |k: &str| map.get(k).and_then(|v| v.parse().ok()).unwrap_or(0);
        let id = map.get("id")?.clone();
        if !valid_id(&id) {
            return None;
        }
        let mut job = Job::new(map.get("url")?, Format::parse(map.get("format")?)?);
        job.id = id;
        job.status = Status::parse(map.get("status")?)?;
        job.title = map.get("title").cloned().unwrap_or_default();
        job.thumbnail = map.get("thumbnail").cloned().unwrap_or_default();
        job.file_name = map.get("file_name").cloned().unwrap_or_default();
        job.error = map.get("error").cloned().unwrap_or_default();
        job.size = num("size");
        job.created = num("created");
        job.finished = num("finished");
        Some(job)
    }
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Aceita só links http(s) para nomes públicos. Bloqueia o próprio servidor, a rede interna e o
/// endereço de metadados da nuvem (169.254.169.254), que um link malicioso poderia tentar ler.
pub fn validate_url(url: &str) -> Result<(), &'static str> {
    const BAD: &str = "Cole um link válido que comece com http:// ou https://.";
    const INTERNAL: &str = "Links para a rede interna ou para o próprio servidor não são permitidos.";
    if url.len() > 2048 || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(BAD);
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or(BAD)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(BAD);
    }
    if authority.starts_with('[') {
        let host = authority.trim_start_matches('[').split(']').next().unwrap_or("");
        let ip: Ipv6Addr = host.parse().map_err(|_| BAD)?;
        let public = !(ip.is_loopback() || ip.is_unspecified() || ip.to_ipv4_mapped().is_some()
            || (ip.segments()[0] & 0xfe00) == 0xfc00
            || (ip.segments()[0] & 0xffc0) == 0xfe80);
        return if public { Ok(()) } else { Err(INTERNAL) };
    }
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h).to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    if host.is_empty() {
        return Err(BAD);
    }
    if host == "localhost" || [".localhost", ".local", ".internal", ".lan", ".home.arpa"].iter().any(|s| host.ends_with(s)) {
        return Err(INTERNAL);
    }
    // Endereços numéricos disfarçados (2130706433, 0x7f.1...) também apontam para IPs.
    let numeric = host.split('.').all(|p| !p.is_empty() && (p.bytes().all(|b| b.is_ascii_digit()) || p.starts_with("0x")));
    if numeric {
        let ip: Ipv4Addr = host.parse().map_err(|_| INTERNAL)?;
        let [a, b, ..] = ip.octets();
        let private = ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified()
            || ip.is_broadcast() || a == 0 || (a == 100 && (64..128).contains(&b)) || a >= 224;
        if private {
            return Err(INTERNAL);
        }
    } else if !host.contains('.') {
        return Err(BAD);
    }
    Ok(())
}

fn friendly_error(raw: &str) -> String {
    let has = |words: &[&str]| words.iter().any(|w| raw.to_lowercase().contains(&w.to_lowercase()));
    if has(&["confirm you're not a bot", "confirm you’re not a bot", "sign in to confirm"]) {
        return "O YouTube bloqueou o servidor com a verificação anti-robô. Configure um arquivo de cookies (veja o README).".into();
    }
    if has(&["unsupported url"]) {
        return "Este site ou link não é suportado.".into();
    }
    if has(&["private video", "login required", "requires authentication", "members-only", "age-restricted", "sign in to confirm your age"]) {
        return "Este vídeo é privado ou exige login. Configure um arquivo de cookies (veja o README).".into();
    }
    if has(&["max-filesize"]) {
        return "O arquivo é maior que o limite configurado no servidor.".into();
    }
    if has(&["http error 429", "too many requests"]) {
        return "O site limitou as requisições deste servidor (erro 429). Tente mais tarde ou use cookies.".into();
    }
    if has(&["video unavailable", "this video is unavailable", "has been removed"]) {
        return "Vídeo indisponível ou removido.".into();
    }
    if has(&["drm"]) {
        return "Este conteúdo é protegido por DRM e não pode ser baixado.".into();
    }
    let cleaned = raw.trim().trim_start_matches("ERROR:").trim();
    let cleaned = match cleaned.strip_prefix('[') {
        Some(rest) => rest.split_once("] ").map_or(cleaned, |(_, msg)| msg),
        None => cleaned,
    };
    // "[youtube] abc123: mensagem" -> "mensagem"
    let cleaned = match cleaned.split_once(": ") {
        Some((id, msg)) if !id.contains(' ') && !msg.is_empty() => msg,
        _ => cleaned,
    };
    let mut out: String = cleaned.chars().take(300).collect();
    if out.is_empty() {
        out = "O download falhou.".into();
    }
    out
}

struct State {
    jobs: Vec<Job>,
    queue: VecDeque<String>,
    running: HashMap<String, i32>,
    canceling: HashSet<String>,
    ytdlp_version: String,
}

pub struct Jobs {
    state: Mutex<State>,
    wake: Condvar,
    cfg: Arc<Config>,
    storage: Arc<dyn Storage>,
    meta_dir: PathBuf,
}

impl Jobs {
    pub fn start(cfg: Arc<Config>, storage: Arc<dyn Storage>) -> std::io::Result<Arc<Self>> {
        let meta_dir = cfg.data_dir.join("jobs");
        fs::create_dir_all(&meta_dir)?;
        fs::create_dir_all(cfg.data_dir.join("cache"))?;

        let mut jobs: Vec<Job> = fs::read_dir(&meta_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "meta"))
            .filter_map(|e| Job::from_meta(&fs::read_to_string(e.path()).ok()?))
            .collect();
        jobs.sort_by_key(|j| std::cmp::Reverse(j.created));

        // O que estava na fila ou baixando quando o servidor parou volta para a fila.
        let mut queue: Vec<&mut Job> = jobs.iter_mut().filter(|j| !j.status.finished()).collect();
        queue.sort_by_key(|j| j.created);
        let queue: VecDeque<String> = queue
            .into_iter()
            .map(|j| {
                j.status = Status::Queued;
                j.id.clone()
            })
            .collect();

        let me = Arc::new(Jobs {
            state: Mutex::new(State {
                jobs,
                queue,
                running: HashMap::new(),
                canceling: HashSet::new(),
                ytdlp_version: String::new(),
            }),
            wake: Condvar::new(),
            cfg: cfg.clone(),
            storage,
            meta_dir,
        });

        for _ in 0..cfg.max_parallel {
            let me = me.clone();
            thread::Builder::new().name("download".into()).spawn(move || me.worker())?;
        }
        let cleaner = me.clone();
        thread::Builder::new().name("cleanup".into()).spawn(move || loop {
            cleaner.cleanup();
            thread::sleep(Duration::from_secs(600));
        })?;
        let updater = me.clone();
        thread::Builder::new().name("update".into()).spawn(move || updater.update_loop())?;
        Ok(me)
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.meta_dir.join(format!("{id}.meta"))
    }

    fn save(&self, job: &Job) {
        let path = self.meta_path(&job.id);
        let tmp = path.with_extension("tmp");
        let result = fs::write(&tmp, job.to_meta()).and_then(|_| fs::rename(&tmp, &path));
        if let Err(e) = result {
            eprintln!("videown: não foi possível salvar o pedido {}: {e}", job.id);
        }
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Job)) -> Option<Job> {
        let mut state = self.state.lock().unwrap();
        let job = state.jobs.iter_mut().find(|j| j.id == id)?;
        f(job);
        Some(job.clone())
    }

    pub fn add(&self, url: &str, format: Format) -> Result<String, &'static str> {
        let url = url.trim();
        validate_url(url)?;
        let job = Job::new(url, format);
        {
            let mut state = self.state.lock().unwrap();
            if state.queue.len() >= MAX_WAITING {
                return Err("A fila está cheia. Espere alguns downloads terminarem.");
            }
            state.jobs.insert(0, job.clone());
            state.queue.push_back(job.id.clone());
        }
        self.save(&job);
        self.wake.notify_one();
        Ok(job.id)
    }

    pub fn cancel(&self, id: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(status) = state.jobs.iter().find(|j| j.id == id).map(|j| j.status) else { return false };
        match status {
            Status::Queued => {
                state.queue.retain(|q| q != id);
                let job = state.jobs.iter_mut().find(|j| j.id == id).unwrap();
                job.status = Status::Canceled;
                job.finished = util::now();
                let job = job.clone();
                drop(state);
                self.save(&job);
                true
            }
            Status::Running => {
                state.canceling.insert(id.to_string());
                if let Some(pgid) = state.running.get(id) {
                    // Mata o yt-dlp e o ffmpeg que ele abriu (mesmo grupo de processos).
                    unsafe { kill(-pgid, SIGKILL) };
                }
                true
            }
            _ => false,
        }
    }

    pub fn delete(&self, id: &str) -> bool {
        {
            let mut state = self.state.lock().unwrap();
            let Some(pos) = state.jobs.iter().position(|j| j.id == id) else { return false };
            if !state.jobs[pos].status.finished() {
                return false;
            }
            state.jobs.remove(pos);
        }
        self.storage.delete(id);
        let _ = fs::remove_file(self.meta_path(id));
        true
    }

    pub fn delivery(&self, id: &str) -> Option<(Delivery, String)> {
        let name = {
            let state = self.state.lock().unwrap();
            let job = state.jobs.iter().find(|j| j.id == id && j.status == Status::Done)?;
            job.file_name.clone()
        };
        Some((self.storage.deliver(id, &name)?, name))
    }

    pub fn list_json(&self) -> String {
        let state = self.state.lock().unwrap();
        let retention = self.cfg.retention_hours * 3600;
        let jobs: Vec<String> = state.jobs.iter().map(|j| j.to_json(retention)).collect();
        format!(
            "{{\"jobs\":[{}],\"retention_hours\":{},\"ytdlp\":{},\"now\":{}}}",
            jobs.join(","),
            self.cfg.retention_hours,
            util::json_str(&state.ytdlp_version),
            util::now()
        )
    }

    fn worker(self: Arc<Self>) {
        loop {
            let job = {
                let mut state = self.state.lock().unwrap();
                loop {
                    if let Some(id) = state.queue.pop_front() {
                        if let Some(job) = state.jobs.iter_mut().find(|j| j.id == id) {
                            job.status = Status::Running;
                            job.reset_progress();
                            job.stage = "Começando…".into();
                            break job.clone();
                        }
                    } else {
                        state = self.wake.wait(state).unwrap();
                    }
                }
            };
            self.save(&job);
            self.run(job);
        }
    }

    fn command(&self, job: &Job, dir: &Path) -> Command {
        let mut cmd = Command::new(&self.cfg.ytdlp);
        cmd.args(["--ignore-config", "--no-playlist", "--newline", "--progress", "--no-colors"])
            .args(["--windows-filenames", "--no-mtime"])
            .arg("--cache-dir")
            .arg(self.cfg.data_dir.join("cache"))
            .arg("-P")
            .arg(dir)
            .args(["-o", "%(title).150B [%(id)s].%(ext)s"])
            .args([
                "--progress-template",
                "download:RR:P %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s %(progress.eta)s",
            ])
            .args(["--print", "before_dl:RR:T %(title)s"])
            .args(["--print", "before_dl:RR:I %(thumbnail)s"])
            .args(["--print", "post_process:RR:S"])
            .args(["--print", "after_move:RR:F %(filepath)s"])
            .args(job.format.args());
        if let Some(cookies) = &self.cfg.cookies_file {
            cmd.arg("--cookies").arg(cookies);
        }
        if let Some(limit) = &self.cfg.max_filesize {
            cmd.args(["--max-filesize", limit]);
        }
        cmd.arg("--").arg(&job.url);
        cmd.env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        cmd
    }

    fn run(&self, job: Job) {
        let id = job.id.clone();
        let dir = self.storage.work_dir(&id);
        let mut file: Option<PathBuf> = None;
        let mut last_error = String::new();

        let outcome = (|| -> Result<std::process::ExitStatus, String> {
            fs::create_dir_all(&dir).map_err(|e| format!("Não foi possível criar a pasta do download: {e}"))?;
            let mut child = self
                .command(&job, &dir)
                .spawn()
                .map_err(|e| format!("Não foi possível iniciar o yt-dlp ({}): {e}", self.cfg.ytdlp.display()))?;
            {
                let mut state = self.state.lock().unwrap();
                state.running.insert(id.clone(), child.id() as i32);
                if state.canceling.contains(&id) {
                    unsafe { kill(-(child.id() as i32), SIGKILL) };
                }
            }
            let (tx, rx) = mpsc::channel::<String>();
            for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)]
                .into_iter()
                .flatten()
            {
                let tx = tx.clone();
                thread::spawn(move || {
                    let mut reader = BufReader::new(stream);
                    let mut buf = Vec::new();
                    while reader.read_until(b'\n', &mut buf).map(|n| n > 0).unwrap_or(false) {
                        let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                        buf.clear();
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(tx);
            for line in rx {
                self.handle_line(&id, &line, &dir, &mut file, &mut last_error);
            }
            child.wait().map_err(|e| e.to_string())
        })();

        let canceled = {
            let mut state = self.state.lock().unwrap();
            state.running.remove(&id);
            state.canceling.remove(&id)
        };

        let now = util::now();
        let result: Result<(String, u64), String> = if canceled {
            Err(String::new())
        } else {
            match outcome {
                Err(e) => Err(e),
                Ok(status) => match &file {
                    Some(path) if status.success() => {
                        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        self.storage
                            .finalize(&id, path)
                            .map(|_| (name, size))
                            .map_err(|e| format!("Não foi possível guardar o arquivo: {e}"))
                    }
                    _ if !last_error.is_empty() => Err(friendly_error(&last_error)),
                    Some(_) => Err(format!("O yt-dlp terminou com erro ({status}).")),
                    None if status.success() => Err("O yt-dlp terminou sem gerar arquivo.".into()),
                    None => Err(format!("O yt-dlp terminou com erro ({status}).")),
                },
            }
        };

        if result.is_err() {
            self.storage.delete(&id);
        }
        let job = self.update(&id, |j| {
            j.reset_progress();
            j.finished = now;
            match result {
                Ok((name, size)) => {
                    j.status = Status::Done;
                    j.file_name = name;
                    j.size = size;
                }
                Err(_) if canceled => j.status = Status::Canceled,
                Err(e) => {
                    j.status = Status::Failed;
                    j.error = e;
                }
            }
        });
        if let Some(job) = job {
            if job.status == Status::Failed {
                eprintln!("videown: pedido {} falhou: {}", job.id, job.error);
            }
            self.save(&job);
        }
    }

    fn handle_line(&self, id: &str, line: &str, dir: &Path, file: &mut Option<PathBuf>, last_error: &mut String) {
        let field = |s: &str| -> Option<String> {
            let s = s.trim();
            (!s.is_empty() && s != "NA").then(|| s.to_string())
        };
        if let Some(rest) = line.strip_prefix("RR:P ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            let num = |i: usize| parts.get(i).and_then(|v| v.parse::<f64>().ok());
            self.update(id, |j| {
                j.downloaded = num(0).unwrap_or(0.0) as u64;
                j.total = num(1).or(num(2)).unwrap_or(0.0) as u64;
                j.speed = num(3).unwrap_or(0.0);
                j.eta = num(4).map_or(-1, |v| v as i64);
                j.stage = "Baixando".into();
            });
        } else if let Some(rest) = line.strip_prefix("RR:T ") {
            if let Some(title) = field(rest) {
                self.update(id, |j| j.title = title.chars().take(300).collect());
            }
        } else if let Some(rest) = line.strip_prefix("RR:I ") {
            if let Some(url) = field(rest).filter(|u| u.starts_with("https://") && u.len() < 2048) {
                self.update(id, |j| j.thumbnail = url);
            }
        } else if line.starts_with("RR:S") {
            self.update(id, |j| {
                j.reset_progress();
                j.stage = "Convertendo…".into();
            });
        } else if let Some(rest) = line.strip_prefix("RR:F ") {
            let path = PathBuf::from(rest.trim());
            let inside = match (path.canonicalize(), dir.canonicalize()) {
                (Ok(p), Ok(d)) => p.starts_with(&d) && p.is_file(),
                _ => false,
            };
            if inside {
                *file = Some(path);
            }
        } else if line.starts_with("ERROR:") || line.contains("max-filesize") {
            *last_error = line.to_string();
        }
    }

    fn cleanup(&self) {
        let now = util::now();
        let retention = self.cfg.retention_hours * 3600;
        let (expired, known): (Vec<String>, HashSet<String>) = {
            let mut state = self.state.lock().unwrap();
            let expired: Vec<String> = state
                .jobs
                .iter()
                .filter(|j| j.status.finished() && now.saturating_sub(j.finished) >= retention)
                .map(|j| j.id.clone())
                .collect();
            state.jobs.retain(|j| !expired.contains(&j.id));
            (expired, state.jobs.iter().map(|j| j.id.clone()).collect())
        };
        for id in &expired {
            self.storage.delete(id);
            let _ = fs::remove_file(self.meta_path(id));
        }
        // Pastas sem pedido (sobras de uma queda do servidor, por exemplo).
        let orphans: Vec<String> = self.storage.job_ids().into_iter().filter(|id| !known.contains(id)).collect();
        for id in &orphans {
            self.storage.delete(id);
        }
        if !expired.is_empty() || !orphans.is_empty() {
            eprintln!("videown: limpeza apagou {} pedido(s) vencido(s) e {} sobra(s)", expired.len(), orphans.len());
        }
    }

    fn ytdlp(&self, args: &[&str]) -> Option<String> {
        let out = Command::new(&self.cfg.ytdlp).args(args).stdin(Stdio::null()).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        Some(text.trim().to_string())
    }

    fn update_loop(&self) {
        let mut first = true;
        loop {
            if self.cfg.auto_update && !first {
                if let Some(out) = self.ytdlp(&["-U"]) {
                    eprintln!("videown: atualização do yt-dlp: {}", out.lines().last().unwrap_or(""));
                }
            }
            first = false;
            match self.ytdlp(&["--version"]) {
                Some(v) => self.state.lock().unwrap().ytdlp_version = v.lines().next().unwrap_or("").into(),
                None => eprintln!("videown: yt-dlp não encontrado em {}", self.cfg.ytdlp.display()),
            }
            thread::sleep(Duration::from_secs(if self.cfg.auto_update { 24 * 3600 } else { 3600 }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_public_links() {
        for url in [
            "https://www.youtube.com/watch?v=abc",
            "https://youtu.be/abc",
            "http://vimeo.com:8080/123",
            "https://8.8.8.8/x",
            "https://[2001:db8::1]/v",
        ] {
            assert!(validate_url(url).is_ok(), "{url}");
        }
    }

    #[test]
    fn rejects_internal_and_odd_links() {
        for url in [
            "ftp://a.com/x",
            "ytsearch:gatos",
            "--exec=rm",
            "https://localhost/x",
            "http://127.0.0.1:8080/",
            "http://169.254.169.254/opc/v2/instance/",
            "http://10.0.0.5/",
            "http://192.168.1.1/",
            "http://172.20.0.1/",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
            "http://2130706433/",
            "http://0x7f.0.0.1/",
            "http://user@youtube.com/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[fd00::1]/",
            "http://servidor/",
            "http://meu.local/",
            "https://a.com/x y",
            "https://",
        ] {
            assert!(validate_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn meta_round_trips() {
        let mut job = Job::new("https://a.com/v", Format::Mp3);
        job.title = "Linha 1\nLinha \\2 = três".into();
        job.status = Status::Done;
        job.size = 1234;
        job.finished = 99;
        let back = Job::from_meta(&job.to_meta()).unwrap();
        assert_eq!(back.title, job.title);
        assert_eq!(back.id, job.id);
        assert_eq!(back.format, Format::Mp3);
        assert_eq!(back.status, Status::Done);
        assert_eq!((back.size, back.finished), (1234, 99));
    }

    #[test]
    fn explains_common_errors() {
        assert!(friendly_error("ERROR: [youtube] abc: Sign in to confirm you're not a bot").contains("anti-robô"));
        assert_eq!(friendly_error("ERROR: [generic] xyz: Algo estranho"), "Algo estranho");
    }
}
