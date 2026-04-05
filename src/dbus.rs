use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use tokio::sync::mpsc;

pub const BUS_NAME: &str = "io.github.jdoss.clipbro";
pub const OBJECT_PATH: &str = "/io/github/jdoss/clipbro";

static VISIBLE: OnceLock<AtomicBool> = OnceLock::new();
static PAUSED: OnceLock<AtomicBool> = OnceLock::new();

pub fn set_visible(visible: bool) {
    if let Some(v) = VISIBLE.get() {
        v.store(visible, Ordering::Relaxed);
    }
}

pub fn set_paused(paused: bool) {
    if let Some(v) = PAUSED.get() {
        v.store(paused, Ordering::Relaxed);
    }
}

pub fn query_paused() -> bool {
    let conn =
        match zbus::blocking::Connection::session() {
            Ok(c) => c,
            Err(_) => return false,
        };
    let reply = conn.call_method(
        Some(BUS_NAME),
        OBJECT_PATH,
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &(
            "io.github.jdoss.clipbro",
            "Paused",
        ),
    );
    match reply {
        Ok(msg) => {
            msg.body()
                .deserialize::<zbus::zvariant::Value>()
                .ok()
                .and_then(|v| {
                    match v {
                        zbus::zvariant::Value::Bool(b) => {
                            Some(b)
                        }
                        _ => None,
                    }
                })
                .unwrap_or(false)
        }
        Err(_) => false,
    }
}

pub fn init_visible() {
    let _ = VISIBLE.set(AtomicBool::new(false));
    let _ = PAUSED.set(AtomicBool::new(false));
}

#[derive(Clone, Debug)]
pub enum PopupAction {
    Toggle,
    Show,
    Hide,
    Clear,
    Store { mime: String, content: Vec<u8>, source: String },
    SelectEntry { id: i64 },
    TogglePause,
}

struct ClipbroDBus {
    tx: mpsc::UnboundedSender<PopupAction>,
}

#[zbus::interface(name = "io.github.jdoss.clipbro")]
impl ClipbroDBus {
    async fn toggle(&self) {
        let _ = self.tx.send(PopupAction::Toggle);
    }

    async fn show(&self) {
        let _ = self.tx.send(PopupAction::Show);
    }

    async fn hide(&self) {
        let _ = self.tx.send(PopupAction::Hide);
    }

    async fn clear(&self) {
        let _ = self.tx.send(PopupAction::Clear);
    }

    async fn store(
        &self,
        mime: String,
        content: Vec<u8>,
        source: String,
    ) {
        let _ = self
            .tx
            .send(PopupAction::Store { mime, content, source });
    }

    async fn select_entry(&self, id: i64) {
        let _ = self.tx.send(PopupAction::SelectEntry { id });
    }

    async fn toggle_pause(&self) {
        let _ = self.tx.send(PopupAction::TogglePause);
    }

    #[zbus(property)]
    fn paused(&self) -> bool {
        PAUSED
            .get()
            .map(|v| v.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    #[zbus(property)]
    fn visible(&self) -> bool {
        VISIBLE
            .get()
            .map(|v| v.load(Ordering::Relaxed))
            .unwrap_or(false)
    }
}

pub async fn serve(
    tx: mpsc::UnboundedSender<PopupAction>,
) -> Result<zbus::Connection, zbus::Error> {
    let service = ClipbroDBus { tx };

    let conn = zbus::connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, service)?
        .build()
        .await?;

    tracing::info!("D-Bus service registered at {BUS_NAME}");
    Ok(conn)
}

pub async fn send_action(action: PopupAction) -> Result<(), zbus::Error> {
    let conn = zbus::Connection::session().await?;

    match action {
        PopupAction::Store { mime, content, source } => {
            conn.call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some("io.github.jdoss.clipbro"),
                "Store",
                &(mime, content, source),
            )
            .await?;
        }
        PopupAction::SelectEntry { id } => {
            conn.call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some("io.github.jdoss.clipbro"),
                "SelectEntry",
                &(id,),
            )
            .await?;
        }
        _ => {
            let method = match action {
                PopupAction::Toggle => "Toggle",
                PopupAction::Show => "Show",
                PopupAction::Hide => "Hide",
                PopupAction::Clear => "Clear",
                PopupAction::TogglePause => {
                    "TogglePause"
                }
                PopupAction::Store { .. }
                | PopupAction::SelectEntry { .. } => {
                    unreachable!()
                }
            };
            conn.call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some("io.github.jdoss.clipbro"),
                method,
                &(),
            )
            .await?;
        }
    }

    Ok(())
}
