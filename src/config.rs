use directories::ProjectDirs;
use std::path::PathBuf;

pub struct Config {
    pub max_entries: usize,
    pub max_age_days: u32,
    pub sync_primary: bool,
    pub incognito: bool,
    pub deny_list: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_entries: 500,
            max_age_days: 30,
            sync_primary: false,
            incognito: false,
            deny_list: vec![
                "1password".into(),
                "org.keepassxc.KeePassXC".into(),
            ],
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
