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
    let ss = two_face::syntax::extra_newlines();
    let syntax = ss
        .find_syntax_by_first_line(text)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    syntax.name.clone()
}

fn ts_parse_score(
    lang: &tree_sitter::Language,
    text: &str,
) -> f64 {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(lang).is_err() {
        return 0.0;
    }
    let Some(tree) = parser.parse(text, None) else {
        return 0.0;
    };
    let root = tree.root_node();
    let total = root.descendant_count() as usize;
    if total == 0 {
        return 0.0;
    }
    let mut errors = 0usize;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            errors += 1;
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i as u32) {
                stack.push(child);
            }
        }
    }
    1.0 - (errors as f64 / total as f64)
}

fn ts_language_for_ext(
    ext: &str,
) -> Option<tree_sitter::Language> {
    Some(match ext {
        "json" => tree_sitter_json::LANGUAGE.into(),
        "py" => tree_sitter_python::LANGUAGE.into(),
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "js" => {
            tree_sitter_javascript::LANGUAGE.into()
        }
        "sh" => tree_sitter_bash::LANGUAGE.into(),
        "yaml" => tree_sitter_yaml::LANGUAGE.into(),
        "toml" => tree_sitter_toml_ng::LANGUAGE.into(),
        _ => return None,
    })
}

fn guess_candidates(text: &str) -> Vec<&'static str> {
    let t = text.trim();
    if t.is_empty() {
        return vec![];
    }

    if t.starts_with('{') && t.contains('"') {
        return vec!["json"];
    }
    if t.starts_with('[') {
        return vec!["json", "toml"];
    }
    if t.starts_with("<?xml") || t.starts_with("<svg") {
        return vec!["xml"];
    }
    if t.starts_with("<!") || t.starts_with("<html") {
        return vec!["html"];
    }
    if t.starts_with("---\n")
        || t.starts_with("---\r\n")
    {
        return vec!["yaml", "toml"];
    }

    let mut candidates = Vec::new();

    let first_lines: Vec<&str> =
        t.lines().take(15).collect();
    let trimmed_lines: Vec<&str> = first_lines
        .iter()
        .map(|l| l.trim_start())
        .collect();

    // Dockerfile / Containerfile
    if trimmed_lines.iter().any(|l| {
        l.starts_with("FROM ")
            || l.starts_with("RUN ")
            || l.starts_with("COPY ")
            || l.starts_with("WORKDIR ")
            || l.starts_with("ENTRYPOINT ")
            || l.starts_with("CMD ")
            || l.starts_with("EXPOSE ")
            || l.starts_with("ENV ")
            || l.starts_with("ARG ")
            || l.starts_with("ADD ")
    }) && trimmed_lines
        .iter()
        .any(|l| l.starts_with("FROM "))
    {
        candidates.push("dockerfile");
    }

    // Rust
    if t.contains("fn ")
        && (t.contains("let ") || t.contains("-> "))
    {
        candidates.push("rs");
    }

    // Python
    let has_py_imports = trimmed_lines.iter().any(|l| {
        l.starts_with("import ")
            || (l.starts_with("from ")
                && l.contains(" import "))
    });
    if (has_py_imports && !t.contains(';'))
        || (t.contains("def ") && t.contains(':'))
    {
        candidates.push("py");
    }

    // Shell / Bash (shebang or common shell patterns)
    if t.starts_with("#!") {
        if t.contains("python") {
            candidates.push("py");
        } else {
            candidates.push("sh");
        }
    } else if trimmed_lines.iter().any(|l| {
        l.starts_with("if [")
            || l.starts_with("for ")
                && l.contains(" in ")
                && l.ends_with("; do")
            || l.starts_with("case ")
                && l.contains(" in")
            || l.starts_with("then")
            || l.starts_with("fi")
            || l.starts_with("done")
            || l.starts_with("echo ")
            || l.starts_with("export ")
            || l.starts_with("set -")
    }) {
        candidates.push("sh");
    }

    // Go
    if t.contains("func ")
        && (t.contains("package ")
            || t.contains(":= "))
    {
        candidates.push("go");
    }

    // TypeScript
    if t.contains("interface ")
        && (t.contains(": string")
            || t.contains(": number")
            || t.contains(": boolean"))
    {
        candidates.push("ts");
    }

    // JavaScript
    if t.contains("function ")
        || t.contains("module.exports")
        || (t.contains("const ")
            && (t.contains("=>")
                || t.contains("require(")))
    {
        candidates.push("js");
    }

    // CSS
    if t.contains('{')
        && t.contains('}')
        && (t.contains("color:")
            || t.contains("display:")
            || t.contains("margin:")
            || t.contains("padding:")
            || t.contains("font-"))
    {
        candidates.push("css");
    }

    // SQL
    if t.contains("SELECT ")
        || t.contains("INSERT ")
        || t.contains("CREATE TABLE")
        || t.contains("ALTER TABLE")
        || t.contains("DROP TABLE")
    {
        candidates.push("sql");
    }

    // Markdown
    if trimmed_lines.iter().any(|l| {
        l.starts_with("# ") || l.starts_with("## ")
    }) && t.contains('\n')
        && (t.contains("```") || t.contains("- "))
    {
        candidates.push("md");
    }

    // TOML (section headers + key = value)
    let has_section_header =
        trimmed_lines.iter().any(|l| {
            l.starts_with('[')
                && l.ends_with(']')
                && !l.contains('{')
        });
    if has_section_header && t.contains(" = ") {
        candidates.push("toml");
    }

    // YAML (last — aggressive check)
    if candidates.is_empty()
        && t.lines().count() > 2
    {
        let non_empty: Vec<&str> = trimmed_lines
            .iter()
            .filter(|l| !l.is_empty())
            .copied()
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
            candidates.push("yaml");
        }
    }

    candidates
}

