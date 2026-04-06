use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, mpsc};

use crate::clipboard::ClipboardService;
use crate::config::Config;
use crate::db::Database;
use crate::dbus::{self, PopupAction};
use crate::entry;

const DEDUP_WINDOW: std::time::Duration =
    std::time::Duration::from_secs(2);

const SELECTION_ECHO_WINDOW: std::time::Duration =
    std::time::Duration::from_secs(3);

const PRIMARY_DEBOUNCE: std::time::Duration =
    std::time::Duration::from_millis(300);

const WATCHER_CHECK_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatcherKind {
    Text,
    Primary,
    Image,
}

const ALL_WATCHERS: [WatcherKind; 3] = [
    WatcherKind::Text,
    WatcherKind::Primary,
    WatcherKind::Image,
];

struct Daemon<C: ClipboardService> {
    db: Arc<Mutex<Database>>,
    config: Config,
    clipboard: C,
    paused: bool,
    overlay_child: Option<tokio::process::Child>,
    watcher_children:
        Vec<(WatcherKind, tokio::process::Child)>,
    last_text_store: Option<(Instant, i64)>,
    last_sync_hash: Option<u64>,
    last_store_hash: Option<(Instant, u64)>,
    last_selection_hash: Option<(Instant, u64)>,
    primary_debounce:
        Option<tokio::task::JoinHandle<()>>,
    action_tx:
        Option<mpsc::UnboundedSender<PopupAction>>,
}

impl<C: ClipboardService> Daemon<C> {
    fn new(
        db: Database,
        config: Config,
        clipboard: C,
    ) -> Self {
        Self {
            db: Arc::new(Mutex::new(db)),
            config,
            clipboard,
            paused: false,
            overlay_child: None,
            watcher_children: Vec::new(),
            last_text_store: None,
            last_sync_hash: None,
            last_store_hash: None,
            last_selection_hash: None,
            primary_debounce: None,
            action_tx: None,
        }
    }

