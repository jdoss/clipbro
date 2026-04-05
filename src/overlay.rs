use std::collections::HashMap;
use std::sync::OnceLock;

use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::window;
use cosmic::iced::{
    self, Color, ContentFit, Element, Length, Subscription,
    Task,
};
use cosmic::iced::widget::{
    button, column, container, image as iced_image,
    rich_text, scrollable, text, text_input, Column, Row,
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
const CARD_PADDING: u16 = 8;
const CARD_SPACING: u16 = 8;

const BAR_THICKNESS: u32 = 400;
const SIDEBAR_WIDTH: u32 = 320;

const SCROLLABLE_ID: &str = "clipbro-cards";
const SEARCH_ID: &str = "clipbro-search";

static FAVORITE_HOTKEY: OnceLock<ParsedHotkey> =
    OnceLock::new();
static DELETE_HOTKEY: OnceLock<ParsedHotkey> =
    OnceLock::new();

#[derive(Debug, Clone)]
enum Message {
    SearchChanged(String),
    SelectEntry(i64),
    ToggleFavorite(i64),
    ToggleFocusedFavorite,
    DeleteEntry,
    NavForward,
    NavBackward,
    CharTyped(String),
    Backspace,
    Dismiss,
    Unfocused,
    SelectionSent,
}

#[derive(Clone)]
struct ParsedHotkey {
    ctrl: bool,
    alt: bool,
    shift: bool,
    key_char: String,
}

impl ParsedHotkey {
    fn parse(s: &str) -> Self {
        let lower = s.to_lowercase();
        let parts: Vec<&str> =
            lower.split('+').map(|p| p.trim()).collect();
        let mut hotkey = Self {
            ctrl: false,
            alt: false,
            shift: false,
            key_char: String::new(),
        };
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                hotkey.key_char = part.to_string();
            } else {
                match *part {
                    "ctrl" => hotkey.ctrl = true,
                    "alt" => hotkey.alt = true,
                    "shift" => hotkey.shift = true,
                    _ => {}
                }
            }
        }
        hotkey
    }

    fn matches(
        &self,
        key: &iced::keyboard::Key,
        mods: iced::keyboard::Modifiers,
    ) -> bool {
        if mods.control() != self.ctrl
            || mods.alt() != self.alt
            || mods.shift() != self.shift
        {
            return false;
        }
        match key {
            iced::keyboard::Key::Character(c) => {
                c.to_lowercase() == self.key_char
            }
            iced::keyboard::Key::Named(named) => {
                self.matches_named(named)
            }
            _ => false,
        }
    }

    fn matches_named(
        &self,
        named: &iced::keyboard::key::Named,
    ) -> bool {
        use iced::keyboard::key::Named;
        let expected = match self.key_char.as_str() {
            "delete" => Named::Delete,
            "insert" => Named::Insert,
            "home" => Named::Home,
            "end" => Named::End,
            "pageup" => Named::PageUp,
            "pagedown" => Named::PageDown,
            "tab" => Named::Tab,
            _ => return false,
        };
        *named == expected
    }
}

struct HighlightedText {
    language: String,
    spans: Vec<(Color, String)>,
}

struct Overlay {
    entries: Vec<Entry>,
    search_query: String,
    focused_index: usize,
    active_entry_id: Option<i64>,
    handles: HashMap<i64, iced_image::Handle>,
    highlights: HashMap<i64, HighlightedText>,
    horizontal: bool,
    #[allow(dead_code)] // used during init for highlights
    is_dark: bool,
    db: Database,
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

        let is_dark = detect_cosmic_theme()
            == iced::Theme::Dark;
        let highlights =
            build_highlights(&entries, is_dark);

        let _ = FAVORITE_HOTKEY.set(
            ParsedHotkey::parse(
                &config.hotkeys.toggle_favorite,
            ),
        );
        let _ = DELETE_HOTKEY.set(
            ParsedHotkey::parse(
                &config.hotkeys.delete_entry,
            ),
        );

