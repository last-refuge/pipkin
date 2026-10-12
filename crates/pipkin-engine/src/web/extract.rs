//! Readable text from fetched bodies without a browser or an HTML parsing dependency: script,
//! style and other non-content elements are dropped, block elements become line breaks,
//! entities are decoded and whitespace is collapsed. Nothing in the page is executed and no
//! markup survives into the text, so page content can only ever be evidence, not instructions
//! the engine acts on.

/// Text extracted from a body plus what the markup said about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extracted {
    pub text: String,
    pub title: Option<String>,
    /// `article:published_time` or `<time datetime>` as written in the page, never derived.
    pub published: Option<String>,
    /// The charset declared in a `<meta>` tag, when the response had none.
    pub meta_charset: Option<String>,
}

const SKIPPED: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "iframe", "object", "canvas",
];
const BLOCK: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "ul",
    "ol",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "tr",
    "table",
    "section",
    "article",
    "header",
    "footer",
    "nav",
    "aside",
    "main",
    "blockquote",
    "pre",
    "hr",
    "dd",
    "dt",
    "dl",
    "figure",
    "figcaption",
    "address",
    "form",
    "fieldset",
    "legend",
    "details",
    "summary",
    "option",
    "td",
    "th",
    "thead",
    "tbody",
    "caption",
];

/// Decode `&amp;`, `&#NNN;`, `&#xHH;` and the common named entities.
pub fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            "ndash" => Some('–'),
            "mdash" => Some('—'),
            "hellip" => Some('…'),
            "copy" => Some('©'),
            "reg" => Some('®'),
            "lsquo" => Some('‘'),
            "rsquo" => Some('’'),
            "ldquo" => Some('“'),
            "rdquo" => Some('”'),
            "laquo" => Some('«'),
            "raquo" => Some('»'),
            "middot" => Some('·'),
            "bull" => Some('•'),
            "euro" => Some('€'),
            "pound" => Some('£'),
            "times" => Some('×'),
            _ => entity
                .strip_prefix('#')
                .and_then(|n| {
                    if let Some(hex) = n.strip_prefix(['x', 'X']) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        n.parse::<u32>().ok()
                    }
                })
                .and_then(char::from_u32)
                .filter(|c| *c != '\0'),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn tag_name(tag: &str) -> String {
    tag.trim_start_matches(['/', '!'])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// The tag's attributes as `(lowercased name, value)`, read with quote awareness so a
/// name inside another attribute's value is never mistaken for an attribute.
fn attrs(tag: &str) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    let bytes = tag.as_bytes();
    let mut i = 0;
    // Skip the tag name.
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'/' {
        i += 1;
    }
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'='
            && bytes[i] != b'/'
        {
            i += 1;
        }
        if i == name_start {
            break;
        }
        let name = tag[name_start..i].to_ascii_lowercase();
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'=' {
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            let value_start = j;
            let value_end;
            if j < bytes.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
                let quote = bytes[j];
                j += 1;
                let start = j;
                while j < bytes.len() && bytes[j] != quote {
                    j += 1;
                }
                value_end = j;
                out.push((name, &tag[start..value_end]));
                i = (j + 1).min(bytes.len());
            } else {
                while j < bytes.len() && !bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                value_end = j;
                out.push((name, &tag[value_start..value_end]));
                i = j;
            }
        } else {
            out.push((name, ""));
            i = j;
        }
    }
    out
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    attrs(tag)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v)
}

/// Collapse runs of whitespace within a line and drop blank lines: one block per line keeps
/// the retained text compact for the model and for offsets.
fn tidy(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            continue;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.trim_end().to_owned()
}

