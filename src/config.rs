use directories::ProjectDirs;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(default)]
pub struct Config {
    pub max_entries: usize,
    pub sync_selections: bool,
    pub encrypt_db: bool,
    pub show_thumbnails: bool,
    pub show_remote_thumbnails: bool,
    pub max_thumbnail_bytes: usize,
    pub position: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_entries: 100,
            sync_selections: true,
            encrypt_db: true,
            show_thumbnails: true,
            show_remote_thumbnails: false,
            max_thumbnail_bytes: 5 * 1024 * 1024,
            position: "top".to_string(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => {
                match toml::from_str(&contents) {
                    Ok(config) => config,
                    Err(e) => {
                        tracing::warn!(
                            "Invalid config at {}: {e}, \
                             using defaults",
                            path.display()
                        );
                        Self::default()
                    }
                }
            }
            Err(_) => Self::default(),
        }
    }
}

fn config_dir() -> PathBuf {
    ProjectDirs::from("io.github", "jdoss", "clipbro")
        .map(|p| p.config_dir().to_path_buf())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME")
                .unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".config/clipbro")
        })
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn data_dir() -> PathBuf {
    ProjectDirs::from("io.github", "jdoss", "clipbro")
        .map(|p| p.data_dir().to_path_buf())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME")
                .unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".local/share/clipbro")
        })
}

pub fn db_path() -> PathBuf {
    data_dir().join("clipbro.db")
}

const DEFAULT_CONFIG_TOML: &str = "\
# Maximum number of clipboard entries to keep
max_entries = 100

# Sync clipboard and primary selection
sync_selections = true

# Encrypt the database using the system keyring
encrypt_db = true

# Show image thumbnails in the overlay
show_thumbnails = true

# Fetch and cache thumbnails for image URLs
show_remote_thumbnails = false

# Maximum size in bytes for remote thumbnail downloads
max_thumbnail_bytes = 5242880

# Overlay position: \"top\", \"bottom\", \"left\", \"right\"
position = \"top\"
";

pub fn write_default_config() -> Result<PathBuf, std::io::Error> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, DEFAULT_CONFIG_TOML)?;
    Ok(path)
}
