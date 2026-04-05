use std::collections::HashMap;

use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::window;
use cosmic::iced::{self, ContentFit, Element, Length, Subscription, Task};
use cosmic::iced::widget::{
    button, column, container, image as iced_image, row,
    scrollable, text, text_input, Column, Row,
};
use cosmic::iced::alignment;
use cosmic::iced_runtime::core::layout::Limits;
use cosmic::iced_runtime::platform_specific::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced_winit::commands::layer_surface::{
    self, KeyboardInteractivity, get_layer_surface,
};

use crate::config::{self, Config};
use crate::db::Database;
use crate::dbus;
use crate::entry::Entry;

const CARD_WIDTH: f32 = 340.0;
const CARD_HEIGHT_VERT: f32 = 200.0;
const CARD_PADDING: u16 = 4;
const CARD_SPACING: u16 = 8;

const BAR_THICKNESS: u32 = 400;
const SIDEBAR_WIDTH: u32 = 320;

#[derive(Debug, Clone)]
enum Message {
    SearchChanged(String),
    SelectEntry(i64),
    NavForward,
    NavBackward,
    Dismiss,
    Unfocused,
    SelectionSent,
}

struct Overlay {
    entries: Vec<Entry>,
    search_query: String,
    focused_index: usize,
    active_entry_id: Option<i64>,
    handles: HashMap<i64, iced_image::Handle>,
    horizontal: bool,
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
                tracing::error!(
                    "Failed to open database: {e}"
                );
                panic!(
                    "Cannot open database at {}: {e}",
                    db_path.display()
                );
            }
        };
        let display_limit = config.max_entries.min(20);
        let entries = db
            .list_entries_light(display_limit)
            .unwrap_or_default();

        let active_entry_id =
            detect_active_entry(&entries);

        let handles = build_handles(
            &entries,
            config.show_thumbnails,
            config.show_remote_thumbnails,
        );

        let horizontal = matches!(
            config.position.as_str(),
            "top" | "bottom"
        );

        let overlay = Self {
            entries,
            search_query: String::new(),
            focused_index: 0,
            active_entry_id,
            handles,
            horizontal,
        };

        let id = window::Id::unique();

        let (anchor, size) = position_settings(
            &config.position,
        );

        let init_task = get_layer_surface(
            SctkLayerSurfaceSettings {
                id,
                keyboard_interactivity:
                    KeyboardInteractivity::Exclusive,
                anchor,
                namespace: "clipbro".into(),
                size,
                size_limits: Limits::NONE
                    .min_width(1.0)
                    .min_height(1.0),
                ..Default::default()
            },
        );

        (overlay, init_task)
    }

    fn update(
        &mut self,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::SearchChanged(query) => {
                self.search_query = query;
                self.focused_index = 0;
            }
            Message::SelectEntry(id) => {
                return Task::perform(
                    async move {
                        let action =
                            dbus::PopupAction::SelectEntry {
                                id,
                            };
                        if let Err(e) =
                            dbus::send_action(action).await
                        {
                            tracing::error!(
                                "Failed to send \
                                 selection: {e}"
                            );
                        }
                    },
                    |_| Message::SelectionSent,
                );
            }
            Message::SelectionSent => {
                return iced::exit();
            }
            Message::Dismiss | Message::Unfocused => {
                return self.select_focused_and_exit();
            }
            Message::NavForward => {
                let filtered = self.filtered_entries();
                if !filtered.is_empty() {
                    self.focused_index =
                        (self.focused_index + 1)
                            % filtered.len();
                }
            }
            Message::NavBackward => {
                let filtered = self.filtered_entries();
                if !filtered.is_empty() {
                    self.focused_index =
                        (self.focused_index
                            + filtered.len()
                            - 1)
                            % filtered.len();
                }
            }
        }
        Task::none()
    }

    fn select_focused_and_exit(&self) -> Task<Message> {
        let filtered = self.filtered_entries();
        if let Some(entry) =
            filtered.get(self.focused_index)
        {
            let id = entry.id;
            if self.active_entry_id != Some(id) {
                return Task::done(
                    Message::SelectEntry(id),
                );
            }
        }
        iced::exit()
    }

    fn view(
        &self,
        _id: window::Id,
    ) -> Element<'_, Message> {
        let search =
            text_input("Search...", &self.search_query)
                .on_input(Message::SearchChanged)
                .padding(10);

        let filtered = self.filtered_entries();

        let cards_widget: Element<'_, Message> =
            if filtered.is_empty() {
                container(text("No clipboard entries"))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into()
            } else {
                let cards: Vec<Element<'_, Message>> =
                    filtered
                        .iter()
                        .enumerate()
                        .map(|(i, entry)| {
                            entry_card(
                                entry,
                                i == self.focused_index,
                                self.active_entry_id
                                    == Some(entry.id),
                                self.handles
                                    .get(&entry.id),
                                self.horizontal,
                            )
                        })
                        .collect();

                if self.horizontal {
                    scrollable(
                        Row::with_children(cards)
                            .spacing(CARD_SPACING),
                    )
                    .direction(
                        scrollable::Direction::Horizontal(
                            scrollable::Scrollbar::new(),
                        ),
                    )
                    .height(Length::Fill)
                    .into()
                } else {
                    scrollable(
                        Column::with_children(cards)
                            .spacing(CARD_SPACING),
                    )
                    .height(Length::Fill)
                    .into()
                }
            };

        let search_widget: Element<'_, Message> =
            if self.horizontal {
                container(search)
                    .width(Length::Fixed(250.0))
                    .into()
            } else {
                container(search)
                    .width(Length::Fill)
                    .into()
            };

        let layout: Element<'_, Message> =
            if self.horizontal {
                row![search_widget, cards_widget]
                    .spacing(8)
                    .padding(12)
                    .height(Length::Fill)
                    .into()
            } else {
                column![search_widget, cards_widget]
                    .spacing(8)
                    .padding(12)
                    .width(Length::Fill)
                    .into()
            };

        container(layout)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|theme: &iced::Theme| {
                let palette = theme.palette();
                container::Style {
                    background: Some(
                        palette.background.into(),
                    ),
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
                    .map(|t| {
                        t.to_lowercase().contains(&query)
                    })
                    .unwrap_or(false)
            })
            .collect()
    }
}

