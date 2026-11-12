//! Servidor HTTP/1.1 mínimo: uma thread por conexão, uma requisição por conexão.
//! Feito para ficar atrás de um proxy com HTTPS (Caddy) e para poucos usuários.

use crate::util;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = 1024 * 1024;
const MAX_CONNECTIONS: usize = 64;

pub struct Request {
    pub method: String,
    pub path: String,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.header("cookie")?
            .split(';')
            .filter_map(|p| p.trim().split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v)
    }

    pub fn form(&self) -> HashMap<String, String> {
        util::parse_form(&String::from_utf8_lossy(&self.body))
    }

    pub fn is_https(&self) -> bool {
        self.header("x-forwarded-proto")
            .is_some_and(|v| v.eq_ignore_ascii_case("https"))
    }
}

pub enum Body {
    Bytes(Vec<u8>),
    File(File, u64),
}

pub struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Body,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Response {
            status,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: Body::Bytes(body.into()),
        }
    }

    pub fn html(body: impl Into<Vec<u8>>) -> Self {
        Self::new(200, "text/html; charset=utf-8", body)
    }

    pub fn json(status: u16, body: String) -> Self {
        Self::new(status, "application/json", body).header("Cache-Control", "no-store")
    }

    pub fn error(status: u16, message: &str) -> Self {
        Self::json(status, format!("{{\"error\":{}}}", util::json_str(message)))
    }

    pub fn redirect(location: &str) -> Self {
        Self::new(303, "text/plain; charset=utf-8", "").header("Location", location)
    }

    pub fn file(file: File, len: u64, name: &str) -> Self {
        let ascii: String = name
            .chars()
            .map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' })
            .collect();
        let disposition = format!(
            "attachment; filename=\"{ascii}\"; filename*=UTF-8''{}",
            util::percent_encode(name)
        );
        Response {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "application/octet-stream".into()),
                ("Content-Disposition".into(), disposition),
            ],
            body: Body::File(file, len),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        303 => "See Other",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        _ => "Error",
    }
}

fn read_request(stream: &TcpStream) -> io::Result<Option<Request>> {
    let limited = stream.try_clone()?.take((MAX_HEADER + MAX_BODY) as u64);
    let mut reader = BufReader::new(limited);
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "requisição inválida");

    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or_else(invalid)?.to_string();
    let target = parts.next().ok_or_else(invalid)?.to_string();

    let mut headers = Vec::new();
    let mut total = line.len();
    loop {
        let mut h = String::new();
        let n = reader.read_line(&mut h)?;
        total += n;
        if n == 0 || total > MAX_HEADER {
            return Err(invalid());
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }

    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.parse().map_err(|_| invalid()))
        .transpose()?
        .unwrap_or(0);
    if len > MAX_BODY {
        return Err(invalid());
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;

    let path = target.split(['?', '#']).next().unwrap_or("/").to_string();
    Ok(Some(Request { method, path, headers, body }))
}

fn write_response(stream: &mut TcpStream, resp: Response, head_only: bool) -> io::Result<()> {
    let len = match &resp.body {
        Body::Bytes(b) => b.len() as u64,
        Body::File(_, len) => *len,
    };
    let mut head = format!("HTTP/1.1 {} {}\r\n", resp.status, reason(resp.status));
    for (k, v) in &resp.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(concat!(
        "X-Content-Type-Options: nosniff\r\n",
        "X-Frame-Options: DENY\r\n",
        "Referrer-Policy: no-referrer\r\n",
        "Content-Security-Policy: default-src 'self'; img-src 'self' https: data:; ",
        "style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; form-action 'self'\r\n",
        "Connection: close\r\n",
    ));
    head.push_str(&format!("Content-Length: {len}\r\n\r\n"));
    stream.write_all(head.as_bytes())?;
    if head_only {
        return Ok(());
    }
    match resp.body {
        Body::Bytes(b) => stream.write_all(&b)?,
        // No Linux, io::copy de arquivo para socket usa sendfile: o arquivo não passa pela memória do programa.
        Body::File(f, len) => {
            io::copy(&mut f.take(len), stream)?;
        }
    }
    stream.flush()
}

struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub type Handler = dyn Fn(&Request) -> Response + Send + Sync;

pub fn serve(listener: TcpListener, handler: Arc<Handler>) {
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let slot = Slot(active.clone());
        let handler = handler.clone();
        let spawned = thread::Builder::new()
            .name("http".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let _slot = slot;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(120)));
                match read_request(&stream) {
                    Ok(Some(req)) => {
                        let resp = handler(&req);
                        let _ = write_response(&mut stream, resp, req.method == "HEAD");
                    }
                    Ok(None) => {}
                    Err(_) => {
                        let _ = write_response(&mut stream, Response::error(400, "Requisição inválida"), false);
                    }
                }
            });
        if let Err(e) = spawned {
            eprintln!("videown: não foi possível abrir thread de conexão: {e}");
        }
    }
}
