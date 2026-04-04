use std::sync::Arc;
use std::time::Duration;

use cosmic::iced::window;
use cosmic::iced::{self, Length, Subscription, Task, Element};
use cosmic::iced::widget::{column, container, scrollable, text, text_input, Column};
use cosmic::iced_futures::futures::{SinkExt, StreamExt};
use cosmic::iced_runtime::core::layout::Limits;
use cosmic::iced_winit::commands::layer_surface::{
    self, KeyboardInteractivity, destroy_layer_surface, get_layer_surface,
};
use cosmic::iced_runtime::platform_specific::wayland::layer_surface::SctkLayerSurfaceSettings;

use tokio::sync::{Mutex, mpsc};

use crate::clipboard;
use crate::config::{self, Config};
use crate::db::Database;
use crate::dbus::{self, PopupAction};
use crate::entry::{Entry, MimeDataMap};

#[derive(Debug, Clone)]
pub enum Message {
    DbusAction(PopupAction),
    SearchChanged(String),
    SelectEntry(i64),
    DeleteEntry(i64),
    ToggleFavorite(i64),
    KeyEvent(iced::keyboard::key::Named),
    EntriesLoaded(Vec<Entry>),
    ClipboardData(MimeDataMap),
    Ignore,
}

struct App {
    db: Arc<Mutex<Database>>,
    config: Config,
    overlay_id: Option<window::Id>,
    entries: Vec<Entry>,
    search_query: String,
    focused_index: usize,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        dbus::init_visible();

