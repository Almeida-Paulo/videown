//! Onde os arquivos prontos ficam.
//!
//! Hoje só existe `LocalDisk`, uma pasta no servidor. Para Google Drive, Dropbox ou S3 no futuro,
//! basta outra implementação de `Storage`: o yt-dlp continua gravando em `work_dir`, `finalize`
//! envia o arquivo para fora e `deliver` devolve um link do serviço em vez do arquivo local.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub enum Delivery {
    /// O servidor envia o arquivo.
    Local(PathBuf),
    /// O navegador é redirecionado para outro serviço (link temporário do S3, do Drive...).
    #[allow(dead_code)]
    Redirect(String),
}

pub trait Storage: Send + Sync {
    /// Pasta temporária onde o yt-dlp grava os arquivos do pedido.
    fn work_dir(&self, job_id: &str) -> PathBuf;
    /// Chamado quando o download termina, com o arquivo final.
    fn finalize(&self, job_id: &str, file: &Path) -> io::Result<()>;
    /// Como entregar o arquivo ao usuário.
    fn deliver(&self, job_id: &str, file_name: &str) -> Option<Delivery>;
    /// Apaga tudo o que pertence ao pedido.
    fn delete(&self, job_id: &str);
    /// Pedidos que têm arquivos guardados (para achar sobras).
    fn job_ids(&self) -> Vec<String>;
}

pub struct LocalDisk {
    root: PathBuf,
}

impl LocalDisk {
    pub fn new(root: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(LocalDisk { root })
    }
}

impl Storage for LocalDisk {
    fn work_dir(&self, job_id: &str) -> PathBuf {
        self.root.join(job_id)
    }

    fn finalize(&self, _job_id: &str, _file: &Path) -> io::Result<()> {
        Ok(())
    }

    fn deliver(&self, job_id: &str, file_name: &str) -> Option<Delivery> {
        let path = self.work_dir(job_id).join(file_name);
        path.is_file().then_some(Delivery::Local(path))
    }

    fn delete(&self, job_id: &str) {
        let dir = self.work_dir(job_id);
        if dir.exists() {
            if let Err(e) = fs::remove_dir_all(&dir) {
                eprintln!("videown: não foi possível apagar {}: {e}", dir.display());
            }
        }
    }

    fn job_ids(&self) -> Vec<String> {
        fs::read_dir(&self.root)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default()
    }
}
