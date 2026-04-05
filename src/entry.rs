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

    pub fn content_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        if let Some(text) = self.text_content() {
            text.as_bytes().hash(&mut hasher);
        } else if let Some((_mime, data)) = self.image_data()
        {
            data.hash(&mut hasher);
        }
        hasher.finish()
    }

    pub fn thumbnail_data(&self) -> Option<&[u8]> {
        self.contents
            .get(THUMBNAIL_MIME)
            .map(|d| d.as_slice())
    }
}

#[allow(dead_code)] // available for future CLI use
pub fn detect_syntax_name(text: &str) -> String {
    let ss = syntect::parsing::SyntaxSet::load_defaults_newlines();
    let syntax = ss
        .find_syntax_by_first_line(text)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    syntax.name.clone()
}

fn guess_extension(text: &str) -> Option<&'static str> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }

    if (t.starts_with('{') && t.contains('"'))
        || (t.starts_with('[') && t.contains('{'))
    {
        return Some("json");
    }
    if t.starts_with("<?xml")
        || t.starts_with("<svg")
    {
        return Some("xml");
    }
    if t.starts_with("<!") || t.starts_with("<html") {
        return Some("html");
    }
    if t.starts_with("---\n") || t.starts_with("---\r\n")
    {
        return Some("yaml");
    }
    if t.contains("fn ")
        && (t.contains("let ") || t.contains("-> "))
    {
        return Some("rs");
    }
    if t.starts_with("#!")
        && t.lines().next().is_some_and(|l| {
            l.contains("python")
        })
    {
        return Some("py");
    }
    if t.contains("def ")
        && t.contains(':')
        && !t.contains(';')
    {
        return Some("py");
    }
    if (t.starts_with("import ")
        && t.contains('\n')
        && !t.contains(';'))
        || (t.starts_with("from ")
            && t.contains(" import "))
    {
        return Some("py");
    }
    if t.starts_with("#!") {
        return Some("sh");
    }
    if t.contains("func ")
        && (t.contains("package ")
            || t.contains(":= "))
    {
        return Some("go");
    }
    if t.contains("function ")
        || (t.contains("const ")
            && (t.contains("=>")
                || t.contains("require(")))
        || t.contains("module.exports")
    {
        return Some("js");
    }
    if t.contains("SELECT ")
        || t.contains("INSERT ")
        || t.contains("CREATE TABLE")
    {
        return Some("sql");
    }
    if t.lines()
        .take(5)
        .any(|l| l.starts_with('[') && l.ends_with(']'))
        && t.contains(" = ")
    {
        return Some("ini");
    }
    if t.lines().count() > 2 {
        let non_empty: Vec<&str> = t
            .lines()
            .take(15)
            .filter(|l| !l.trim().is_empty())
            .collect();
        let has_kv = non_empty.iter().any(|l| {
            !l.starts_with(' ')
                && !l.starts_with('#')
                && l.contains(": ")
        });
        let all_yaml = non_empty.iter().all(|l| {
            l.contains(": ")
                || l.starts_with('#')
                || l.starts_with("- ")
                || l.starts_with("  ")
        });
        if has_kv && all_yaml {
            return Some("yaml");
        }
    }
    None
}

pub fn highlight_text(
    text: &str,
    is_dark: bool,
) -> (String, Vec<([u8; 4], String)>) {
    let ss =
        syntect::parsing::SyntaxSet::load_defaults_newlines();
    let ts =
        syntect::highlighting::ThemeSet::load_defaults();

    let syntax = ss
        .find_syntax_by_first_line(text)
        .or_else(|| {
            guess_extension(text).and_then(|ext| {
                ss.find_syntax_by_extension(ext)
            })
        })
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let language = syntax.name.clone();

    let theme_name = if is_dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    let theme = &ts.themes[theme_name];

    let mut highlighter =
        syntect::easy::HighlightLines::new(syntax, theme);

    let mut spans = Vec::new();
    for line in syntect::util::LinesWithEndings::from(text)
    {
        let Ok(ranges) =
            highlighter.highlight_line(line, &ss)
        else {
            spans.push(([255, 255, 255, 255], line.to_string()));
            continue;
        };
        for (style, fragment) in ranges {
            let fg = style.foreground;
            spans.push((
                [fg.r, fg.g, fg.b, fg.a],
                fragment.to_string(),
            ));
        }
    }

    (language, spans)
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
