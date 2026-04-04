mod app;
mod clipboard;
mod config;
mod db;
mod dbus;
mod entry;

use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "clipbro", about = "Clipboard manager daemon with hotkey overlay")]
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
    /// Clear clipboard history
    Clear,
}

fn setup_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,clipbro=info"));
    let fmt_layer = fmt::layer().with_target(true);
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .init();
}

fn main() {
    setup_logging();
    let cli = Cli::parse();

    match cli.command {
        Some(command) => {
            let action = match command {
                Command::Toggle => dbus::PopupAction::Toggle,
                Command::Show => dbus::PopupAction::Show,
                Command::Hide => dbus::PopupAction::Hide,
                Command::Clear => dbus::PopupAction::Clear,
            };

            let rt = tokio::runtime::Runtime::new().unwrap();
            if let Err(e) = rt.block_on(dbus::send_action(action)) {
                tracing::error!("Failed to send command: {e}");
                std::process::exit(1);
            }
        }
        None => {
            tracing::info!("Starting clipbro daemon");
            app::run();
        }
    }
}
