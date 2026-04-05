use directories::ProjectDirs;
use std::path::PathBuf;

pub struct Config {
    pub max_entries: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_entries: 500,
        }
    }
}

pub fn data_dir() -> PathBuf {
    ProjectDirs::from("io.github", "jdoss", "clipbro")
        .map(|p| p.data_dir().to_path_buf())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".local/share/clipbro")
        })
}

pub fn db_path() -> PathBuf {
    data_dir().join("clipbro.db")
}