/// Extract readable text from HTML. `max_chars` bounds the output (characters).
pub fn html_to_text(html: &str, max_chars: usize) -> Extracted {
    let mut text = String::with_capacity(html.len().min(max_chars * 4));
    let mut title = String::new();
    let mut in_title = false;
    let mut skip_until: Option<String> = None;
    let mut published = None;
    let mut meta_charset = None;
    let mut preformatted = 0usize;
    let mut rest = html;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            if skip_until.is_none() {
                push_text(&mut text, &mut title, in_title, rest, preformatted > 0);
            }
            break;
        };
        if skip_until.is_none() {
            push_text(
                &mut text,
                &mut title,
                in_title,
                &rest[..lt],
                preformatted > 0,
            );
        }
        rest = &rest[lt..];
        if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => rest = &rest[end + 3..],
                None => break,
            }
            continue;
        }
        let Some(gt) = find_tag_end(rest) else { break };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        let closing = tag.starts_with('/');
        let name = tag_name(tag);
        if let Some(until) = &skip_until {
            if closing && name == *until {
                skip_until = None;
            }
            continue;
        }
        if name.is_empty() {
            continue;
        }
        match name.as_str() {
            "meta" if !closing => {
                if let Some(charset) = attr(tag, "charset") {
                    meta_charset.get_or_insert_with(|| charset.trim().to_ascii_lowercase());
                } else if let (Some(http_equiv), Some(content)) =
                    (attr(tag, "http-equiv"), attr(tag, "content"))
                    && http_equiv.eq_ignore_ascii_case("content-type")
                    && let Some(cs) = charset_param(content)
                {
                    meta_charset.get_or_insert(cs);
                }
                let key = attr(tag, "property").or_else(|| attr(tag, "name"));
                if let (Some(key), Some(content)) = (key, attr(tag, "content"))
                    && matches!(
                        key.to_ascii_lowercase().as_str(),
                        "article:published_time" | "datepublished" | "date" | "dc.date"
                    )
                    && published.is_none()
                    && !content.trim().is_empty()
                {
                    published = Some(decode_entities(content.trim()));
                }
                continue;
            }
            "time" if !closing && published.is_none() => {
                if let Some(datetime) = attr(tag, "datetime")
                    && !datetime.trim().is_empty()
                {
                    published = Some(decode_entities(datetime.trim()));
                }
            }
            "title" => {
                in_title = !closing;
                continue;
            }
            "pre" => {
                if closing {
                    preformatted = preformatted.saturating_sub(1);
                } else {
                    preformatted += 1;
                }
            }
            _ => {}
        }
        if !closing && SKIPPED.contains(&name.as_str()) && !tag.ends_with('/') {
            skip_until = Some(name.clone());
            continue;
        }
        if BLOCK.contains(&name.as_str()) {
            if name == "li" && !closing {
                text.push_str("\n- ");
            } else {
                text.push('\n');
            }
        }
        if text.chars().count() > max_chars + 4096 {
            break;
        }
    }
    let mut text = tidy(&decode_entities(&text));
    if text.chars().count() > max_chars {
        let cut = text
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        text.truncate(cut);
    }
    let title = tidy(&decode_entities(&title));
    Extracted {
        text,
        title: (!title.is_empty()).then_some(title),
        published,
        meta_charset,
    }
}

fn push_text(text: &mut String, title: &mut String, in_title: bool, piece: &str, pre: bool) {
    if in_title {
        title.push_str(piece);
    } else if pre {
        text.push_str(piece);
    } else {
        // Inline text: newlines in the source are just whitespace.
        for (i, part) in piece.split(['\n', '\r']).enumerate() {
            if i > 0 {
                text.push(' ');
            }
            text.push_str(part);
        }
    }
}

/// The index of the `>` that ends the tag at the start of `s`, honouring quoted attributes.
fn find_tag_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut quote: Option<u8> = None;
    for (i, b) in bytes.iter().enumerate().skip(1) {
        match (quote, *b) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"') | (None, b'\'') => quote = Some(*b),
            (None, b'>') => return Some(i),
            _ => {}
        }
    }
    None
}

/// The `charset=` parameter of a content type, lowercased.
pub fn charset_param(content_type: &str) -> Option<String> {
    content_type
        .split(';')
        .skip(1)
        .map(str::trim)
        .find_map(|p| {
            let (k, v) = p.split_once('=')?;
            k.trim()
                .eq_ignore_ascii_case("charset")
                .then(|| v.trim().trim_matches('"').to_ascii_lowercase())
        })
}