fn position_settings(
    position: &str,
) -> (layer_surface::Anchor, Option<(Option<u32>, Option<u32>)>)
{
    match position {
        "bottom" => (
            layer_surface::Anchor::BOTTOM
                | layer_surface::Anchor::LEFT
                | layer_surface::Anchor::RIGHT,
            Some((None, Some(BAR_THICKNESS))),
        ),
        "left" => (
            layer_surface::Anchor::TOP
                | layer_surface::Anchor::LEFT
                | layer_surface::Anchor::BOTTOM,
            Some((Some(SIDEBAR_WIDTH), None)),
        ),
        "right" => (
            layer_surface::Anchor::TOP
                | layer_surface::Anchor::RIGHT
                | layer_surface::Anchor::BOTTOM,
            Some((Some(SIDEBAR_WIDTH), None)),
        ),
        _ => (
            layer_surface::Anchor::TOP
                | layer_surface::Anchor::LEFT
                | layer_surface::Anchor::RIGHT,
            Some((None, Some(BAR_THICKNESS))),
        ),
    }
}

fn build_handles(
    entries: &[Entry],
    show_thumbnails: bool,
    show_remote_thumbnails: bool,
) -> HashMap<i64, iced_image::Handle> {
    let mut map = HashMap::new();
    for entry in entries {
        let dominated_by_config = match &entry.entry_type {
            crate::entry::EntryType::Image => {
                show_thumbnails
            }
            crate::entry::EntryType::Url => {
                show_remote_thumbnails
            }
            _ => false,
        };
        if !dominated_by_config {
            continue;
        }
        if let Some(data) = entry.thumbnail_data() {
            let handle =
                iced_image::Handle::from_bytes(
                    data.to_vec(),
                );
            map.insert(entry.id, handle);
        }
    }
    map
}

