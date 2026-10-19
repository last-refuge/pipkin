//! Frozen user input only. No file paths, discovery, tools or execution authority.
use base64::Engine;
use pipkin_engine_protocol::{Block, ImageInput, limits};
use serde_json::{Value, json};

pub(crate) fn validate(text: &str, images: &[ImageInput]) -> Result<(), &'static str> {
    if text.len() > limits::MAX_PROMPT_BYTES
        || text.contains('\0')
        || (text.trim().is_empty() && images.is_empty())
        || images.len() > limits::MAX_IMAGES
    {
        return Err("invalid CLI input");
    }
    let mut bytes = serde_json::to_vec(text)
        .map_err(|_| "cannot encode input")?
        .len();
    for image in images {
        if !matches!(
            image.mime_type.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) || image.data.len() > limits::MAX_IMAGE_BYTES.div_ceil(3) * 4
        {
            return Err("unsupported or oversized CLI image");
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .map_err(|_| "invalid CLI image encoding")?;
        if decoded.is_empty() || decoded.len() > limits::MAX_IMAGE_BYTES {
            return Err("empty or oversized CLI image");
        }
        bytes = bytes.saturating_add(image.data.len()).saturating_add(256);
    }
    if !images.is_empty() && bytes > limits::MAX_FRAME_BYTES - 4096 {
        return Err("CLI input exceeds frame limit");
    }
    Ok(())
}

pub(crate) fn content(text: &str, images: &[ImageInput]) -> Vec<Block> {
    let mut content = Vec::new();
    if !text.is_empty() || images.is_empty() {
        content.push(Block::text(text));
    }
    content.extend(images.iter().map(|i| Block::image(&i.mime_type, &i.data)));
    content
}

pub(crate) fn image_blocks(images: &[ImageInput], mcp: bool) -> Vec<Value> {
    images.iter().map(|i| if mcp {
        json!({"type":"image","mimeType":i.mime_type,"data":i.data})
    } else {
        json!({"type":"image","source":{"type":"base64","media_type":i.mime_type,"data":i.data}})
    }).collect()
}

pub(crate) fn wire(text: &str, images: &[ImageInput]) -> Result<Vec<u8>, &'static str> {
    validate(text, images)?;
    if images.is_empty() {
        return Ok([text.as_bytes(), b"\n"].concat());
    }
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(json!({"type":"text","text":text}));
    }
    content.extend(image_blocks(images, false));
    let mut bytes = serde_json::to_vec(&json!({"type":"user","message":{"role":"user","content":content},"parent_tool_use_id":null}))
        .map_err(|_| "cannot encode CLI input")?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// CLI aliases resolve dynamically. Request only real CLI effort levels; never claim that
/// "off" disables reasoning on models whose thinking is always enabled.
pub(crate) fn thinking(value: Option<&str>) -> Result<Option<&str>, &'static str> {
    match value {
        None => Ok(None),
        Some(v @ ("low" | "medium" | "high" | "xhigh" | "max")) => Ok(Some(v)),
        _ => Err("unsupported Claude CLI effort; choose low, medium, high, xhigh or max"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(bytes: usize) -> ImageInput {
        ImageInput {
            mime_type: "image/png".into(),
            data: base64::engine::general_purpose::STANDARD.encode(vec![1; bytes]),
        }
    }
    #[test]
    fn features_input_checks_image_and_frame_bounds_without_touching_paths() {
        let i = image(1);
        assert!(validate("", std::slice::from_ref(&i)).is_ok());
        assert!(validate("", &[]).is_err());
        assert!(validate("x\0", std::slice::from_ref(&i)).is_err());
        assert!(validate("x", &vec![i.clone(); limits::MAX_IMAGES + 1]).is_err());
        assert!(
            validate(
                "x",
                &[ImageInput {
                    mime_type: "image/svg+xml".into(),
                    ..i.clone()
                }]
            )
            .is_err()
        );
        for bad in ["", "not base64", "/tmp/image.png", "a==="] {
            assert!(
                validate(
                    "x",
                    &[ImageInput {
                        data: bad.into(),
                        ..i.clone()
                    }]
                )
                .is_err()
            );
        }
        let max = image(limits::MAX_IMAGE_BYTES);
        assert!(validate("", std::slice::from_ref(&max)).is_ok());
        assert!(validate("", &[image(limits::MAX_IMAGE_BYTES + 1)]).is_err());
        assert!(validate("", &vec![max; 4]).is_err());
        let text = "é\n\"inert user input\"";
        let bytes = wire(text, std::slice::from_ref(&i)).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["message"]["content"][0]["text"], text);
        assert_eq!(
            value["message"]["content"][1]["source"]["media_type"],
            "image/png"
        );
        assert_eq!(wire(text, &[]).unwrap(), format!("{text}\n").as_bytes());
    }
    #[test]
    fn features_effort_does_not_silently_map_unsupported_levels() {
        for level in ["off", "minimal", "ultracode", "--tools", "HIGH", ""] {
            assert!(thinking(Some(level)).is_err());
        }
        assert_eq!(thinking(None).unwrap(), None);
    }
}
