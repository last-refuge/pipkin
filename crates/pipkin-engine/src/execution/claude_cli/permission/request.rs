use std::{collections::BTreeSet, fmt, marker::PhantomData};

use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, SeqAccess, Visitor, value::MapAccessDeserializer},
};
use sha2::{Digest, Sha256};

pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_DISPLAY_BYTES: usize = 32 * 1024;
/// SHA-256 of the complete original UTF-8 JSON bytes, including whitespace.
/// Not a truncated digest, normalized JSON hash, or a tool-use identifier.
pub type Fingerprint = [u8; 32];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    InvalidScope,
    TooLarge,
    InvalidJson,
    WrongScope,
    Unsupported,
    DisplayTooLarge,
}

/// Caller-captured identity. Canonical filesystem provenance is the caller's job.
#[derive(Clone, PartialEq, Eq)]
pub struct CliScope {
    session_uuid: String,
    canonical_cwd: String,
}

impl fmt::Debug for CliScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CliScope([redacted])")
    }
}

impl CliScope {
    pub fn new(session_uuid: &str, canonical_cwd: &str) -> Result<Self, RequestError> {
        let uuid = session_uuid.len() == 36
            && session_uuid.bytes().enumerate().all(|(i, b)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    b == b'-'
                } else {
                    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                }
            });
        // Canonical Unix cwd spelling, not Path::components (which hides /./).
        let cwd = canonical_cwd.len() <= 4096
            && canonical_cwd.starts_with('/')
            && !canonical_cwd.contains('\0')
            && (canonical_cwd == "/"
                || canonical_cwd[1..]
                    .split('/')
                    .all(|c| !c.is_empty() && c != "." && c != ".."));
        if !uuid || !cwd {
            return Err(RequestError::InvalidScope);
        }
        Ok(Self {
            session_uuid: session_uuid.into(),
            canonical_cwd: canonical_cwd.into(),
        })
    }
}

// No Debug/Serialize: raw input must not escape through diagnostics. Derived
// struct deserialization rejects duplicate known keys (including escaped keys)
// and deny_unknown_fields rejects all others, at both object levels.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    session_id: String,
    cwd: String,
    hook_event_name: String,
    tool_name: String,
    #[serde(deserialize_with = "object")]
    tool_input: WriteInput,
    // Documented common metadata is never opened, executed, displayed as authority,
    // or used to correlate a CLI tool-use ID. Its original bytes stay in the digest.
    #[serde(
        default,
        rename = "transcript_path",
        deserialize_with = "present_string"
    )]
    _transcript_path: Option<String>,
    #[serde(default, rename = "prompt_id", deserialize_with = "present_string")]
    _prompt_id: Option<String>,
    #[serde(
        default,
        rename = "scratchpad_dir",
        deserialize_with = "present_string"
    )]
    _scratchpad_dir: Option<String>,
    #[serde(default, deserialize_with = "present_string")]
    permission_mode: Option<String>,
    #[serde(default, rename = "effort", deserialize_with = "present_effort")]
    _effort: Option<Effort>,
    #[serde(
        default,
        rename = "permission_suggestions",
        deserialize_with = "inert_array"
    )]
    _permission_suggestions: (),
}

