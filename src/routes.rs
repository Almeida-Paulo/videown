//! As rotas do site.

use crate::auth::Auth;
use crate::http::{Request, Response};
use crate::info::Info;
use crate::jobs::{valid_id, Format, Jobs, NewItem};
use crate::util;
use crate::storage::Delivery;
use std::fs::File;

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const LOGIN: &str = include_str!("../web/login.html");
const ICON: &str = include_str!("../web/icon.svg");
const STYLE: &str = include_str!("../web/style.css");

const SESSION_COOKIE: &str = "videown_session";

fn login_page(error: &str) -> Response {
    let message = if error.is_empty() { String::new() } else { format!("<p class=\"erro\">{error}</p>") };
    Response::html(LOGIN.replace("{{ERRO}}", &message)).header("Cache-Control", "no-store")
}

pub fn handle(req: &Request, auth: &Auth, jobs: &Jobs, info: &Info) -> Response {
    let method = if req.method == "HEAD" { "GET" } else { req.method.as_str() };
    let path = req.path.as_str();

    // Rotas públicas.
    match (method, path) {
        ("GET", "/login") => return login_page(""),
        ("POST", "/login") => {
            let form = req.form();
            return match auth.login(form.get("password").map(String::as_str).unwrap_or("")) {
                Ok(token) => {
                    let secure = if req.is_https() { "; Secure" } else { "" };
                    Response::redirect("/").header(
                        "Set-Cookie",
                        &format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000{secure}"),
                    )
                }
                Err(message) => login_page(message),
            };
        }
        ("GET", "/icon.svg") => {
            return Response::new(200, "image/svg+xml", ICON).header("Cache-Control", "max-age=86400")
        }
        ("GET", "/style.css") => {
            return Response::new(200, "text/css; charset=utf-8", STYLE).header("Cache-Control", "no-cache")
        }
        ("GET", "/healthz") => return Response::new(200, "text/plain", "ok"),
        _ => {}
    }

    let token = req.cookie(SESSION_COOKIE);
    if !auth.check(token) {
        return if path.starts_with("/api/") || req.method != "GET" {
            Response::error(401, "Faça login de novo.")
        } else {
            Response::redirect("/login")
        };
    }

    // Toda ação que muda algo vem do nosso JavaScript, com este cabeçalho. Outro site não consegue enviá-lo.
    if req.method == "POST" && req.header("x-requested-with") != Some("videown") {
        return Response::error(403, "Requisição recusada.");
    }

    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match (method, segments.as_slice()) {
        ("GET", [""]) => Response::html(INDEX).header("Cache-Control", "no-store"),
        ("GET", ["app.js"]) => Response::new(200, "text/javascript; charset=utf-8", APP_JS).header("Cache-Control", "no-store"),
        ("POST", ["logout"]) => {
            auth.logout(token);
            Response::json(200, "{}".into()).header("Set-Cookie", &format!("{SESSION_COOKIE}=; Path=/; Max-Age=0"))
        }
        ("GET", ["api", "jobs"]) => Response::json(200, jobs.list_json()),
        ("POST", ["api", "info"]) => {
            let form = req.form();
            match info.fetch(form.get("url").map(String::as_str).unwrap_or("")) {
                Ok(preview) => Response::json(200, preview.to_json()),
                Err(message) => Response::error(422, &message),
            }
        }
        ("POST", ["api", "jobs"]) => {
            // Campos url, title e thumbnail podem se repetir: um conjunto por item.
            let pairs = util::parse_form_pairs(&String::from_utf8_lossy(&req.body));
            let Some(format) = pairs.iter().find(|(k, _)| k == "format").and_then(|(_, v)| Format::parse(v)) else {
                return Response::error(400, "Formato desconhecido.");
            };
            let field = |name: &str| -> Vec<&str> { pairs.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_str()).collect() };
            let (titles, thumbs) = (field("title"), field("thumbnail"));
            let items: Vec<NewItem> = field("url")
                .into_iter()
                .enumerate()
                .map(|(i, url)| NewItem {
                    url: url.to_string(),
                    title: titles.get(i).unwrap_or(&"").to_string(),
                    thumbnail: thumbs.get(i).unwrap_or(&"").to_string(),
                })
                .collect();
            match jobs.add(&items, format) {
                Ok(n) => Response::json(200, format!("{{\"added\":{n}}}")),
                Err(message) => Response::error(400, message),
            }
        }
        ("POST", ["api", "jobs", id, action]) if valid_id(id) => {
            let ok = match *action {
                "cancel" => jobs.cancel(id),
                "delete" => jobs.delete(id),
                _ => return Response::error(404, "Ação desconhecida."),
            };
            if ok {
                Response::json(200, "{}".into())
            } else {
                Response::error(409, "Não dá para fazer isso com este download agora.")
            }
        }
        ("GET", ["files", id]) if valid_id(id) => match jobs.delivery(id) {
            Some((Delivery::Local(path), name)) => match File::open(&path).and_then(|f| f.metadata().map(|m| (f, m.len()))) {
                Ok((file, len)) => Response::file(file, len, &name),
                Err(_) => Response::error(404, "Arquivo não encontrado."),
            },
            Some((Delivery::Redirect(url), _)) => Response::redirect(&url),
            None => Response::error(404, "Arquivo não encontrado ou já apagado."),
        },
        (_, _) if matches!(method, "GET" | "POST") => Response::error(404, "Página não encontrada."),
        _ => Response::error(405, "Método não permitido."),
    }
}
