use std::collections::HashMap;

pub type Mime = String;
pub type RawContent = Vec<u8>;
pub type MimeDataMap = HashMap<Mime, RawContent>;

pub const THUMBNAIL_MIME: &str = "x-clipbro/thumbnail";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryType {
    Text,
    Image,
    Url,
}

impl EntryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntryType::Text => "text",
            EntryType::Image => "image",
            EntryType::Url => "url",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "image" => EntryType::Image,
            "url" => EntryType::Url,
            _ => EntryType::Text,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: i64,
    #[allow(dead_code)] // loaded from DB, used for sort/display
    pub created_at: i64,
    pub entry_type: EntryType,
    pub favorite: bool,
    pub contents: MimeDataMap,
}

impl Entry {
    pub fn text_content(&self) -> Option<&str> {
        let text_mimes = [
            "text/plain;charset=utf-8",
            "text/plain",
            "UTF8_STRING",
            "STRING",
            "TEXT",
        ];

        for mime in &text_mimes {
            if let Some(data) = self.contents.get(*mime) {
                if let Ok(s) = std::str::from_utf8(data) {
                    if !s.contains('\0') {
                        return Some(s);
                    }
                }
            }
        }
        None
    }

    pub fn image_data(&self) -> Option<(&str, &[u8])> {
        let image_mimes =
            ["image/png", "image/jpeg", "image/jpg", "image/bmp"];

        for mime in &image_mimes {
            if let Some(data) = self.contents.get(*mime) {
                return Some((mime, data));
            }
        }
        None
    }

    pub fn thumbnail_data(&self) -> Option<&[u8]> {
        self.contents
            .get(THUMBNAIL_MIME)
            .map(|d| d.as_slice())
    }
}

static URL_PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^https?://\S+$").unwrap()
});

static IMAGE_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg",
];

pub fn is_image_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    IMAGE_EXTENSIONS.iter().any(|ext| path.ends_with(ext))
}

pub fn detect_entry_type(data: &MimeDataMap) -> EntryType {
    let has_image = data
        .keys()
        .any(|m| m.starts_with("image/"));

    if has_image {
        return EntryType::Image;
    }

    if data.contains_key("text/uri-list")
        || data.contains_key("text/x-moz-url")
    {
        return EntryType::Url;
    }

    let text_content =
        ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"]
            .iter()
            .find_map(|mime| {
                data.get(*mime)
                    .and_then(|d| std::str::from_utf8(d).ok())
                    .filter(|s| !s.contains('\0'))
            });

    if let Some(text) = text_content {
        let trimmed = text.trim();
        if URL_PATTERN.is_match(trimmed) {
            return EntryType::Url;
        }
    }

    EntryType::Text
}
