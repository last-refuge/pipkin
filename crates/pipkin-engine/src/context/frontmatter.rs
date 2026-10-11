//! YAML frontmatter (reference `coding-agent/src/utils/frontmatter.ts`).
//!
//! The text (BOM stripped, CRLF/CR normalized to LF) must start with `---`; the block ends at
//! the first `\n---` after it. Without a complete block the whole normalized text is the
//! (untrimmed) body; with one, the body after the closing marker is trimmed. YAML is parsed
//! with the YAML 1.2 core schema; an empty or comment-only block is an empty mapping.

use yaml_rust2::{Yaml, YamlLoader};

use super::js;

/// Parsed frontmatter. Only string and boolean field reads are needed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frontmatter {
    value: Option<Yaml>,
}

impl Frontmatter {
    fn field(&self, key: &str) -> Option<&Yaml> {
        match &self.value {
            Some(Yaml::Hash(hash)) => hash.get(&Yaml::String(key.to_string())),
            _ => None,
        }
    }

    /// The field when it is a YAML string (`typeof value === "string"`).
    pub fn string(&self, key: &str) -> Option<&str> {
        match self.field(key) {
            Some(Yaml::String(text)) => Some(text),
            _ => None,
        }
    }

    /// The field when it is a YAML boolean.
    pub fn boolean(&self, key: &str) -> Option<bool> {
        match self.field(key) {
            Some(Yaml::Boolean(value)) => Some(*value),
            _ => None,
        }
    }
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// `(yaml, body)`; `yaml` is `None` without a complete frontmatter block.
fn extract(content: &str) -> (Option<String>, String) {
    let normalized = normalize_newlines(js::strip_bom(content));
    if !normalized.starts_with("---") {
        return (None, normalized);
    }
    let Some(end) = normalized[3..].find("\n---").map(|at| at + 3) else {
        return (None, normalized);
    };
    // `slice(4, end)`: skip the one character after `---` (normally the newline); empty when
    // the closing marker starts right after `---`.
    let start = 3 + normalized[3..].chars().next().map_or(0, char::len_utf8);
    let yaml = if end > start {
        normalized[start..end].to_string()
    } else {
        String::new()
    };
    let body = js::trim(&normalized[end + 4..]).to_string();
    (Some(yaml), body)
}

/// Parse frontmatter and body. A YAML error is returned as
/// `"<reason> at line L, column C"` (line relative to the block, both 1-based).
pub fn parse(content: &str) -> Result<(Frontmatter, String), String> {
    let (yaml, body) = extract(content);
    let Some(yaml) = yaml.filter(|text| !text.is_empty()) else {
        return Ok((Frontmatter::default(), body));
    };
    let mut docs = YamlLoader::load_from_str(&yaml).map_err(|err| {
        format!(
            "{} at line {}, column {}",
            err.info(),
            err.marker().line(),
            err.marker().col() + 1
        )
    })?;
    if docs.len() > 1 {
        return Err("Source contains multiple documents".to_string());
    }
    let value = docs.pop().filter(|value| !matches!(value, Yaml::Null));
    Ok((Frontmatter { value }, body))
}

/// `stripFrontmatter`: the body only.
pub fn strip(content: &str) -> Result<String, String> {
    parse(content).map(|(_, body)| body)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from coding-agent/test/frontmatter.test.ts.

    #[test]
    fn parses_keys_strips_quotes_and_returns_body() {
        let input =
            "---\nname: \"skill-name\"\ndescription: 'A desc'\nfoo-bar: value\n---\n\nBody text";
        let (fm, body) = parse(input).unwrap();
        assert_eq!(fm.string("name"), Some("skill-name"));
        assert_eq!(fm.string("description"), Some("A desc"));
        assert_eq!(fm.string("foo-bar"), Some("value"));
        assert_eq!(body, "Body text");
    }

    #[test]
    fn normalizes_crlf() {
        let (_, body) = parse("---\r\nname: test\r\n---\r\nLine one\r\nLine two").unwrap();
        assert_eq!(body, "Line one\nLine two");
    }

    #[test]
    fn errors_on_invalid_yaml_with_position() {
        let err = parse("---\nfoo: [bar\n---\nBody").unwrap_err();
        assert!(err.contains(" at line "), "{err}");
        let err =
            parse("---\ndescription: Broken: unquoted colon\n---\nDo something.\n").unwrap_err();
        assert!(err.contains("line 1, column"), "{err}");
        // Duplicate keys are an error, as in the reference's `yaml` package.
        assert!(parse("---\na: 1\na: 2\n---\n").is_err());
    }

    #[test]
    fn parses_block_scalars() {
        let (fm, body) = parse("---\ndescription: |\n  Line one\n  Line two\n---\n\nBody").unwrap();
        assert_eq!(fm.string("description"), Some("Line one\nLine two\n"));
        assert_eq!(body, "Body");
    }

    #[test]
    fn missing_or_unterminated_frontmatter_returns_the_whole_text() {
        let (fm, body) = parse("Just text\nsecond line").unwrap();
        assert_eq!(fm, Frontmatter::default());
        assert_eq!(body, "Just text\nsecond line");
        let (_, body) = parse("---\nname: test\nBody without terminator").unwrap();
        assert_eq!(body, "---\nname: test\nBody without terminator");
    }

    #[test]
    fn empty_or_comment_only_frontmatter_is_empty() {
        let (fm, body) = parse("---\n# just a comment\n---\nBody").unwrap();
        assert_eq!(fm.string("name"), None);
        assert_eq!(body, "Body");
        let (_, body) = parse("---\n---\n  Body  ").unwrap();
        assert_eq!(body, "Body");
    }

    #[test]
    fn strip_trims_only_after_frontmatter() {
        assert_eq!(strip("---\nkey: value\n---\n\nBody\n").unwrap(), "Body");
        assert_eq!(
            strip("\n  No frontmatter body  \n").unwrap(),
            "\n  No frontmatter body  \n"
        );
    }

    #[test]
    fn non_string_fields_are_not_strings() {
        let (fm, _) =
            parse("---\nname: 12\ndisable-model-invocation: true\nx: 'true'\n---\n").unwrap();
        assert_eq!(fm.string("name"), None);
        assert_eq!(fm.boolean("disable-model-invocation"), Some(true));
        assert_eq!(fm.boolean("x"), None);
        // BOM before the opening marker is ignored.
        let (fm, _) = parse("\u{FEFF}---\nname: a\n---\n").unwrap();
        assert_eq!(fm.string("name"), Some("a"));
    }
}