        let overlay = Self {
            entries,
            search_query: String::new(),
            focused_index: 0,
            active_entry_id,
            handles,
            highlights,
            horizontal,
            is_dark,
            db,
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
            Message::CharTyped(c) => {
                self.search_query.push_str(&c);
                self.focused_index = 0;
            }
            Message::Backspace => {
                self.search_query.pop();
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
            Message::ToggleFocusedFavorite => {
                if let Some(entry) = self
                    .filtered_entries()
                    .get(self.focused_index)
                {
                    let id = entry.id;
                    return Task::done(
                        Message::ToggleFavorite(id),
                    );
                }
            }
            Message::ToggleFavorite(id) => {
                if let Err(e) =
                    self.db.toggle_favorite(id)
                {
                    tracing::error!(
                        "Failed to toggle \
                         favorite: {e}"
                    );
                } else if let Some(entry) = self
                    .entries
                    .iter_mut()
                    .find(|e| e.id == id)
                {
                    entry.favorite = !entry.favorite;
                }
            }
            Message::DeleteEntry => {
                let filtered = self.filtered_entries();
                let entry =
                    filtered.get(self.focused_index);
                if let Some(entry) = entry {
                    if !entry.favorite {
                        let id = entry.id;
                        if let Err(e) =
                            self.db.delete(id)
                        {
                            tracing::error!(
                                "Failed to delete \
                                 entry: {e}"
                            );
                        } else {
                            self.entries
                                .retain(|e| e.id != id);
                            let count =
                                self.filtered_entries()
                                    .len();
                            if count > 0
                                && self.focused_index
                                    >= count
                            {
                                self.focused_index =
                                    count - 1;
                            }
                        }
                    }
                }
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
                    return self.scroll_to_focused();
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
                    return self.scroll_to_focused();
                }
            }
        }
        Task::none()
    }

    fn scroll_to_focused(&self) -> Task<Message> {
        let count = self.filtered_entries().len();
        if count <= 1 {
            return Task::none();
        }
        let ratio = self.focused_index as f32
            / (count - 1) as f32;
        let offset = if self.horizontal {
            scrollable::RelativeOffset {
                x: Some(ratio),
                y: None,
            }
        } else {
            scrollable::RelativeOffset {
                x: None,
                y: Some(ratio),
            }
        };
        scrollable::snap_to(
            iced::widget::Id::new(SCROLLABLE_ID),
            offset,
        )
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
                .id(iced::widget::Id::new(SEARCH_ID))
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
                                self.highlights
                                    .get(&entry.id),
                                self.horizontal,
                            )
                        })
                        .collect();

                let sid = iced::widget::Id::new(
                    SCROLLABLE_ID,
                );
                if self.horizontal {
                    scrollable(
                        container(
                            Row::with_children(cards)
                                .spacing(CARD_SPACING),
                        )
                        .padding([0, 0, 16, 0]),
                    )
                    .id(sid)
                    .direction(
                        scrollable::Direction::Horizontal(
                            scrollable::Scrollbar::new(),
                        ),
                    )
                    .height(Length::Fill)
                    .into()
                } else {
                    scrollable(
                        container(
                            Column::with_children(cards)
                                .spacing(CARD_SPACING),
                        )
                        .padding([0, 16, 0, 0]),
                    )
                    .id(sid)
                    .height(Length::Fill)
                    .into()
                }
            };

        let search_widget = container(search)
            .width(Length::Fixed(300.0))
            .center_x(Length::Fill);

        let layout: Element<'_, Message> = column![
            search_widget,
            cards_widget,
        ]
        .spacing(8)
        .padding(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into();

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

        let terms: Vec<String> = self
            .search_query
            .to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect();

        self.entries
            .iter()
            .filter(|e| {
                let text_lower = e
                    .text_content()
                    .map(|t| t.to_lowercase())
                    .unwrap_or_default();
                let type_lower = self
                    .highlights
                    .get(&e.id)
                    .map(|hl| hl.language.to_lowercase())
                    .unwrap_or_default();
                let entry_type = match &e.entry_type {
                    crate::entry::EntryType::Image => {
                        "image"
                    }
                    crate::entry::EntryType::Url => "url",
                    crate::entry::EntryType::Text => "",
                };

                terms.iter().all(|term| {
                    text_lower.contains(term)
                        || type_lower.contains(term)
                        || entry_type.contains(term)
                })
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

fn build_highlights(
    entries: &[Entry],
    is_dark: bool,
) -> HashMap<i64, HighlightedText> {
    let mut map = HashMap::new();
    for entry in entries {
        if entry.entry_type
            != crate::entry::EntryType::Text
        {
            continue;
        }
        let Some(content) = entry.text_content() else {
            continue;
        };
        let truncated: String =
            content.chars().take(500).collect();
        let (language, raw_spans) =
            crate::entry::highlight_text(
                &truncated, is_dark,
            );
        let spans = raw_spans
            .into_iter()
            .map(|([r, g, b, a], s)| {
                (
                    Color::from_rgba8(r, g, b, f32::from(a) / 255.0),
                    s,
                )
            })
            .collect();
        map.insert(
            entry.id,
            HighlightedText { language, spans },
        );
    }
    map
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
    highlight: Option<&'a HighlightedText>,
    horizontal: bool,
) -> Element<'a, Message> {
    use crate::entry::{EntryType, is_image_url};

    let (star, star_color) = if entry.favorite {
        (
            "\u{2605}",
            Color::from_rgba8(218, 165, 32, 1.0),
        )
    } else {
        (
            "\u{2606}",
            Color::from_rgba8(160, 160, 160, 0.8),
        )
    };
    let entry_id = entry.id;
    let star_btn: Element<'a, Message> = button(
        text(star).size(18).color(star_color),
    )
    .on_press(Message::ToggleFavorite(entry_id))
    .padding([0, 2])
    .style(|_theme: &iced::Theme, _status| {
        button::Style {
            background: None,
            ..Default::default()
        }
    })
    .into();

    let active_badge = if active {
        "\u{1f4cb} "
    } else {
        ""
    };

    let (body, type_label): (Element<'a, Message>, &str) =
        match &entry.entry_type {
            EntryType::Image => {
                let el = if let Some(h) = handle {
                    iced_image::Image::new(h.clone())
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into()
                } else {
                    container(text("[Image]"))
                        .center_x(Length::Fill)
                        .center_y(Length::Fill)
                        .into()
                };
                (el, "Image")
            }
            EntryType::Url => {
                let url = entry
                    .text_content()
                    .unwrap_or("[url]");
                let emoji = if is_image_url(url) {
                    "\u{1f5bc}\u{fe0f} "
                } else {
                    "\u{1f517} "
                };
                if let Some(h) = handle {
                    let el = column![
                        iced_image::Image::new(
                            h.clone(),
                        )
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fill)
                        .height(Length::FillPortion(3)),
                        text(format!("{emoji}{url}"))
                            .size(11)
                            .wrapping(
                                Wrapping::WordOrGlyph,
                            )
                            .width(Length::Fill),
                    ]
                    .spacing(2)
                    .into();
                    (el, "Image URL")
                } else {
                    let el = text(format!(
                        "{emoji}{url}"
                    ))
                    .size(13)
                    .wrapping(Wrapping::WordOrGlyph)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into();
                    (el, "URL")
                }
            }
            EntryType::Text => {
                if let Some(hl) = highlight {
                    let is_code =
                        hl.language != "Plain Text";
                    let font_size =
                        if is_code { 12 } else { 14 };
                    let spans: Vec<
                        iced::widget::text::Span<
                            '_,
                            (),
                            iced::Font,
                        >,
                    > = hl
                        .spans
                        .iter()
                        .map(|(color, s)| {
                            iced::widget::text::Span::new(
                                s.as_str(),
                            )
                            .color(*color)
                            .size(font_size)
                        })
                        .collect();
                    let el = rich_text(spans)
                        .wrapping(Wrapping::Word)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into();
                    (el, &hl.language)
                } else {
                    let content = entry
                        .text_content()
                        .unwrap_or("[empty]");
                    let truncated: String =
                        content.chars().take(500).collect();
                    let el = text(truncated)
                        .size(14)
                        .wrapping(Wrapping::Word)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into();
                    (el, "Text")
                }
            }
        };

    let footer = Row::new()
        .push(
            text(format!("{active_badge}{type_label}"))
                .size(11),
        )
        .push(
            container(star_btn)
                .width(Length::Fill)
                .align_x(alignment::Horizontal::Right),
        )
        .align_y(alignment::Vertical::Center);

    let card_content = Column::new()
        .spacing(2)
        .width(Length::Fill)
        .height(Length::Fill)
        .push(body)
        .push(footer);

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

    let is_favorite = entry.favorite;
    let btn_style =
        move |theme: &iced::Theme,
              _status: button::Status|
              -> button::Style {
            let palette = theme.palette();
            let focus_green = iced::Color {
                r: 0.0,
                g: 0.5,
                b: 0.2,
                a: 1.0,
            };
            let favorite_gold = iced::Color {
                r: 0.85,
                g: 0.65,
                b: 0.13,
                a: 1.0,
            };
            let (bg, border) = if focused {
                let border_color = if is_favorite {
                    favorite_gold
                } else {
                    focus_green
                };
                (
                    iced::Color {
                        a: 0.20,
                        ..border_color
                    }
                    .into(),
                    iced::Border {
                        color: border_color,
                        width: 3.0,
                        radius: 8.0.into(),
                    },
                )
            } else if is_favorite {
                (
                    iced::Color {
                        a: 0.10,
                        ..favorite_gold
                    }
                    .into(),
                    iced::Border {
                        color: favorite_gold,
                        width: 2.0,
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
                    key, modifiers, ..
                },
            ) => {
                if let Some(hk) = FAVORITE_HOTKEY.get() {
                    if hk.matches(key, *modifiers) {
                        return Some(
                            Message::ToggleFocusedFavorite,
                        );
                    }
                }
                if let Some(hk) = DELETE_HOTKEY.get() {
                    if hk.matches(key, *modifiers) {
                        return Some(
                            Message::DeleteEntry,
                        );
                    }
                }
                match key {
                    iced::keyboard::Key::Named(named) => {
                        use iced::keyboard::key::Named;
                        match named {
                            Named::Escape
                            | Named::Enter => {
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
                            Named::Backspace => {
                                Some(Message::Backspace)
                            }
                            _ => None,
                        }
                    }
                    iced::keyboard::Key::Character(c) => {
                        if modifiers.control()
                            || modifiers.alt()
                        {
                            return None;
                        }
                        Some(Message::CharTyped(
                            c.to_string(),
                        ))
                    }
                    _ => None,
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

fn theme_for_overlay(
    _state: &Overlay,
    _window: window::Id,
) -> iced::Theme {
    detect_cosmic_theme()
}

fn detect_cosmic_theme() -> iced::Theme {
    let Ok(mode_config) =
        cosmic::cosmic_theme::ThemeMode::config()
    else {
        return iced::Theme::Dark;
    };
    let Ok(is_dark) =
        cosmic::cosmic_theme::ThemeMode::is_dark(
            &mode_config,
        )
    else {
        return iced::Theme::Dark;
    };
    if is_dark {
        iced::Theme::Dark
    } else {
        iced::Theme::Light
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_key() {
        let hk = ParsedHotkey::parse("delete");
        assert!(!hk.ctrl);
        assert!(!hk.alt);
        assert!(!hk.shift);
        assert_eq!(hk.key_char, "delete");
    }

    #[test]
    fn parse_ctrl_key() {
        let hk = ParsedHotkey::parse("ctrl+f");
        assert!(hk.ctrl);
        assert!(!hk.alt);
        assert!(!hk.shift);
        assert_eq!(hk.key_char, "f");
    }

    #[test]
    fn parse_multiple_modifiers() {
        let hk =
            ParsedHotkey::parse("ctrl+shift+x");
        assert!(hk.ctrl);
        assert!(!hk.alt);
        assert!(hk.shift);
        assert_eq!(hk.key_char, "x");
    }

    #[test]
    fn parse_all_modifiers() {
        let hk =
            ParsedHotkey::parse("ctrl+alt+shift+z");
        assert!(hk.ctrl);
        assert!(hk.alt);
        assert!(hk.shift);
        assert_eq!(hk.key_char, "z");
    }

    #[test]
    fn parse_case_insensitive() {
        let hk = ParsedHotkey::parse("Ctrl+F");
        assert!(hk.ctrl);
        assert_eq!(hk.key_char, "f");
    }

    #[test]
    fn parse_with_spaces() {
        let hk =
            ParsedHotkey::parse("ctrl + shift + a");
        assert!(hk.ctrl);
        assert!(hk.shift);
        assert_eq!(hk.key_char, "a");
    }

    #[test]
    fn parse_alt_key() {
        let hk = ParsedHotkey::parse("alt+s");
        assert!(!hk.ctrl);
        assert!(hk.alt);
        assert!(!hk.shift);
        assert_eq!(hk.key_char, "s");
    }
}

pub fn run() {
    let result = iced::daemon(
        Overlay::new,
        Overlay::update,
        Overlay::view,
    )
    .subscription(Overlay::subscription)
    .theme(theme_for_overlay)
    .run();

    if let Err(e) = result {
        tracing::error!("Overlay error: {e}");
        std::process::exit(1);
    }
}
