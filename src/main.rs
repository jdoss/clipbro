mod clipboard;
mod config;
mod daemon;
mod db;
mod dbus;
mod entry;
mod overlay;
mod systemd;

use clap::{Parser, Subcommand};
use tracing_subscriber::{
    EnvFilter, fmt, layer::SubscriberExt,
    util::SubscriberInitExt,
};

#[derive(Parser)]
#[command(
    name = "clipbro",
    about = "Clipboard manager daemon with hotkey overlay"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Toggle the clipboard overlay
    Toggle,
    /// Show the clipboard overlay
    Show,
    /// Hide the clipboard overlay
    Hide,
    /// Initialize config and database
    Init,
    /// Install systemd user service and enable it
    Install,
    /// Start the clipbro systemd service
    Start,
    /// Stop the clipbro systemd service
    Stop,
    /// Restart the clipbro systemd service
    Restart,
    /// Show clipbro systemd service status
    Status,
    /// Clear clipboard history
    Clear,
    /// Toggle pause on clipboard monitoring
    Pause,
    /// Store clipboard content (used by wl-paste --watch)
    Store {
        #[arg(long, default_value = "text/plain")]
        mime: String,
        #[arg(long, default_value = "clipboard")]
        source: String,
    },
    /// Open the overlay UI (spawned by daemon)
    Overlay,
}

fn setup_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,clipbro=info"));

    let stderr_layer = fmt::layer().with_target(true);

    let log_path = config::data_dir().join("clipbro.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();
    let file_layer = log_file.map(|f| {
        fmt::layer()
            .with_target(true)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(f))
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();

    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("PANIC: {info}");
        let bt = std::backtrace::Backtrace::force_capture();
        tracing::error!("Backtrace:\n{bt}");
    }));
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Init) => {
            run_init();
        }
        Some(Command::Install) => {
            systemd::install();
        }
        Some(Command::Start) => {
            systemd::start();
        }
        Some(Command::Stop) => {
            systemd::stop();
        }
        Some(Command::Restart) => {
            systemd::restart();
        }
        Some(Command::Status) => {
            systemd::status();
        }
        Some(Command::Store { mime, source }) => {
            run_store(mime, source);
        }
        Some(Command::Overlay) => {
            setup_logging();
            tracing::info!("Starting overlay");
            overlay::run();
        }
        Some(command) => {
            setup_logging();
            let action = match command {
                Command::Toggle => dbus::PopupAction::Toggle,
                Command::Show => dbus::PopupAction::Show,
                Command::Hide => dbus::PopupAction::Hide,
                Command::Clear => dbus::PopupAction::Clear,
                Command::Pause => dbus::PopupAction::TogglePause,
                Command::Init
                | Command::Install
                | Command::Start
                | Command::Stop
                | Command::Restart
                | Command::Status
                | Command::Store { .. }
                | Command::Overlay => {
                    unreachable!()
                }

            };

            let rt = tokio::runtime::Runtime::new().unwrap();
            if let Err(e) =
                rt.block_on(dbus::send_action(action))
            {
                tracing::error!("Failed to send command: {e}");
                std::process::exit(1);
            }
        }
        None => {
            setup_logging();
            tracing::info!("Starting clipbro daemon");

            let config = config::Config::load();

            let db_path = config::db_path(
                config.db_path.as_deref(),
            );
            let db = match db::Database::open(
                &db_path,
                config.encrypt_db,
            ) {
                Ok(db) => db,
                Err(e) => {
                    if config.encrypt_db {
                        tracing::error!(
                            "Failed to open encrypted database: \
                             {e}. Set encrypt_db = false in {} \
                             or start your secret-service \
                             provider.",
                            config::config_path().display()
                        );
                    } else {
                        tracing::error!(
                            "Failed to open database: {e}"
                        );
                    }
                    std::process::exit(1);
                }
            };

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(daemon::run(db, config));
        }
    }
}

