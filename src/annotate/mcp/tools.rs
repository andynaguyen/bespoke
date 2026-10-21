//! The tools Annotate's MCP server offers. Each reads captures from the captures folder and
//! answers with MCP content: text, and PNG images no larger than `MAX_IMAGE_EDGE` on a side.

use std::io::Cursor;
use std::path::Path;

use anyhow::{ensure, Context, Result};
use base64::Engine as _;
use chrono::{Local, TimeZone};
use image::{imageops::FilterType, ImageFormat, ImageReader};
use scrawl::host::Capture;
use serde_json::{json, Value};

/// The longest side of an image sent to an agent. Contact sheets are already this wide, and full
/// Retina frames shrink to it.
const MAX_IMAGE_EDGE: u32 = 1600;
/// The most keyframes one `get_frames` call returns.
const MAX_FRAMES: usize = 8;
/// Manifest fields `get_manifest` leaves out unless asked.
const MANIFEST_EXTRAS: [&str; 3] = ["cursor", "windows", "strokes"];

/// The `tools/list` result.
pub fn list() -> Value {
    let capture = json!({
        "type": "string",
        "description": "`latest` (the default), an id like `scrawl-1792561826739-14c29-6`, or the bundle path from a pasted capture."
    });
    let read_only = json!({"readOnlyHint": true, "openWorldHint": false});
    json!([
        {
            "name": "get_capture",
            "title": "Get capture",
            "description": "Read an Annotate capture: the summary of its keyframes, with what was said and done at each, and the contact sheet showing every keyframe. Start here when a message contains a capture or mentions something the user just recorded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "capture": capture,
                    "full": {
                        "type": "boolean",
                        "description": "Return the uncropped contact sheet. Only differs when the summary has a `Cropped to:` line."
                    }
                }
            },
            "annotations": read_only,
        },
        {
            "name": "get_frames",
            "title": "Get keyframes",
            "description": format!("Get keyframes at full size by their `#N` numbers from the summary, up to {MAX_FRAMES} per call."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "capture": capture,
                    "keyframes": {
                        "type": "array",
                        "items": {"type": "integer", "minimum": 0},
                        "minItems": 1,
                        "maxItems": MAX_FRAMES,
                        "description": "Keyframe numbers, like [3] for #3."
                    },
                    "full": {
                        "type": "boolean",
                        "description": "Return the whole screen instead of the crop. Only differs when the summary has a `Cropped to:` line."
                    }
                },
                "required": ["keyframes"]
            },
            "annotations": read_only,
        },
        {
            "name": "list_captures",
            "title": "List captures",
            "description": "List recent Annotate captures, newest first, with their ids, times, lengths and the app recorded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 50, "description": "How many to list (default 10)."}
                }
            },
            "annotations": read_only,
        },
        {
            "name": "get_manifest",
            "title": "Get manifest",
            "description": "Get a capture's scrawl.json: marks with their shape and box, click and key events, word timings, and how each keyframe was cropped. Cursor samples, on-screen window lists and stroke points are left out unless named in `include`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "capture": capture,
                    "include": {
                        "type": "array",
                        "items": {"type": "string", "enum": MANIFEST_EXTRAS},
                        "description": "Fields to keep: `cursor`, `windows` (every on-screen window over time), `strokes` (the points of each stroke)."
                    }
                }
            },
            "annotations": read_only,
        },
    ])
}

/// Runs tool `name` against the captures in `captures`. `None` for a tool that doesn't exist;
/// `Some(Err)` for one that failed, which the agent sees as a tool error.
pub fn call(captures: &Path, name: &str, arguments: &Value) -> Option<Result<Vec<Value>>> {
    Some(match name {
        "get_capture" => get_capture(captures, arguments),
        "get_frames" => get_frames(captures, arguments),
        "list_captures" => list_captures(captures, arguments),
        "get_manifest" => get_manifest(captures, arguments),
        _ => return None,
    })
}

