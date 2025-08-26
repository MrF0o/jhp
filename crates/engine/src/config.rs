use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub host: String,
    pub port: u16,
    pub document_root: PathBuf,
    pub index_file: String,
    pub extensions_dir: PathBuf,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3000,
            document_root: PathBuf::from("jhp-tests"),
            index_file: "index.jhp".to_string(),
            extensions_dir: PathBuf::from("ext"),
        }
    }
}

impl EngineConfig {
    pub fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn index_path(&self) -> PathBuf {
        self.document_root.join(&self.index_file)
    }

    pub fn set_document_root<P: AsRef<Path>>(mut self, root: P) -> Self {
        self.document_root = root.as_ref().to_path_buf();
        self
    }

    pub fn set_extensions_dir<P: AsRef<Path>>(mut self, dir: P) -> Self {
        self.extensions_dir = dir.as_ref().to_path_buf();
        self
    }

    pub fn http(&self) -> HttpServerConfig {
        self.into()
    }
}

#[derive(Debug, Clone)]
pub struct HttpServerConfig {
    pub host: String,
    pub port: u16,
    pub document_root: PathBuf,
    pub index_file: String,
}

impl HttpServerConfig {
    pub fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn index_path(&self) -> PathBuf {
        self.document_root.join(&self.index_file)
    }
}

impl From<&EngineConfig> for HttpServerConfig {
    fn from(cfg: &EngineConfig) -> Self {
        Self {
            host: cfg.host.clone(),
            port: cfg.port,
            document_root: cfg.document_root.clone(),
            index_file: cfg.index_file.clone(),
        }
    }
}