fn guess_extension(text: &str) -> Option<&'static str> {
    let candidates = guess_candidates(text);
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 {
        return Some(candidates[0]);
    }

    let mut best: Option<(&str, f64)> = None;
    for ext in &candidates {
        let Some(lang) = ts_language_for_ext(ext) else {
            continue;
        };
        let score = ts_parse_score(&lang, text);
        if best
            .as_ref()
            .map(|(_, s)| score > *s)
            .unwrap_or(true)
        {
            best = Some((ext, score));
        }
    }
    Some(best.map(|(ext, _)| ext).unwrap_or(candidates[0]))
}

#[allow(dead_code)]
fn guess_extension_heuristic_only(text: &str) -> Option<&'static str> {
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

fn ext_to_display_name(ext: &str) -> String {
    match ext {
        "rs" => "Rust",
        "py" => "Python",
        "js" => "JavaScript",
        "ts" => "TypeScript",
        "go" => "Go",
        "sh" => "Shell",
        "json" => "JSON",
        "yaml" => "YAML",
        "toml" => "TOML",
        "ini" => "INI",
        "sql" => "SQL",
        "xml" => "XML",
        "html" => "HTML",
        "css" => "CSS",
        "md" => "Markdown",
        "dockerfile" => "Dockerfile",
        other => other,
    }
    .to_string()
}

pub fn highlight_text(
    text: &str,
    is_dark: bool,
) -> (String, Vec<([u8; 4], String)>) {
    let ss =
        two_face::syntax::extra_newlines();
    let ts =
        syntect::highlighting::ThemeSet::load_defaults();

    let guessed_ext = guess_extension(text);

    let syntax = guessed_ext
        .and_then(|ext| ss.find_syntax_by_extension(ext))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let language = if syntax.name == "Plain Text" {
        guessed_ext
            .map(ext_to_display_name)
            .unwrap_or_else(|| "Plain Text".to_string())
    } else {
        syntax.name.clone()
    };

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
