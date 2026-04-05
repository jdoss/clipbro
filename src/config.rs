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
    pub hotkeys: Hotkeys,
}

#[derive(Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub toggle_favorite: String,
    pub delete_entry: String,
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
            hotkeys: Hotkeys::default(),
        }
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            toggle_favorite: "ctrl+f".to_string(),
            delete_entry: "delete".to_string(),
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

[hotkeys]
# Toggle favorite on the focused entry
toggle_favorite = \"ctrl+f\"

# Delete the focused entry (favorites are protected)
delete_entry = \"delete\"
";

pub fn write_default_config(
) -> Result<PathBuf, std::io::Error> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, DEFAULT_CONFIG_TOML)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let c = Config::default();
        assert_eq!(c.max_entries, 100);
        assert!(c.sync_selections);
        assert!(c.encrypt_db);
        assert!(c.show_thumbnails);
        assert!(!c.show_remote_thumbnails);
        assert_eq!(c.max_thumbnail_bytes, 5 * 1024 * 1024);
        assert_eq!(c.position, "top");
        assert_eq!(
            c.hotkeys.toggle_favorite,
            "ctrl+f",
        );
        assert_eq!(c.hotkeys.delete_entry, "delete");
    }

    #[test]
    fn toml_partial_fills_defaults() {
        let toml = r#"max_entries = 50"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.max_entries, 50);
        assert!(c.sync_selections);
        assert_eq!(c.position, "top");
    }

    #[test]
    fn toml_full_override() {
        let toml = r#"
max_entries = 200
sync_selections = false
encrypt_db = false
show_thumbnails = false
show_remote_thumbnails = true
max_thumbnail_bytes = 1000
position = "bottom"

[hotkeys]
toggle_favorite = "alt+s"
delete_entry = "ctrl+d"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.max_entries, 200);
        assert!(!c.sync_selections);
        assert!(!c.encrypt_db);
        assert!(!c.show_thumbnails);
        assert!(c.show_remote_thumbnails);
        assert_eq!(c.max_thumbnail_bytes, 1000);
        assert_eq!(c.position, "bottom");
        assert_eq!(c.hotkeys.toggle_favorite, "alt+s");
        assert_eq!(c.hotkeys.delete_entry, "ctrl+d");
    }

    #[test]
    fn write_default_config_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, DEFAULT_CONFIG_TOML).unwrap();
        let contents =
            std::fs::read_to_string(&path).unwrap();
        let c: Config =
            toml::from_str(&contents).unwrap();
        assert_eq!(c.max_entries, 100);
        assert_eq!(c.position, "top");
    }
}