fn get_capture(captures: &Path, arguments: &Value) -> Result<Vec<Value>> {
    let capture = find(captures, arguments)?;
    let sheet = capture.contact_sheet(flag(arguments, "full"))?;
    Ok(vec![text(capture.summary()), image(&sheet)?])
}

fn get_frames(captures: &Path, arguments: &Value) -> Result<Vec<Value>> {
    let capture = find(captures, arguments)?;
    let indexes: Vec<usize> = arguments
        .get("keyframes")
        .and_then(Value::as_array)
        .map(|numbers| {
            numbers
                .iter()
                .filter_map(Value::as_u64)
                .map(|n| n as usize)
                .collect()
        })
        .unwrap_or_default();
    ensure!(
        !indexes.is_empty(),
        "Pass the keyframes to get, like [3] for #3."
    );
    ensure!(
        indexes.len() <= MAX_FRAMES,
        "Ask for at most {MAX_FRAMES} keyframes at a time."
    );
    let full = flag(arguments, "full");
    let mut content = Vec::with_capacity(indexes.len() * 2);
    for index in indexes {
        let (label, path) = capture.keyframe(index, full)?;
        content.push(text(label));
        content.push(image(&path)?);
    }
    Ok(content)
}

fn list_captures(captures: &Path, arguments: &Value) -> Result<Vec<Value>> {
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(10, |n| n.clamp(1, 50) as usize);
    let recent = Capture::recent(captures, limit);
    if recent.is_empty() {
        return Ok(vec![text("No captures yet.".to_owned())]);
    }
    let lines: Vec<String> = recent
        .iter()
        .map(|capture| {
            let keyframes = match capture.keyframe_count() {
                1 => "1 keyframe".to_owned(),
                n => format!("{n} keyframes"),
            };
            let mut line = format!(
                "{} · {} · {} · {} · {keyframes}",
                capture.id(),
                local_time(capture.created_at_ms()),
                capture.duration(),
                capture.target(),
            );
            if let Some(note) = capture.note() {
                line.push_str(&format!(" · Note: {note}"));
            }
            line
        })
        .collect();
    Ok(vec![text(lines.join("\n"))])
}

fn get_manifest(captures: &Path, arguments: &Value) -> Result<Vec<Value>> {
    let capture = find(captures, arguments)?;
    let include: Vec<&str> = arguments
        .get("include")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    Ok(vec![text(capture.manifest(&include)?)])
}

fn find(captures: &Path, arguments: &Value) -> Result<Capture> {
    let reference = arguments
        .get("capture")
        .and_then(Value::as_str)
        .unwrap_or("latest");
    Capture::find(captures, reference)
}

fn flag(arguments: &Value, key: &str) -> bool {
    arguments.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn local_time(ms: u64) -> String {
    Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn text(text: String) -> Value {
    json!({"type": "text", "text": text})
}

fn image(path: &Path) -> Result<Value> {
    let png = std::fs::read(path).with_context(|| format!("Could not read {}", path.display()))?;
    let png = shrink(png).with_context(|| format!("Could not read {}", path.display()))?;
    Ok(json!({
        "type": "image",
        "data": base64::engine::general_purpose::STANDARD.encode(png),
        "mimeType": "image/png",
    }))
}

/// Shrinks a PNG whose longest side passes `MAX_IMAGE_EDGE`. Smaller ones pass through as they
/// are, without decoding.
fn shrink(png: Vec<u8>) -> Result<Vec<u8>> {
    let (width, height) =
        ImageReader::with_format(Cursor::new(&png), ImageFormat::Png).into_dimensions()?;
    if width.max(height) <= MAX_IMAGE_EDGE {
        return Ok(png);
    }
    let resized = image::load_from_memory_with_format(&png, ImageFormat::Png)?.resize(
        MAX_IMAGE_EDGE,
        MAX_IMAGE_EDGE,
        FilterType::Triangle,
    );
    let mut out = Cursor::new(Vec::new());
    resized.write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
}
