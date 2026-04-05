use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, mpsc};

use crate::clipboard;
use crate::config::Config;
use crate::db::Database;
use crate::dbus::{self, PopupAction};

const DEDUP_WINDOW: std::time::Duration =
    std::time::Duration::from_secs(2);

struct Daemon {
    db: Arc<Mutex<Database>>,
    config: Config,
    overlay_child: Option<tokio::process::Child>,
    watcher_children: Vec<tokio::process::Child>,
    last_text_store: Option<(Instant, i64)>,
    last_sync_hash: Option<u64>,
}

impl Daemon {
    fn new(db: Database, config: Config) -> Self {
        Self {
            db: Arc::new(Mutex::new(db)),
            config,
            overlay_child: None,
            watcher_children: Vec::new(),
            last_text_store: None,
            last_sync_hash: None,
        }
    }

    async fn handle_action(&mut self, action: PopupAction) {
        match action {
            PopupAction::Toggle => {
                if self.overlay_running() {
                    self.kill_overlay().await;
                } else {
                    self.spawn_overlay();
                }
            }
            PopupAction::Show => {
                if !self.overlay_running() {
                    self.spawn_overlay();
                }
            }
            PopupAction::Hide => {
                self.kill_overlay().await;
            }
            PopupAction::Clear => {
                let db = self.db.clone();
                let result = {
                    let db = db.lock().await;
                    db.clear()
                };
                if let Err(e) = result {
                    tracing::error!("Failed to clear: {e}");
                }
            }
            PopupAction::Store { mime, content, source } => {
                if content.len() < 2 {
                    return;
                }

                let content_hash = hash_content(&content);
                if let Some(sync_hash) = self.last_sync_hash {
                    if sync_hash == content_hash {
                        self.last_sync_hash = None;
                        tracing::debug!(
                            "Skipping sync echo ({source})"
                        );
                        return;
                    }
                }

                let is_image = mime.starts_with("image/");
                tracing::info!(
                    "Store: {} ({} bytes, {source})",
                    mime,
                    content.len()
                );

                let mut image_superseded_text = false;
                if is_image {
                    if let Some((time, text_id)) =
                        self.last_text_store.take()
                    {
                        if time.elapsed() < DEDUP_WINDOW {
                            let db = self.db.clone();
                            let result = {
                                let db = db.lock().await;
                                db.delete(text_id)
                            };
                            if let Err(e) = result {
                                tracing::error!(
                                    "Failed to delete text \
                                     dupe: {e}"
                                );
                            } else {
                                tracing::info!(
                                    "Removed text entry \
                                     {text_id} (image \
                                     supersedes)"
                                );
                                image_superseded_text = true;
                            }
                        }
                    }
                }

                let mut data = crate::entry::MimeDataMap::new();
                data.insert(mime.clone(), content.clone());
                let db = self.db.clone();
                let result = {
                    let db = db.lock().await;
                    db.insert(data)
                };
                match result {
                    Ok(id) => {
                        tracing::info!("Inserted entry {id}");
                        if !is_image {
                            let is_new = self
                                .last_text_store
                                .as_ref()
                                .map(|(_, prev_id)| *prev_id != id)
                                .unwrap_or(true);
                            if is_new {
                                self.last_text_store =
                                    Some((Instant::now(), id));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to insert: {e}"
                        );
                    }
                }

                let should_sync = self.config.sync_selections
                    && (!is_image || image_superseded_text);
                if should_sync {
                    let target = match source.as_str() {
                        "primary" => "clipboard",
                        _ => "primary",
                    };
                    self.last_sync_hash =
                        Some(content_hash);
                    clipboard::sync_to_selection(
                        target, &content,
                    )
                    .await;
                    tracing::debug!(
                        "Synced to {target}"
                    );
                }
            }
            PopupAction::SelectEntry { id } => {
                tracing::info!("SelectEntry {id}");
                let db = self.db.clone();
                let entry = {
                    let db = db.lock().await;
                    db.get_entry(id)
                };
                match entry {
                    Ok(Some(entry)) => {
                        clipboard::copy_to_clipboard(&entry).await;
                        tracing::info!("Copied entry {id}");
                    }
                    Ok(None) => {
                        tracing::warn!("Entry {id} not found");
                    }
                    Err(e) => {
                        tracing::error!("Failed to load entry: {e}");
                    }
                }
                self.kill_overlay().await;
            }
        }
    }

    fn overlay_running(&mut self) -> bool {
        if let Some(child) = &mut self.overlay_child {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.overlay_child = None;
                    dbus::set_visible(false);
                    false
                }
                Ok(None) => true,
                Err(_) => {
                    self.overlay_child = None;
                    dbus::set_visible(false);
                    false
                }
            }
        } else {
            false
        }
    }

    fn spawn_overlay(&mut self) {
        let exe = std::env::current_exe().unwrap_or_else(|_| {
            std::path::PathBuf::from("clipbro")
        });

        match tokio::process::Command::new(&exe)
            .arg("overlay")
            .spawn()
        {
            Ok(child) => {
                tracing::info!(
                    "Overlay started (PID {})",
                    child.id().unwrap_or(0)
                );
                self.overlay_child = Some(child);
                dbus::set_visible(true);
            }
            Err(e) => {
                tracing::error!("Failed to spawn overlay: {e}");
            }
        }
    }

    async fn kill_overlay(&mut self) {
        if let Some(mut child) = self.overlay_child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        dbus::set_visible(false);
    }

    fn spawn_clipboard_watchers(&mut self) {
        let exe = std::env::current_exe().unwrap_or_else(|_| {
            std::path::PathBuf::from("clipbro")
        });
        let exe_str = exe.to_str().unwrap_or("clipbro");

        let text = tokio::process::Command::new("wl-paste")
            .args([
                "--no-newline",
                "--watch",
                exe_str,
                "store",
                "--mime",
                "text/plain",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        match text {
            Ok(child) => {
                tracing::info!(
                    "Text watcher started (PID {})",
                    child.id().unwrap_or(0)
                );
                self.watcher_children.push(child);
            }
            Err(e) => tracing::error!("Text watcher failed: {e}"),
        }

        let primary = tokio::process::Command::new("wl-paste")
            .args([
                "--no-newline",
                "--primary",
                "--watch",
                exe_str,
                "store",
                "--mime",
                "text/plain",
                "--source",
                "primary",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        match primary {
            Ok(child) => {
                tracing::info!(
                    "Primary watcher started (PID {})",
                    child.id().unwrap_or(0)
                );
                self.watcher_children.push(child);
            }
            Err(e) => {
                tracing::error!("Primary watcher failed: {e}")
            }
        }

        let image = tokio::process::Command::new("wl-paste")
            .args([
                "--no-newline",
                "--type",
                "image",
                "--watch",
                exe_str,
                "store",
                "--mime",
                "image",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        match image {
            Ok(child) => {
                tracing::info!(
                    "Image watcher started (PID {})",
                    child.id().unwrap_or(0)
                );
                self.watcher_children.push(child);
            }
            Err(e) => tracing::error!("Image watcher failed: {e}"),
        }
    }

    async fn shutdown(&mut self) {
        self.kill_overlay().await;
        for mut child in self.watcher_children.drain(..) {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        tracing::info!("Watchers stopped");
    }
}

fn hash_content(data: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

pub async fn run(db: Database, config: Config) {
    dbus::init_visible();

    let (tx, mut rx) = mpsc::unbounded_channel();

    let conn = match dbus::serve(tx).await {
        Ok(conn) => conn,
        Err(e) => {
            tracing::error!("D-Bus registration failed: {e}");
            std::process::exit(1);
        }
    };

    tracing::info!(
        "sync_selections = {}",
        config.sync_selections
    );
    let mut daemon = Daemon::new(db, config);
    daemon.spawn_clipboard_watchers();

    tracing::info!("Daemon running (no Wayland connection)");

    while let Some(action) = rx.recv().await {
        daemon.handle_action(action).await;
    }

    daemon.shutdown().await;
    drop(conn);
}