fn entry_card<'a>(
    entry: &'a Entry,
    focused: bool,
    active: bool,
    handle: Option<&iced_image::Handle>,
    horizontal: bool,
) -> Element<'a, Message> {
    use crate::entry::{EntryType, is_image_url};

    let badge = match (active, entry.favorite) {
        (true, true) => "\u{1f4cb} \u{2b50}",
        (true, false) => "\u{1f4cb}",
        (false, true) => "\u{2b50}",
        (false, false) => "",
    };

    let has_badge = !badge.is_empty();

    let body: Element<'a, Message> = match &entry.entry_type
    {
        EntryType::Image => {
            if let Some(h) = handle {
                column![
                    iced_image::Image::new(h.clone())
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fill)
                        .height(Length::FillPortion(4)),
                    text("Image")
                        .size(12)
                        .width(Length::Fill)
                        .wrapping(Wrapping::None),
                ]
                .spacing(4)
                .align_x(alignment::Horizontal::Center)
                .into()
            } else {
                container(text("[Image]"))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into()
            }
        }
        EntryType::Url => {
            let url =
                entry.text_content().unwrap_or("[url]");
            let emoji = if is_image_url(url) {
                "\u{1f5bc}\u{fe0f} "
            } else {
                "\u{1f517} "
            };
            if let Some(h) = handle {
                column![
                    iced_image::Image::new(h.clone())
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fill)
                        .height(Length::FillPortion(3)),
                    text(format!("{emoji}{url}"))
                        .size(12)
                        .wrapping(Wrapping::WordOrGlyph)
                        .width(Length::Fill),
                ]
                .spacing(4)
                .into()
            } else {
                container(
                    text(format!("{emoji}{url}"))
                        .size(13)
                        .wrapping(Wrapping::WordOrGlyph)
                        .width(Length::Fill),
                )
                .padding(4)
                .into()
            }
        }
        EntryType::Text => {
            let content =
                entry.text_content().unwrap_or("[empty]");
            let truncated: String =
                content.chars().take(200).collect();
            container(
                text(truncated)
                    .size(13)
                    .wrapping(Wrapping::Word)
                    .width(Length::Fill),
            )
            .padding(4)
            .into()
        }
    };

    let mut card_content = Column::new()
        .spacing(2)
        .width(Length::Fill)
        .height(Length::Fill);
    if has_badge {
        card_content =
            card_content.push(text(badge).size(12));
    }
    card_content = card_content.push(body);

    let (card_w, card_h) = if horizontal {
        (
            Length::Fixed(CARD_WIDTH),
            Length::Fill,
        )
    } else {
        (
            Length::Fill,
            Length::Fixed(CARD_HEIGHT_VERT),
        )
    };

    let btn_style =
        move |theme: &iced::Theme,
              _status: button::Status|
              -> button::Style {
            let palette = theme.palette();
            let (bg, border) = if focused {
                (
                    iced::Color {
                        a: 0.20,
                        ..palette.primary
                    }
                    .into(),
                    iced::Border {
                        color: palette.primary,
                        width: 3.0,
                        radius: 8.0.into(),
                    },
                )
            } else if active {
                (
                    iced::Color {
                        a: 0.10,
                        ..palette.success
                    }
                    .into(),
                    iced::Border {
                        color: palette.success,
                        width: 2.0,
                        radius: 8.0.into(),
                    },
                )
            } else {
                (
                    iced::Color {
                        a: 0.05,
                        ..palette.text
                    }
                    .into(),
                    iced::Border {
                        color: iced::Color {
                            a: 0.15,
                            ..palette.text
                        },
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                )
            };
            button::Style {
                background: Some(bg),
                border,
                text_color: palette.text,
                ..Default::default()
            }
        };

    button(card_content)
        .on_press(Message::SelectEntry(entry.id))
        .width(card_w)
        .height(card_h)
        .padding(CARD_PADDING)
        .style(btn_style)
        .into()
}

fn detect_active_entry(entries: &[Entry]) -> Option<i64> {
    let output = std::process::Command::new("wl-paste")
        .arg("--no-newline")
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty()
    {
        return None;
    }

    let clip = &output.stdout;
    for entry in entries {
        if let Some(t) = entry.text_content() {
            if t.as_bytes() == clip.as_slice() {
                return Some(entry.id);
            }
        }
    }

    None
}

fn input_subscription() -> Subscription<Message> {
    cosmic::iced_futures::event::listen_with(
        |event, status, _| match &event {
            iced::Event::Keyboard(
                iced::keyboard::Event::KeyPressed {
                    key, ..
                },
            ) => {
                if let iced::keyboard::Key::Named(named) =
                    key
                {
                    use iced::keyboard::key::Named;
                    match named {
                        Named::Escape | Named::Enter => {
                            Some(Message::Dismiss)
                        }
                        Named::ArrowRight
                        | Named::ArrowDown => {
                            Some(Message::NavForward)
                        }
                        Named::ArrowLeft
                        | Named::ArrowUp => {
                            Some(Message::NavBackward)
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
            _ => {
                if matches!(
                    status,
                    iced::event::Status::Captured
                ) {
                    return None;
                }
                None
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