    async fn handle_action(
        &mut self,
        action: PopupAction,
    ) {
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
                    tracing::error!(
                        "Failed to clear: {e}"
                    );
                }
            }
            PopupAction::TogglePause => {
                self.paused = !self.paused;
                dbus::set_paused(self.paused);
                tracing::info!(
                    "Clipboard monitoring {}",
                    if self.paused {
                        "paused"
                    } else {
                        "resumed"
                    }
                );
            }
            PopupAction::Store {
                mime,
                content,
                source,
            } => {
                self.handle_store(
                    mime, content, source,
                )
                .await;
            }
            PopupAction::SelectEntry { id } => {
                self.handle_select_entry(id).await;
            }
        }
    }

    async fn handle_store(
        &mut self,
        mime: String,
        content: Vec<u8>,
        source: String,
    ) {
        if self.paused {
            tracing::debug!(
                "Skipping store (paused)"
            );
            return;
        }

        if source == "primary" {
            if let Some(h) =
                self.primary_debounce.take()
            {
                h.abort();
            }
            if let Some(tx) = &self.action_tx {
                let tx = tx.clone();
                let m = mime.clone();
                let c = content.clone();
                self.primary_debounce =
                    Some(tokio::spawn(async move {
                        tokio::time::sleep(
                            PRIMARY_DEBOUNCE,
                        )
                        .await;
                        let _ = tx.send(
                            PopupAction::Store {
                                mime: m,
                                content: c,
                                source:
                                    "primary-debounced"
                                        .into(),
                            },
                        );
                    }));
                return;
            }
        }

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

        if let Some((time, sel_hash)) =
            self.last_selection_hash
        {
            if sel_hash == content_hash
                && time.elapsed()
                    < SELECTION_ECHO_WINDOW
            {
                tracing::debug!(
                    "Skipping selection echo \
                     ({source})"
                );
                return;
            }
        }

        if let Some((time, prev_hash)) =
            self.last_store_hash
        {
            if prev_hash == content_hash
                && time.elapsed()
                    < std::time::Duration::from_secs(1)
            {
                tracing::debug!(
                    "Skipping duplicate store"
                );
                return;
            }
        }
        self.last_store_hash =
            Some((Instant::now(), content_hash));

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

        let mut data = entry::MimeDataMap::new();
        data.insert(mime.clone(), content.clone());
        let db = self.db.clone();
        let result = {
            let db = db.lock().await;
            db.insert(data)
        };
        match result {
            Ok(id) => {
                tracing::info!(
                    "Inserted entry {id}"
                );
                if !is_image {
                    let is_new = self
                        .last_text_store
                        .as_ref()
                        .map(|(_, prev_id)| {
                            *prev_id != id
                        })
                        .unwrap_or(true);
                    if is_new {
                        self.last_text_store =
                            Some((Instant::now(), id));
                    }
                }

                if is_image {
                    self.generate_thumbnail(
                        id, &content,
                    );
                } else if self
                    .config
                    .show_remote_thumbnails
                {
                    self.maybe_fetch_thumbnail(
                        id, &content,
                    );
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
                "primary" | "primary-debounced" => {
                    "clipboard"
                }
                _ => "primary",
            };
            self.last_sync_hash =
                Some(content_hash);
            self.clipboard
                .sync_to_selection(target, &content)
                .await;
            tracing::debug!("Synced to {target}");
        }
    }

    async fn handle_select_entry(&mut self, id: i64) {
        tracing::info!("SelectEntry {id}");
        let db = self.db.clone();
        let entry = {
            let db = db.lock().await;
            let _ = db.touch(id);
            db.get_entry(id)
        };
        match entry {
            Ok(Some(entry)) => {
                let hash = entry.content_hash();
                self.last_selection_hash =
                    Some((Instant::now(), hash));
                self.clipboard
                    .copy_to_clipboard(&entry)
                    .await;
                tracing::info!(
                    "Copied entry {id}"
                );
            }
            Ok(None) => {
                tracing::warn!(
                    "Entry {id} not found"
                );
            }
            Err(e) => {
                tracing::error!(
                    "Failed to load entry: {e}"
                );
            }
        }
        self.kill_overlay().await;
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
        let exe =
            std::env::current_exe().unwrap_or_else(
                |_| std::path::PathBuf::from("clipbro"),
            );

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
                tracing::error!(
                    "Failed to spawn overlay: {e}"
                );
            }
        }
    }

    async fn kill_overlay(&mut self) {
        if let Some(mut child) =
            self.overlay_child.take()
        {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        dbus::set_visible(false);
    }

    fn generate_thumbnail(
        &self,
        entry_id: i64,
        image_data: &[u8],
    ) {
        let data = image_data.to_vec();
        let db = self.db.clone();

        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(
                move || resize_to_thumbnail(&data),
            )
            .await;

            let Some(png_bytes) =
                result.ok().flatten()
            else {
                return;
            };

            let db = db.lock().await;
            if let Err(e) = db.add_content(
                entry_id,
                entry::THUMBNAIL_MIME,
                &png_bytes,
            ) {
                tracing::error!(
                    "Failed to store image \
                     thumbnail: {e}"
                );
            } else {
                tracing::debug!(
                    "Generated thumbnail for entry \
                     {entry_id} ({} bytes)",
                    png_bytes.len()
                );
            }
        });
    }

    fn maybe_fetch_thumbnail(
        &self,
        entry_id: i64,
        content: &[u8],
    ) {
        let text = match std::str::from_utf8(content)
        {
            Ok(s) => s.trim(),
            Err(_) => return,
        };
        if !entry::is_image_url(text) {
            return;
        }

        let url = text.to_string();
        let max_bytes = self.config.max_thumbnail_bytes;
        let db = self.db.clone();

        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(
                move || {
                    fetch_thumbnail(&url, max_bytes)
                },
            )
            .await;

            let Some(raw) = result.ok().flatten()
            else {
                return;
            };
            let thumb = resize_to_thumbnail(&raw)
                .unwrap_or(raw);

            let db = db.lock().await;
            if let Err(e) = db.add_content(
                entry_id,
                entry::THUMBNAIL_MIME,
                &thumb,
            ) {
                tracing::error!(
                    "Failed to store thumbnail: {e}"
                );
            } else {
                tracing::info!(
                    "Cached thumbnail for entry \
                     {entry_id} ({} bytes)",
                    thumb.len()
                );
            }
        });
    }

    fn spawn_watcher(
        exe_str: &str,
        kind: WatcherKind,
    ) -> Option<tokio::process::Child> {
        let args: Vec<&str> = match kind {
            WatcherKind::Text => vec![
                "--no-newline",
                "--watch",
                exe_str,
                "store",
                "--mime",
                "text/plain",
            ],
            WatcherKind::Primary => vec![
                "--no-newline",
                "--primary",
                "--watch",
                exe_str,
                "store",
                "--mime",
                "text/plain",
                "--source",
                "primary",
            ],
            WatcherKind::Image => vec![
                "--no-newline",
                "--type",
                "image",
                "--watch",
                exe_str,
                "store",
                "--mime",
                "image",
            ],
        };

        match tokio::process::Command::new("wl-paste")
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                tracing::info!(
                    "{kind:?} watcher started \
                     (PID {})",
                    child.id().unwrap_or(0)
                );
                Some(child)
            }
            Err(e) => {
                tracing::error!(
                    "{kind:?} watcher spawn failed: {e}"
                );
                None
            }
        }
    }

    fn spawn_clipboard_watchers(&mut self) {
        let exe =
            std::env::current_exe().unwrap_or_else(
                |_| std::path::PathBuf::from("clipbro"),
            );
        let exe_str = exe
            .to_str()
            .unwrap_or("clipbro")
            .to_string();

        for &kind in &ALL_WATCHERS {
            if let Some(child) =
                Self::spawn_watcher(&exe_str, kind)
            {
                self.watcher_children
                    .push((kind, child));
            }
        }
    }

    /// Reap any wl-paste watcher that has exited
    /// (typically because the Wayland socket went
    /// away on logout) and respawn it so the daemon
    /// keeps capturing across sessions.
    fn check_watchers(&mut self) {
        let exe =
            std::env::current_exe().unwrap_or_else(
                |_| std::path::PathBuf::from("clipbro"),
            );
        let exe_str = exe
            .to_str()
            .unwrap_or("clipbro")
            .to_string();

        let mut alive = Vec::with_capacity(
            self.watcher_children.len(),
        );
        for (kind, mut child) in
            self.watcher_children.drain(..)
        {
            match child.try_wait() {
                Ok(None) => alive.push((kind, child)),
                Ok(Some(status)) => {
                    tracing::warn!(
                        "{kind:?} watcher exited \
                         ({status}); respawning"
                    );
                    if let Some(new_child) =
                        Self::spawn_watcher(
                            &exe_str, kind,
                        )
                    {
                        alive.push((kind, new_child));
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "{kind:?} watcher try_wait \
                         failed: {e}; respawning"
                    );
                    let _ = child.start_kill();
                    if let Some(new_child) =
                        Self::spawn_watcher(
                            &exe_str, kind,
                        )
                    {
                        alive.push((kind, new_child));
                    }
                }
            }
        }
        self.watcher_children = alive;
    }

    async fn shutdown(&mut self) {
        self.kill_overlay().await;
        for (_, mut child) in
            self.watcher_children.drain(..)
        {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        tracing::info!("Watchers stopped");
    }
}