fn present_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}
fn present_effort<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Effort>, D::Error> {
    object(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Effort {
    #[serde(rename = "level")]
    _level: EffortLevel,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum EffortLevel {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

// Suggestions can evolve independently of tool inputs. They are inert JSON only:
// never adopt them, change a permission mode, or return updatedPermissions.
fn inert_array<'de, D: Deserializer<'de>>(d: D) -> Result<(), D::Error> {
    struct Array;
    impl<'de> Visitor<'de> for Array {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an inert array")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<(), A::Error> {
            while a.next_element::<IgnoredAny>()?.is_some() {}
            Ok(())
        }
    }
    d.deserialize_seq(Array)
}

// First pass discards values but rejects duplicate keys at EVERY depth, including
// ignored suggestions. serde_json's normal recursion limit remains enabled.
struct UniqueJson;
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Unique;
        impl<'de> Visitor<'de> for Unique {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded unique-key JSON")
            }
            fn visit_bool<E>(self, _: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_i64<E>(self, _: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_u64<E>(self, _: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_f64<E>(self, _: f64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_str<E>(self, _: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_unit<E>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<UniqueJson, A::Error> {
                while a.next_element::<UniqueJson>()?.is_some() {}
                Ok(UniqueJson)
            }
            fn visit_map<M: MapAccess<'de>>(self, mut m: M) -> Result<UniqueJson, M::Error> {
                let mut seen = BTreeSet::new();
                while let Some(key) = m.next_key::<String>()? {
                    if !seen.insert(key) {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                    m.next_value::<UniqueJson>()?;
                }
                Ok(UniqueJson)
            }
        }
        d.deserialize_any(Unique)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteInput {
    file_path: String,
    content: String,
}

// serde's struct visitor also accepts sequences; explicitly require JSON maps.
fn object<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    struct Object<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Object<T> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a closed object")
        }
        fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
            T::deserialize(MapAccessDeserializer::new(map))
        }
    }
    d.deserialize_map(Object(PhantomData))
}

/// Immutable reviewed request. Raw arguments are discarded after rendering.
/// Debug deliberately exposes neither argument text, cwd, UUID nor fingerprint.
pub struct PermissionRequest {
    pub(super) scope: CliScope,
    fingerprint: Fingerprint,
    display: String,
}

impl fmt::Debug for PermissionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PermissionRequest([redacted])")
    }
}

impl PermissionRequest {
    pub fn parse(input: &[u8], expected: &CliScope) -> Result<Self, RequestError> {
        if input.len() > MAX_INPUT_BYTES {
            return Err(RequestError::TooLarge);
        }
        serde_json::from_slice::<UniqueJson>(input).map_err(|_| RequestError::InvalidJson)?;
        let mut deserializer = serde_json::Deserializer::from_slice(input);
        let request: Envelope = object(&mut deserializer).map_err(|_| RequestError::InvalidJson)?;
        deserializer.end().map_err(|_| RequestError::InvalidJson)?;
        if request.session_id != expected.session_uuid || request.cwd != expected.canonical_cwd {
            return Err(RequestError::WrongScope);
        }
        if request.hook_event_name != "PermissionRequest"
            || request.tool_name != "Write"
            || request
                .permission_mode
                .as_deref()
                .is_some_and(|mode| mode != "default")
        {
            return Err(RequestError::Unsupported);
        }
        let mut display = String::new();
        append(&mut display, "Write\nfile_path: ")?;
        quoted(&mut display, &request.tool_input.file_path)?;
        append(&mut display, "\ncontent: ")?;
        quoted(&mut display, &request.tool_input.content)?;
        Ok(Self {
            scope: expected.clone(),
            fingerprint: Sha256::digest(input).into(),
            display,
        })
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    /// Complete ASCII plaintext. Every non-ASCII scalar is escaped, including
    /// bidi controls, combining marks, invisible format characters and confusables.
    /// Quotes/backslashes/control characters are unambiguously escaped as well.
    /// Only the fixed field separators contain literal newlines. Never truncate.
    pub fn argument_display(&self) -> &str {
        &self.display
    }
}

fn append(out: &mut String, text: &str) -> Result<(), RequestError> {
    if out.len() + text.len() > MAX_DISPLAY_BYTES {
        return Err(RequestError::DisplayTooLarge);
    }
    out.push_str(text);
    Ok(())
}

fn quoted(out: &mut String, value: &str) -> Result<(), RequestError> {
    append(out, "\"")?;
    for c in value.chars() {
        // escape_default renders all non-ASCII and controls, not just bidi ranges.
        for escaped in c.escape_default() {
            let mut bytes = [0; 4];
            append(out, escaped.encode_utf8(&mut bytes))?;
        }
    }
    append(out, "\"")
}
