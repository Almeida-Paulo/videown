//! Login com uma senha só (um usuário). Sessões ficam na memória.

use crate::util;
use std::collections::HashMap;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

const SESSION_SECS: u64 = 30 * 24 * 3600;
const FAILURE_WINDOW: u64 = 15 * 60;
const MAX_FAILURES: usize = 10;

pub struct Auth {
    password: String,
    sessions: Mutex<HashMap<String, u64>>,
    failures: Mutex<Vec<u64>>,
}

impl Auth {
    pub fn new(password: String) -> Self {
        Auth { password, sessions: Mutex::new(HashMap::new()), failures: Mutex::new(Vec::new()) }
    }

    /// Devolve um token de sessão novo, ou a mensagem para mostrar.
    pub fn login(&self, attempt: &str) -> Result<String, &'static str> {
        let now = util::now();
        {
            let mut failures = self.failures.lock().unwrap();
            failures.retain(|t| now.saturating_sub(*t) < FAILURE_WINDOW);
            if failures.len() >= MAX_FAILURES {
                return Err("Muitas tentativas erradas. Espere 15 minutos.");
            }
        }
        if util::constant_time_eq(attempt.as_bytes(), self.password.as_bytes()) {
            let token = util::random_hex(32);
            let mut sessions = self.sessions.lock().unwrap();
            sessions.retain(|_, expires| *expires > now);
            sessions.insert(token.clone(), now + SESSION_SECS);
            Ok(token)
        } else {
            self.failures.lock().unwrap().push(now);
            eprintln!("videown: tentativa de login com senha errada");
            thread::sleep(Duration::from_secs(1));
            Err("Senha incorreta.")
        }
    }

    pub fn check(&self, token: Option<&str>) -> bool {
        let Some(token) = token else { return false };
        let now = util::now();
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .any(|(t, expires)| *expires > now && util::constant_time_eq(t.as_bytes(), token.as_bytes()))
    }

    pub fn logout(&self, token: Option<&str>) {
        if let Some(token) = token {
            self.sessions.lock().unwrap().remove(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_in_and_out() {
        let auth = Auth::new("senha-bem-comprida".into());
        assert!(!auth.check(None));
        let token = auth.login("senha-bem-comprida").unwrap();
        assert!(auth.check(Some(&token)));
        assert!(!auth.check(Some("outro")));
        auth.logout(Some(&token));
        assert!(!auth.check(Some(&token)));
    }
}