fn resize_to_thumbnail(
    data: &[u8],
) -> Option<Vec<u8>> {
    let img = image::load_from_memory(data).ok()?;
    let thumb = img.thumbnail(256, 256);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, image::ImageFormat::Png)
        .ok()?;
    Some(buf.into_inner())
}

fn fetch_thumbnail(
    url: &str,
    max_bytes: usize,
) -> Option<Vec<u8>> {
    use std::io::Read;

    let mut response = ureq::get(url)
        .header("Accept", "image/*")
        .call()
        .ok()?;

    let content_type = response
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.starts_with("image/") {
        tracing::debug!(
            "Remote URL is not an image: \
             {content_type}"
        );
        return None;
    }

    let length: usize = response
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if length > max_bytes {
        tracing::debug!(
            "Remote image too large: {length} bytes"
        );
        return None;
    }

    let mut buf = Vec::with_capacity(length);
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(max_bytes as u64)
        .reader();
    match reader.read_to_end(&mut buf) {
        Ok(_) => Some(buf),
        Err(e) => {
            tracing::debug!(
                "Failed to read image body: {e}"
            );
            None
        }
    }
}

fn hash_content(data: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::MockClipboardService;
    use crate::entry::EntryType;

    fn test_db() -> Database {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db =
            Database::open(&path, false).unwrap();
        std::mem::forget(dir);
        db
    }

    fn test_config() -> Config {
        Config {
            sync_selections: true,
            ..Config::default()
        }
    }

    fn test_daemon(
        mock: MockClipboardService,
    ) -> Daemon<MockClipboardService> {
        dbus::init_visible();
        Daemon::new(test_db(), test_config(), mock)
    }

    fn store_action(
        mime: &str,
        content: &[u8],
        source: &str,
    ) -> PopupAction {
        PopupAction::Store {
            mime: mime.into(),
            content: content.to_vec(),
            source: source.into(),
        }
    }

    // -- Unit tests for pure functions --

    #[test]
    fn hash_content_deterministic() {
        let data = b"hello world";
        assert_eq!(
            hash_content(data),
            hash_content(data),
        );
    }

    #[test]
    fn hash_content_different_data() {
        assert_ne!(
            hash_content(b"alpha"),
            hash_content(b"beta"),
        );
    }

    #[test]
    fn resize_to_thumbnail_valid_png() {
        let img = image::RgbaImage::new(512, 512);
        let mut buf =
            std::io::Cursor::new(Vec::new());
        img.write_to(
            &mut buf,
            image::ImageFormat::Png,
        )
        .unwrap();
        let png_bytes = buf.into_inner();

        let thumb =
            resize_to_thumbnail(&png_bytes).unwrap();
        assert!(!thumb.is_empty());
        assert!(thumb.len() < png_bytes.len());

        let decoded =
            image::load_from_memory(&thumb).unwrap();
        assert!(decoded.width() <= 256);
        assert!(decoded.height() <= 256);
    }

    #[test]
    fn resize_to_thumbnail_invalid_data() {
        assert!(
            resize_to_thumbnail(b"not an image")
                .is_none()
        );
    }

    // -- Integration tests with mocked clipboard --

    #[tokio::test]
    async fn store_text_inserts_entry() {
        let mut mock = MockClipboardService::new();
        mock.expect_sync_to_selection()
            .returning(|_, _| Box::pin(async {}));
        let mut daemon = test_daemon(mock);

        daemon
            .handle_action(store_action(
                "text/plain",
                b"hello world",
                "clipboard",
            ))
            .await;

        let db = daemon.db.lock().await;
        let entries =
            db.list_entries(10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].text_content(),
            Some("hello world"),
        );
    }

    #[tokio::test]
    async fn store_skips_tiny_content() {
        let mock = MockClipboardService::new();
        let mut daemon = test_daemon(mock);

        daemon
            .handle_action(store_action(
                "text/plain",
                b"x",
                "clipboard",
            ))
            .await;

        let db = daemon.db.lock().await;
        let entries =
            db.list_entries(10).unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn store_syncs_clipboard_to_primary() {
        let mut mock = MockClipboardService::new();
        mock.expect_sync_to_selection()
            .withf(|target, _| target == "primary")
            .times(1)
            .returning(|_, _| Box::pin(async {}));
        let mut daemon = test_daemon(mock);

        daemon
            .handle_action(store_action(
                "text/plain",
                b"sync me",
                "clipboard",
            ))
            .await;
    }

    #[tokio::test]
    async fn store_syncs_primary_to_clipboard() {
        let mut mock = MockClipboardService::new();
        mock.expect_sync_to_selection()
            .withf(|target, _| {
                target == "clipboard"
            })
            .times(1)
            .returning(|_, _| Box::pin(async {}));
        let mut daemon = test_daemon(mock);

        daemon
            .handle_action(store_action(
                "text/plain",
                b"sync me back",
                "primary-debounced",
            ))
            .await;
    }

    #[tokio::test]
    async fn store_skips_sync_echo() {
        let mut mock = MockClipboardService::new();
        mock.expect_sync_to_selection()
            .times(0);
        let mut daemon = test_daemon(mock);

        let content = b"echo content";
        daemon.last_sync_hash =
            Some(hash_content(content));

        daemon
            .handle_action(store_action(
                "text/plain",
                content,
                "clipboard",
            ))
            .await;

        let db = daemon.db.lock().await;
        assert!(db.list_entries(10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn store_image_supersedes_text() {
        let mut mock = MockClipboardService::new();
        mock.expect_sync_to_selection()
            .returning(|_, _| Box::pin(async {}));
        let mut daemon = test_daemon(mock);

        daemon
            .handle_action(store_action(
                "text/plain",
                b"will be superseded",
                "clipboard",
            ))
            .await;

        let text_id = {
            let db = daemon.db.lock().await;
            let entries =
                db.list_entries(10).unwrap();
            assert_eq!(entries.len(), 1);
            entries[0].id
        };

        daemon
            .handle_action(store_action(
                "image/png",
                b"fake image data bytes",
                "clipboard",
            ))
            .await;

        let db = daemon.db.lock().await;
        assert!(
            db.get_entry(text_id)
                .unwrap()
                .is_none(),
            "text entry should be deleted",
        );
        let entries = db.list_entries(10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].entry_type,
            EntryType::Image,
        );
    }

    #[tokio::test]
    async fn store_no_sync_when_disabled() {
        let mock = MockClipboardService::new();
        dbus::init_visible();
        let config = Config {
            sync_selections: false,
            ..Config::default()
        };
        let mut daemon = Daemon::new(
            test_db(),
            config,
            mock,
        );

        daemon
            .handle_action(store_action(
                "text/plain",
                b"no sync",
                "clipboard",
            ))
            .await;

        let db = daemon.db.lock().await;
        assert_eq!(
            db.list_entries(10).unwrap().len(),
            1,
        );
    }

    #[tokio::test]
    async fn select_entry_copies_to_clipboard() {
        let mut mock = MockClipboardService::new();
        mock.expect_sync_to_selection()
            .returning(|_, _| Box::pin(async {}));
        mock.expect_copy_to_clipboard()
            .times(1)
            .returning(|_| Box::pin(async {}));
        let mut daemon = test_daemon(mock);

        daemon
            .handle_action(store_action(
                "text/plain",
                b"select this",
                "clipboard",
            ))
            .await;

        let id = {
            let db = daemon.db.lock().await;
            db.list_entries(10).unwrap()[0].id
        };

        daemon
            .handle_action(
                PopupAction::SelectEntry { id },
            )
            .await;
    }

    #[tokio::test]
    async fn clear_removes_non_favorites() {
        let mut mock = MockClipboardService::new();
        mock.expect_sync_to_selection()
            .returning(|_, _| Box::pin(async {}));
        let mut daemon = test_daemon(mock);

        daemon
            .handle_action(store_action(
                "text/plain",
                b"keeper entry",
                "clipboard",
            ))
            .await;

        let fav_id = {
            let db = daemon.db.lock().await;
            let entries =
                db.list_entries(10).unwrap();
            let id = entries[0].id;
            db.toggle_favorite(id).unwrap();
            id
        };

        std::thread::sleep(
            std::time::Duration::from_millis(5),
        );

        daemon
            .handle_action(store_action(
                "text/plain",
                b"disposable entry",
                "clipboard",
            ))
            .await;

        daemon
            .handle_action(PopupAction::Clear)
            .await;

        let db = daemon.db.lock().await;
        let entries = db.list_entries(10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, fav_id);
        assert!(entries[0].favorite);
    }

    #[tokio::test]
    async fn store_dedup_skips_rapid_duplicate() {
        let mut mock = MockClipboardService::new();
        mock.expect_sync_to_selection()
            .returning(|_, _| Box::pin(async {}));
        let mut daemon = test_daemon(mock);

        daemon
            .handle_action(store_action(
                "text/plain",
                b"duplicate me",
                "clipboard",
            ))
            .await;

        daemon
            .handle_action(store_action(
                "text/plain",
                b"duplicate me",
                "clipboard",
            ))
            .await;

        let db = daemon.db.lock().await;
        let entries = db.list_entries(10).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "rapid duplicate should be skipped",
        );
    }

    #[tokio::test]
    async fn store_skips_selection_echo() {
        let mock = MockClipboardService::new();
        let mut daemon = test_daemon(mock);

        let content = b"selected content";
        daemon.last_selection_hash = Some((
            Instant::now(),
            hash_content(content),
        ));

        daemon
            .handle_action(store_action(
                "text/plain",
                content,
                "clipboard",
            ))
            .await;

        let db = daemon.db.lock().await;
        assert!(
            db.list_entries(10).unwrap().is_empty(),
            "selection echo should be skipped",
        );
    }

    #[tokio::test]
    async fn store_image_does_not_sync() {
        let mut mock = MockClipboardService::new();
        mock.expect_sync_to_selection().times(0);
        let mut daemon = test_daemon(mock);

        daemon
            .handle_action(store_action(
                "image/png",
                b"fake image bytes here",
                "clipboard",
            ))
            .await;

        let db = daemon.db.lock().await;
        let entries = db.list_entries(10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].entry_type,
            EntryType::Image,
        );
    }

    #[tokio::test]
    async fn select_nonexistent_entry() {
        let mock = MockClipboardService::new();
        let mut daemon = test_daemon(mock);

        daemon
            .handle_action(
                PopupAction::SelectEntry { id: 999999 },
            )
            .await;
    }

    #[tokio::test]
    async fn store_skipped_when_paused() {
        let mock = MockClipboardService::new();
        let mut daemon = test_daemon(mock);

        daemon
            .handle_action(PopupAction::TogglePause)
            .await;
        assert!(daemon.paused);

        daemon
            .handle_action(store_action(
                "text/plain",
                b"should be ignored",
                "clipboard",
            ))
            .await;

        let db = daemon.db.lock().await;
        assert!(
            db.list_entries(10).unwrap().is_empty(),
            "store should be skipped when paused",
        );
    }

    #[tokio::test]
    async fn toggle_pause_resumes() {
        let mut mock = MockClipboardService::new();
        mock.expect_sync_to_selection()
            .returning(|_, _| Box::pin(async {}));
        let mut daemon = test_daemon(mock);

        daemon
            .handle_action(PopupAction::TogglePause)
            .await;
        assert!(daemon.paused);

        daemon
            .handle_action(PopupAction::TogglePause)
            .await;
        assert!(!daemon.paused);

        daemon
            .handle_action(store_action(
                "text/plain",
                b"should be stored",
                "clipboard",
            ))
            .await;

        let db = daemon.db.lock().await;
        assert_eq!(
            db.list_entries(10).unwrap().len(),
            1,
        );
    }
}