        let db_path = config::db_path();
        let db = match Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                tracing::error!("Failed to open database: {e}");
                panic!("Cannot open database at {}: {e}", db_path.display());
            }
        };

        let db = Arc::new(Mutex::new(db));

        let app = Self {
            db,
            config: Config::default(),
            overlay_id: None,
            entries: Vec::new(),
            search_query: String::new(),
            focused_index: 0,
        };

        (app, Task::none())
    }

    fn title(&self, _id: window::Id) -> String {
        "clipbro".into()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DbusAction(action) => match action {
                PopupAction::Toggle => {
                    if self.overlay_id.is_some() {
                        return self.hide_overlay();
                    }
                    return self.show_overlay();
                }
                PopupAction::Show => return self.show_overlay(),
                PopupAction::Hide => return self.hide_overlay(),
                PopupAction::Clear => {
                    let db = self.db.clone();
                    tokio::spawn(async move {
                        let db = db.lock().await;
                        if let Err(e) = db.clear() {
                            tracing::error!("Failed to clear: {e}");
                        }
                    });
                    self.entries.clear();
                }
            },
            Message::SearchChanged(query) => {
                self.search_query = query;
                self.focused_index = 0;
            }
            Message::SelectEntry(id) => {
                if let Some(entry) = self.entries.iter().find(|e| e.id == id) {
                    if let Some(text) = entry.text_content() {
                        tracing::info!("Copied entry {id}");
                        let _ = text; // TODO: copy to clipboard
                    }
                }
                return self.hide_overlay();
            }
            Message::DeleteEntry(id) => {
                let db = self.db.clone();
                tokio::spawn(async move {
                    let db = db.lock().await;
                    if let Err(e) = db.delete(id) {
                        tracing::error!("Failed to delete: {e}");
                    }
                });
                self.entries.retain(|e| e.id != id);
            }
            Message::ToggleFavorite(id) => {
                let db = self.db.clone();
                tokio::spawn(async move {
                    let db = db.lock().await;
                    if let Err(e) = db.toggle_favorite(id) {
                        tracing::error!("Failed to toggle favorite: {e}");
                    }
                });
                if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
                    entry.favorite = !entry.favorite;
                }
            }
            Message::KeyEvent(key) => {
                match key {
                    iced::keyboard::key::Named::Escape => {
                        return self.hide_overlay();
                    }
                    iced::keyboard::key::Named::ArrowDown => {
                        let filtered = self.filtered_entries();
                        if !filtered.is_empty() {
                            self.focused_index =
                                (self.focused_index + 1) % filtered.len();
                        }
                    }
                    iced::keyboard::key::Named::ArrowUp => {
                        let filtered = self.filtered_entries();
                        if !filtered.is_empty() {
                            self.focused_index = (self.focused_index
                                + filtered.len()
                                - 1)
                                % filtered.len();
                        }
                    }
                    iced::keyboard::key::Named::Enter => {
                        let filtered = self.filtered_entries();
                        if let Some(entry) = filtered.get(self.focused_index) {
                            let id = entry.id;
                            return iced::Task::done(Message::SelectEntry(id));
                        }
                    }
                    _ => {}
                }
            }
            Message::EntriesLoaded(entries) => {
                self.entries = entries;
            }
            Message::ClipboardData(data) => {
                tracing::info!(
                    "Clipboard data received: {} mime types",
                    data.len()
                );
                let db = self.db.clone();
                tokio::spawn(async move {
                    let db = db.lock().await;
                    match db.insert(data) {
                        Ok(id) => tracing::info!("Inserted entry {id}"),
                        Err(e) => tracing::error!("Failed to insert: {e}"),
                    }
                });
                return self.load_entries();
            }
            Message::Ignore => {}
        }
        Task::none()
    }

    fn view(&self, _id: window::Id) -> Element<'_, Message> {
        let search = text_input("Search...", &self.search_query)
            .on_input(Message::SearchChanged)
            .padding(10)
            .width(Length::Fill);

        let filtered = self.filtered_entries();

        let entries_list: Element<'_, Message> = if filtered.is_empty() {
            container(text("No clipboard entries"))
                .center_x(Length::Fill)
                .padding(20)
                .into()
        } else {
            let items: Vec<Element<'_, Message>> = filtered
                .iter()
                .enumerate()
                .map(|(i, entry)| {
                    let is_focused = i == self.focused_index;
                    self.entry_row(entry, is_focused)
                })
                .collect();

            scrollable(Column::with_children(items).spacing(4))
                .height(Length::Fill)
                .into()
        };

        container(
            column![search, entries_list].spacing(8).padding(12),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|theme: &iced::Theme| {
            let palette = theme.palette();
            container::Style {
                background: Some(palette.background.into()),
                ..Default::default()
            }
        })
        .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            dbus_subscription(),
            keyboard_subscription(),
            Subscription::run(|| clipboard::clipboard_stream().map(Message::ClipboardData)),
        ])
    }

    fn show_overlay(&mut self) -> Task<Message> {
        if self.overlay_id.is_some() {
            return Task::none();
        }

        let id = window::Id::unique();
        self.overlay_id = Some(id);
        self.search_query.clear();
        self.focused_index = 0;
        dbus::set_visible(true);

        Task::batch([
            get_layer_surface(SctkLayerSurfaceSettings {
                id,
                keyboard_interactivity: KeyboardInteractivity::Exclusive,
                anchor: layer_surface::Anchor::TOP
                    | layer_surface::Anchor::LEFT
                    | layer_surface::Anchor::RIGHT,
                namespace: "clipbro".into(),
                size: Some((None, Some(400))),
                size_limits: Limits::NONE.min_width(1.0).min_height(1.0),
                ..Default::default()
            }),
            self.load_entries(),
        ])
    }

    fn hide_overlay(&mut self) -> Task<Message> {
        if let Some(id) = self.overlay_id.take() {
            dbus::set_visible(false);
            destroy_layer_surface(id)
        } else {
            Task::none()
        }
    }

    fn load_entries(&self) -> Task<Message> {
        let db = self.db.clone();
        let max = self.config.max_entries;
        Task::perform(
            async move {
                let db = db.lock().await;
                db.list_entries(max).unwrap_or_default()
            },
            Message::EntriesLoaded,
        )
    }

    fn filtered_entries(&self) -> Vec<&Entry> {
        if self.search_query.is_empty() {
            return self.entries.iter().collect();
        }

        let query = self.search_query.to_lowercase();
        self.entries
            .iter()
            .filter(|e| {
                e.text_content()
                    .map(|t| t.to_lowercase().contains(&query))
                    .unwrap_or(false)
            })
            .collect()
    }

    fn entry_row<'a>(
        &self,
        entry: &'a Entry,
        focused: bool,
    ) -> Element<'a, Message> {
        use cosmic::iced::widget::button;

        let label = match &entry.entry_type {
            crate::entry::EntryType::Text => {
                let content = entry
                    .text_content()
                    .unwrap_or("[empty]");
                let preview: String = content
                    .chars()
                    .take(100)
                    .collect();
                preview
            }
            crate::entry::EntryType::Url => {
                let url = entry
                    .text_content()
                    .unwrap_or("[url]");
                format!("🔗 {url}")
            }
            crate::entry::EntryType::Image => {
                "[Image]".to_string()
            }
        };

        let fav = if entry.favorite { "⭐ " } else { "" };
        let display = format!("{fav}{label}");

        let btn = button(text(display).width(Length::Fill))
            .on_press(Message::SelectEntry(entry.id))
            .width(Length::Fill)
            .padding(8);

        if focused {
            container(btn)
                .style(|theme: &iced::Theme| {
                    let palette = theme.palette();
                    container::Style {
                        background: Some(
                            iced::Color {
                                a: 0.15,
                                ..palette.primary
                            }
                            .into(),
                        ),
                        ..Default::default()
                    }
                })
                .into()
        } else {
            btn.into()
        }
    }
}

fn dbus_subscription() -> Subscription<Message> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(16, async move |mut output| {
            let (tx, mut rx) = mpsc::unbounded_channel();

            match dbus::serve(tx).await {
                Ok(_conn) => {
                    while let Some(action) = rx.recv().await {
                        if output.send(Message::DbusAction(action)).await.is_err()
                        {
                            break;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("D-Bus failed: {e}");
                }
            }

            std::future::pending::<()>().await;
        })
    })
}

fn keyboard_subscription() -> Subscription<Message> {
    cosmic::iced_futures::event::listen_with(|event, status, _| {
        if matches!(status, iced::event::Status::Captured) {
            return None;
        }

        match event {
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key,
                ..
            }) => {
                if let iced::keyboard::Key::Named(named) = key {
                    match named {
                        iced::keyboard::key::Named::Escape
                        | iced::keyboard::key::Named::ArrowUp
                        | iced::keyboard::key::Named::ArrowDown
                        | iced::keyboard::key::Named::Enter => {
                            Some(Message::KeyEvent(named))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    })
}

pub fn run() {
    let result = iced::daemon(App::new, App::update, App::view)
        .subscription(App::subscription)
        .run();

    if let Err(e) = result {
        tracing::error!("Application error: {e}");
        std::process::exit(1);
    }
}
