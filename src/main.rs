//! Videown: baixe vídeos e áudios pelo navegador, num servidor seu, com yt-dlp.

mod auth;
mod http;
mod info;
mod jobs;
mod routes;
mod storage;
mod util;

use std::env;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

pub struct Config {
    pub listen: String,
    pub password: String,
    pub data_dir: PathBuf,
    pub ytdlp: PathBuf,
    pub max_parallel: usize,
    pub retention_hours: u64,
    pub cookies_file: Option<PathBuf>,
    pub max_filesize: Option<String>,
    pub auto_update: bool,
    pub proxy: Option<String>,
    pub force_ipv4: bool,
    pub extractor_args: Option<String>,
}

impl Config {
    /// Argumentos comuns a toda chamada do yt-dlp (download e prévia).
    pub fn base_args(&self) -> Vec<std::ffi::OsString> {
        let mut a: Vec<std::ffi::OsString> = vec!["--ignore-config".into(), "--no-colors".into(), "--cache-dir".into()];
        a.push(self.data_dir.join("cache").into());
        if let Some(cookies) = &self.cookies_file {
            a.push("--cookies".into());
            a.push(cookies.into());
        }
        if let Some(proxy) = &self.proxy {
            a.push("--proxy".into());
            a.push(proxy.into());
        }
        if self.force_ipv4 {
            a.push("--force-ipv4".into());
        }
        if let Some(args) = &self.extractor_args {
            a.push("--extractor-args".into());
            a.push(args.into());
        }
        a
    }
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let var = |name: &str| env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let number = |name: &str, default: u64| -> Result<u64, String> {
            var(name).map_or(Ok(default), |v| v.parse().map_err(|_| format!("{name} precisa ser um número")))
        };
        let password = var("VIDEOWN_PASSWORD").ok_or("defina a senha em VIDEOWN_PASSWORD")?;
        if password.chars().count() < 12 {
            return Err("VIDEOWN_PASSWORD precisa ter pelo menos 12 caracteres".into());
        }
        let max_filesize = var("VIDEOWN_MAX_FILESIZE");
        if let Some(limit) = &max_filesize {
            let digits = limit.trim_end_matches(|c: char| "kKmMgGtT".contains(c));
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err("VIDEOWN_MAX_FILESIZE precisa ser algo como 500M ou 4G".into());
            }
        }
        Ok(Config {
            listen: var("VIDEOWN_LISTEN").unwrap_or_else(|| "127.0.0.1:8080".into()),
            password,
            data_dir: var("VIDEOWN_DATA_DIR").map_or_else(|| PathBuf::from("data"), PathBuf::from),
            ytdlp: var("VIDEOWN_YTDLP").map_or_else(|| PathBuf::from("yt-dlp"), PathBuf::from),
            max_parallel: number("VIDEOWN_MAX_PARALLEL", 2)?.clamp(1, 16) as usize,
            retention_hours: number("VIDEOWN_RETENTION_HOURS", 72)?.max(1),
            cookies_file: var("VIDEOWN_COOKIES_FILE").map(PathBuf::from),
            max_filesize,
            auto_update: var("VIDEOWN_AUTO_UPDATE").is_none_or(|v| !is_off(&v)),
            proxy: var("VIDEOWN_PROXY"),
            force_ipv4: var("VIDEOWN_FORCE_IPV4").is_some_and(|v| !is_off(&v)),
            extractor_args: var("VIDEOWN_EXTRACTOR_ARGS"),
        })
    }
}

fn is_off(v: &str) -> bool {
    matches!(v.to_lowercase().as_str(), "0" | "false" | "nao" | "não" | "off" | "no")
}

fn main() {
    let cfg = match Config::from_env() {
        Ok(cfg) => Arc::new(cfg),
        Err(e) => {
            eprintln!("videown: {e}");
            std::process::exit(2);
        }
    };
    let fail = |what: &str, e: std::io::Error| -> ! {
        eprintln!("videown: {what}: {e}");
        std::process::exit(1);
    };

    let storage = storage::LocalDisk::new(cfg.data_dir.join("files")).unwrap_or_else(|e| fail("pasta de dados", e));
    let jobs = jobs::Jobs::start(cfg.clone(), Arc::new(storage)).unwrap_or_else(|e| fail("fila de downloads", e));
    let auth = Arc::new(auth::Auth::new(cfg.password.clone()));
    let listener = TcpListener::bind(&cfg.listen).unwrap_or_else(|e| fail(&format!("não foi possível usar {}", cfg.listen), e));

    eprintln!(
        "videown {}: ouvindo em http://{} · arquivos guardados por {} h · {} download(s) por vez",
        env!("CARGO_PKG_VERSION"),
        cfg.listen,
        cfg.retention_hours,
        cfg.max_parallel
    );
    let info = Arc::new(info::Info::new(cfg.clone()));
    let handler = Arc::new(move |req: &http::Request| routes::handle(req, &auth, &jobs, &info));
    http::serve(listener, handler);
}
