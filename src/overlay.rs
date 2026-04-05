use cosmic::iced::window;
use cosmic::iced::{self, Element, Length, Subscription, Task};
use cosmic::iced::widget::{
    column, container, scrollable, text, text_input, Column,
};
use cosmic::iced_runtime::core::layout::Limits;
use cosmic::iced_runtime::platform_specific::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced_winit::commands::layer_surface::{
    self, KeyboardInteractivity, get_layer_surface,
};

use crate::config::{self, Config};
use crate::db::Database;
use crate::dbus;
use crate::entry::Entry;

#[derive(Debug, Clone)]
enum Message {
    SearchChanged(String),
    SelectEntry(i64),
    KeyEvent(iced::keyboard::key::Named),
    Unfocused,
    SelectionSent,
}

struct Overlay {
    entries: Vec<Entry>,
    search_query: String,
    focused_index: usize,
}

impl Overlay {
    fn new() -> (Self, Task<Message>) {
        let config = Config::load();
        let db_path = config::db_path();
        let db = match Database::open(
            &db_path,
            config.encrypt_db,
        ) {
            Ok(db) => db,
            Err(e) => {
                tracing::error!("Failed to open database: {e}");
                panic!(
                    "Cannot open database at {}: {e}",
                    db_path.display()
                );
            }
        };
        let entries = db
            .list_entries(config.max_entries)
            .unwrap_or_default();

        let overlay = Self {
            entries,
            search_query: String::new(),
            focused_index: 0,
        };

        let id = window::Id::unique();

        let init_task = get_layer_surface(
            SctkLayerSurfaceSettings {
                id,
                keyboard_interactivity:
                    KeyboardInteractivity::Exclusive,
                anchor: layer_surface::Anchor::TOP
                    | layer_surface::Anchor::LEFT
                    | layer_surface::Anchor::RIGHT,
                namespace: "clipbro".into(),
                size: Some((None, Some(400))),
                size_limits: Limits::NONE
                    .min_width(1.0)
                    .min_height(1.0),
                ..Default::default()
            },
        );

        (overlay, init_task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SearchChanged(query) => {
                self.search_query = query;
                self.focused_index = 0;
            }
            Message::SelectEntry(id) => {
                return Task::perform(
                    async move {
                        let action =
                            dbus::PopupAction::SelectEntry { id };
                        if let Err(e) =
                            dbus::send_action(action).await
                        {
                            tracing::error!(
                                "Failed to send selection: {e}"
                            );
                        }
                    },
                    |_| Message::SelectionSent,
                );
            }
            Message::Unfocused | Message::SelectionSent => {
                return iced::exit();
            }
            Message::KeyEvent(key) => match key {
                iced::keyboard::key::Named::Escape => {
                    return iced::exit();
                }
                iced::keyboard::key::Named::ArrowDown => {
                    let filtered = self.filtered_entries();
                    if !filtered.is_empty() {
                        self.focused_index =
                            (self.focused_index + 1)
                                % filtered.len();
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
                    if let Some(entry) =
                        filtered.get(self.focused_index)
                    {
                        let id = entry.id;
                        return Task::done(
                            Message::SelectEntry(id),
                        );
                    }
                }
                _ => {}
            },
        }
        Task::none()
    }

    fn view(&self, _id: window::Id) -> Element<'_, Message> {
        let search = text_input("Search...", &self.search_query)
            .on_input(Message::SearchChanged)
            .padding(10)
            .width(Length::Fill);

        let filtered = self.filtered_entries();

        let entries_list: Element<'_, Message> =
            if filtered.is_empty() {
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
                        entry_row(entry, is_focused)
                    })
                    .collect();

                scrollable(
                    Column::with_children(items).spacing(4),
                )
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
        input_subscription()
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
}

fn entry_row<'a>(
    entry: &'a Entry,
    focused: bool,
) -> Element<'a, Message> {
    use cosmic::iced::widget::button;

    let label = match &entry.entry_type {
        crate::entry::EntryType::Text => {
            let content =
                entry.text_content().unwrap_or("[empty]");
            content.chars().take(100).collect()
        }
        crate::entry::EntryType::Url => {
            let url =
                entry.text_content().unwrap_or("[url]");
            format!("\u{1f517} {url}")
        }
        crate::entry::EntryType::Image => {
            "[Image]".to_string()
        }
    };

    let fav = if entry.favorite { "\u{2b50} " } else { "" };
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

fn input_subscription() -> Subscription<Message> {
    cosmic::iced_futures::event::listen_with(
        |event, status, _| {
            if matches!(
                status,
                iced::event::Status::Captured
            ) {
                return None;
            }

            match event {
                iced::Event::Keyboard(
                    iced::keyboard::Event::KeyPressed {
                        key, ..
                    },
                ) => {
                    if let iced::keyboard::Key::Named(named) =
                        key
                    {
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
                iced::Event::PlatformSpecific(
                    iced::event::PlatformSpecific::Wayland(
                        iced::event::wayland::Event::Layer(
                            iced::event::wayland::LayerEvent::Unfocused,
                            _,
                            _,
                        ),
                    ),
                ) => Some(Message::Unfocused),
                _ => None,
            }
        },
    )
}

pub fn run() {
    let result = iced::daemon(
        Overlay::new,
        Overlay::update,
        Overlay::view,
    )
    .subscription(Overlay::subscription)
    .run();

    if let Err(e) = result {
        tracing::error!("Overlay error: {e}");
        std::process::exit(1);
    }
}
