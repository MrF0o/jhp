use std::path::{Path, PathBuf};
use tokio::fs;

#[derive(Clone, Debug)]
pub struct DocumentRoot {
    root: PathBuf,
    index_file: String,
}

impl DocumentRoot {
    /// Create a new DocumentRoot for the web server.
    /// `root` is the directory that serves as the document root, and
    /// `index_file` is the default index document (e.g., "index.jhp").
    pub fn new(root: PathBuf, index_file: String) -> Self {
        Self { root, index_file }
    }

    pub async fn root_file_exists(&self, name: &str) -> bool {
        fs::metadata(self.root.join(name)).await.is_ok()
    }

    /// Returns the full path to the index document under the document root.
    pub fn index_path(&self) -> PathBuf {
        self.root.join(&self.index_file)
    }

    /// Returns the name of the index file (e.g., "index.jhp").
    pub fn index_name(&self) -> &str {
        &self.index_file
    }

    /// Read the index document contents.
    pub async fn read_index(&self) -> std::io::Result<String> {
        fs::read_to_string(self.index_path()).await
    }

    /// Read an arbitrary file under the document root.
    pub async fn read_file<P: AsRef<Path>>(&self, rel: P) -> std::io::Result<String> {
        fs::read_to_string(self.root.join(rel)).await
    }
}