/// Decode bytes as the named charset. UTF-8 (with invalid sequences replaced), or one of the
/// single-byte Latin encodings mapped byte for byte; `lossy` reports whether anything was
/// replaced or an unknown charset was treated as UTF-8.
pub fn decode_body(bytes: &[u8], charset: Option<&str>) -> (String, bool) {
    let charset = charset.unwrap_or("utf-8");
    match charset {
        "utf-8" | "utf8" | "us-ascii" | "ascii" => match std::str::from_utf8(bytes) {
            Ok(s) => (s.to_owned(), false),
            Err(_) => (String::from_utf8_lossy(bytes).into_owned(), true),
        },
        "iso-8859-1" | "latin1" | "latin-1" | "windows-1252" | "cp1252" => {
            // Windows-1252 differs from Latin-1 only in 0x80..0x9F; map those to the
            // characters Windows-1252 assigns and the rest byte-for-byte.
            let s = bytes
                .iter()
                .map(|&b| match b {
                    0x80 => '€',
                    0x82 => '‚',
                    0x83 => 'ƒ',
                    0x84 => '„',
                    0x85 => '…',
                    0x86 => '†',
                    0x87 => '‡',
                    0x88 => 'ˆ',
                    0x89 => '‰',
                    0x8a => 'Š',
                    0x8b => '‹',
                    0x8c => 'Œ',
                    0x8e => 'Ž',
                    0x91 => '‘',
                    0x92 => '’',
                    0x93 => '“',
                    0x94 => '”',
                    0x95 => '•',
                    0x96 => '–',
                    0x97 => '—',
                    0x98 => '˜',
                    0x99 => '™',
                    0x9a => 'š',
                    0x9b => '›',
                    0x9c => 'œ',
                    0x9e => 'ž',
                    0x9f => 'Ÿ',
                    b => b as char,
                })
                .collect();
            (s, false)
        }
        _ => (String::from_utf8_lossy(bytes).into_owned(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_styles_and_markup_never_reach_the_text() {
        let html = r#"<!doctype html><html><head><title>Hello &amp; welcome</title>
        <meta property="article:published_time" content="2026-01-02T03:04:05Z">
        <style>body{color:red}</style><script>alert("x")</script></head>
        <body><h1>Heading</h1><p>One <b>two</b> three.</p><!-- hidden -->
        <ul><li>a</li><li>b &lt;c&gt;</li></ul><script type="module">evil()</script>
        <noscript>no js</noscript><pre>  keep
   this</pre><div onclick="x()">Click <a href="/x" title="t>">here</a></div></body></html>"#;
        let e = html_to_text(html, 10_000);
        assert_eq!(e.title.as_deref(), Some("Hello & welcome"));
        assert_eq!(e.published.as_deref(), Some("2026-01-02T03:04:05Z"));
        assert!(!e.text.contains("alert"), "{}", e.text);
        assert!(!e.text.contains("evil"), "{}", e.text);
        assert!(!e.text.contains("no js"), "{}", e.text);
        assert!(!e.text.contains("color:red"), "{}", e.text);
        assert!(
            !e.text.contains("</") && !e.text.contains("<p") && !e.text.contains("<div"),
            "{}",
            e.text
        );
        assert!(e.text.contains("Heading\nOne two three."), "{}", e.text);
        assert!(e.text.contains("- a\n- b <c>"), "{}", e.text);
        assert!(e.text.contains("keep\nthis"), "{}", e.text);
        assert!(e.text.ends_with("Click here"), "{}", e.text);
    }

    #[test]
    fn output_is_bounded_and_entities_decode() {
        let html = format!("<p>{}</p>", "word ".repeat(5_000));
        let e = html_to_text(&html, 100);
        assert_eq!(e.text.chars().count(), 100);
        assert_eq!(
            decode_entities("a &#65;&#x42; &unknown; &amp;&nbsp;x"),
            "a AB &unknown; &\u{a0}x"
        );
        assert_eq!(decode_entities("&#0;&lt;"), "&#0;<");
    }

    #[test]
    fn charsets_are_read_from_headers_and_meta_and_bodies_decode() {
        assert_eq!(
            charset_param("text/html; charset=ISO-8859-1").as_deref(),
            Some("iso-8859-1")
        );
        assert_eq!(charset_param("text/html").as_deref(), None);
        let e = html_to_text(r#"<meta charset="windows-1252"><p>x</p>"#, 100);
        assert_eq!(e.meta_charset.as_deref(), Some("windows-1252"));
        let e = html_to_text(
            r#"<meta http-equiv="Content-Type" content="text/html; charset=utf-8">"#,
            100,
        );
        assert_eq!(e.meta_charset.as_deref(), Some("utf-8"));
        assert_eq!(
            decode_body(b"caf\xe9 \x93x\x94", Some("windows-1252")),
            ("café “x”".into(), false)
        );
        assert_eq!(decode_body(b"ok", None), ("ok".into(), false));
        let (s, lossy) = decode_body(b"bad \xff byte", Some("utf-8"));
        assert!(lossy);
        assert!(s.contains("bad"));
        assert!(
            decode_body(b"x", Some("shift_jis")).1,
            "unknown charsets are reported lossy"
        );
    }

    #[test]
    fn unclosed_tags_and_attribute_angle_brackets_do_not_derail_extraction() {
        let e = html_to_text(
            "<p>a <span data-x=\"1>2\">b</span> c<p>d</p><br/>e<script>",
            100,
        );
        assert_eq!(e.text, "a b c\nd\ne");
        assert_eq!(
            attr("meta content=\"x; charset=y\" charset=z", "charset"),
            Some("z")
        );
        assert_eq!(attr("a href=/x title='t>'", "title"), Some("t>"));
        assert_eq!(attr("a href=/x", "href"), Some("/x"));
        assert_eq!(attr("input disabled", "disabled"), Some(""));
        let e = html_to_text("<time datetime=\"2025-05-05\">May</time> text", 100);
        assert_eq!(e.published.as_deref(), Some("2025-05-05"));
        assert_eq!(e.text, "May text");
    }
}
