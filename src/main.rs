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
    let log_file = std::fs::File::create(&log_path).ok();
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
                Command::Init
                | Command::Install
                | Command::Start
                | Command::Stop
                | Command::Restart
                | Command::Status
                | Command::Store { .. }
                | Command::Overlay => unreachable!(),

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

            let db_path = config::db_path();
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
    let db_path = config::db_path();
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

    let mime = detect_mime(&mime, &content);

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

fn detect_mime(hint: &str, data: &[u8]) -> String {
    if hint != "image" {
        return hint.to_string();
    }
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png".to_string()
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg".to_string()
    } else if data.starts_with(b"GIF8") {
        "image/gif".to_string()
    } else if data.starts_with(b"RIFF")
        && data.len() > 12
        && &data[8..12] == b"WEBP"
    {
        "image/webp".to_string()
    } else if data.starts_with(b"BM") {
        "image/bmp".to_string()
    } else {
        "image/png".to_string()
    }
}