pub async fn run(db: Database, config: Config) {
    dbus::init_visible();

    let (tx, mut rx) = mpsc::unbounded_channel();

    let conn = match dbus::serve(tx.clone()).await {
        Ok(conn) => conn,
        Err(e) => {
            tracing::error!(
                "D-Bus registration failed: {e}"
            );
            std::process::exit(1);
        }
    };

    tracing::info!(
        "sync_selections = {}",
        config.sync_selections
    );
    let clipboard = crate::clipboard::WaylandClipboard;
    let mut daemon =
        Daemon::new(db, config, clipboard);
    daemon.action_tx = Some(tx.clone());
    daemon.spawn_clipboard_watchers();

    tracing::info!(
        "Daemon running (no Wayland connection)"
    );

    let mut watcher_check = tokio::time::interval(
        WATCHER_CHECK_INTERVAL,
    );
    watcher_check.set_missed_tick_behavior(
        tokio::time::MissedTickBehavior::Skip,
    );

    loop {
        tokio::select! {
            action = rx.recv() => {
                match action {
                    Some(action) => {
                        daemon
                            .handle_action(action)
                            .await;
                    }
                    None => break,
                }
            }
            _ = watcher_check.tick() => {
                daemon.check_watchers();
            }
        }
    }

    daemon.shutdown().await;
    drop(conn);
}
