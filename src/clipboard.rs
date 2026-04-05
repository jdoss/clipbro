use tokio::io::AsyncWriteExt;

use crate::entry::Entry;

#[cfg_attr(test, mockall::automock)]
pub trait ClipboardService: Send + Sync {
    fn sync_to_selection(
        &self,
        target: &str,
        data: &[u8],
    ) -> impl std::future::Future<Output = ()> + Send;

    fn copy_to_clipboard(
        &self,
        entry: &Entry,
    ) -> impl std::future::Future<Output = ()> + Send;
}

pub struct WaylandClipboard;

impl ClipboardService for WaylandClipboard {
    async fn sync_to_selection(
        &self,
        target: &str,
        data: &[u8],
    ) {
        match target {
            "clipboard" => wl_copy(&[], data).await,
            "primary" => {
                wl_copy(&["--primary"], data).await
            }
            other => {
                tracing::warn!(
                    "Unknown sync target: {other}"
                );
            }
        }
    }

    async fn copy_to_clipboard(&self, entry: &Entry) {
        if let Some(text) = entry.text_content() {
            let data = text.as_bytes().to_vec();
            tokio::join!(
                wl_copy(&[], &data),
                wl_copy(&["--primary"], &data),
            );
        } else if let Some((mime, data)) =
            entry.image_data()
        {
            let mime = mime.to_string();
            let data = data.to_vec();
            let clip_args = ["--type", &mime];
            let primary_args =
                ["--primary", "--type", &mime];
            tokio::join!(
                wl_copy(&clip_args, &data),
                wl_copy(&primary_args, &data),
            );
        }
    }
}

async fn wl_copy(args: &[&str], data: &[u8]) {
    let mut child =
        match tokio::process::Command::new("wl-copy")
            .args(args)
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                tracing::error!(
                    "wl-copy spawn failed: {e}"
                );
                return;
            }
        };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(data).await {
            tracing::error!(
                "wl-copy stdin write failed: {e}"
            );
            let _ = child.kill().await;
        }
    }
}