fn run_init() {
    let config_path = config::config_path();
    if config_path.exists() {
        eprintln!(
            "Config already exists: {}",
            config_path.display()
        );
    } else {
        match config::write_default_config() {
            Ok(path) => {
                eprintln!("Wrote config: {}", path.display());
            }
            Err(e) => {
                eprintln!(
                    "Failed to write config: {e}"
                );
                std::process::exit(1);
            }
        }
    }

    let cfg = config::Config::load();
    let db_path =
        config::db_path(cfg.db_path.as_deref());
    if db_path.exists() {
        eprintln!(
            "Database already exists: {}",
            db_path.display()
        );
    } else {
        match db::Database::open(&db_path, cfg.encrypt_db) {
            Ok(_) => {
                eprintln!(
                    "Created database: {}",
                    db_path.display()
                );
            }
            Err(e) => {
                eprintln!("Failed to create database: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn run_store(mime: String, source: String) {
    use std::io::Read;
    let mut content = Vec::new();
    std::io::stdin().read_to_end(&mut content).unwrap_or(0);
    if content.len() < 2 {
        return;
    }
    if mime.starts_with("text/") && content.contains(&0) {
        return;
    }

    let (mime, offset) =
        match detect_mime(&mime, &content) {
            Some(m) => m,
            None => {
                eprintln!(
                    "clipbro store: dropping {} bytes \
                     with image hint but no recognized \
                     image signature",
                    content.len()
                );
                return;
            }
        };
    if offset > 0 {
        content.drain(..offset);
    }

    let conn = match zbus::blocking::Connection::session() {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = conn.call_method(
        Some(dbus::BUS_NAME),
        dbus::OBJECT_PATH,
        Some("io.github.jdoss.clipbro"),
        "Store",
        &(mime, content, source),
    );
}

/// Image-format magic bytes scanned for at the start of
/// clipboard payloads. Chromium prepends an opaque token
/// (e.g. 4 bytes) to image content on Wayland, so we
/// scan a small window rather than only checking offset
/// 0. The data before the signature is stripped before
/// storage so other apps receive a clean image.
const IMAGE_PREFIX_SCAN: usize = 16;

fn detect_mime(
    hint: &str,
    data: &[u8],
) -> Option<(String, usize)> {
    if hint != "image" {
        return Some((hint.to_string(), 0));
    }
    if data.starts_with(b"BM") {
        return Some(("image/bmp".to_string(), 0));
    }
    let max = data.len().min(IMAGE_PREFIX_SCAN);
    for offset in 0..=max {
        let slice = &data[offset..];
        if let Some(mime) = sniff_image_at(slice) {
            return Some((mime, offset));
        }
    }
    None
}

fn sniff_image_at(slice: &[u8]) -> Option<String> {
    if slice.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        Some("image/png".to_string())
    } else if slice.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg".to_string())
    } else if slice.starts_with(b"GIF8") {
        Some("image/gif".to_string())
    } else if slice.starts_with(b"RIFF")
        && slice.len() > 12
        && &slice[8..12] == b"WEBP"
    {
        Some("image/webp".to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_mime_png() {
        let data = [0x89, 0x50, 0x4E, 0x47, 0x0D];
        assert_eq!(
            detect_mime("image", &data),
            Some(("image/png".to_string(), 0)),
        );
    }

    #[test]
    fn detect_mime_jpeg() {
        let data = [0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(
            detect_mime("image", &data),
            Some(("image/jpeg".to_string(), 0)),
        );
    }

    #[test]
    fn detect_mime_gif() {
        assert_eq!(
            detect_mime("image", b"GIF89a..."),
            Some(("image/gif".to_string(), 0)),
        );
    }

    #[test]
    fn detect_mime_webp() {
        let mut data = vec![0u8; 16];
        data[..4].copy_from_slice(b"RIFF");
        data[8..12].copy_from_slice(b"WEBP");
        assert_eq!(
            detect_mime("image", &data),
            Some(("image/webp".to_string(), 0)),
        );
    }

    #[test]
    fn detect_mime_bmp() {
        assert_eq!(
            detect_mime("image", b"BM\x00\x00"),
            Some(("image/bmp".to_string(), 0)),
        );
    }

    #[test]
    fn detect_mime_png_with_chromium_prefix() {
        let mut data: Vec<u8> =
            vec![0x4a, 0x95, 0x37, 0x00];
        data.extend_from_slice(&[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A,
            0x0A,
        ]);
        assert_eq!(
            detect_mime("image", &data),
            Some(("image/png".to_string(), 4)),
        );
    }

    #[test]
    fn detect_mime_unknown_image_returns_none() {
        assert_eq!(
            detect_mime("image", b"\x00\x00\x00\x00"),
            None,
        );
    }

    #[test]
    fn detect_mime_image_placeholder_text_dropped() {
        assert_eq!(detect_mime("image", b"[Image]"), None);
    }

    #[test]
    fn detect_mime_non_image_passthrough() {
        assert_eq!(
            detect_mime("text/plain", b"anything"),
            Some(("text/plain".to_string(), 0)),
        );
    }
}
