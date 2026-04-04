use std::collections::HashMap;

pub type Mime = String;
pub type RawContent = Vec<u8>;
pub type MimeDataMap = HashMap<Mime, RawContent>;

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
                    return Some(s);
                }
            }
        }
        None
    }

    pub fn image_data(&self) -> Option<(&str, &[u8])> {
        let image_mimes = ["image/png", "image/jpeg", "image/jpg", "image/bmp"];

        for mime in &image_mimes {
            if let Some(data) = self.contents.get(*mime) {
                return Some((mime, data));
            }
        }
        None
    }
}

static URL_PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^https?://\S+$").unwrap()
});

pub fn detect_entry_type(data: &MimeDataMap) -> EntryType {
    let has_image = data
        .keys()
        .any(|m| m.starts_with("image/"));

    if has_image {
        return EntryType::Image;
    }

    let text_content = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"]
        .iter()
        .find_map(|mime| {
            data.get(*mime)
                .and_then(|d| std::str::from_utf8(d).ok())
        });

    if let Some(text) = text_content {
        let trimmed = text.trim();
        if URL_PATTERN.is_match(trimmed) {
            return EntryType::Url;
        }
    }

    EntryType::Text
}
