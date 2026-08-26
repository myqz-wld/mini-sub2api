use serde_json::Map;
use serde_json::Value;

const DOCUMENTED_CONTENT_FIELDS: &[&str] = &[
    "type",
    "text",
    "prompt_cache_breakpoint",
    "image_url",
    "file_id",
    "detail",
    "input_audio",
    "audio_url",
    "file_data",
    "file_url",
    "filename",
    "annotations",
    "logprobs",
    "refusal",
    "encrypted_content",
    "data",
    "transcript",
];

pub(super) fn canonicalize(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let fields: &[&str] = match object.get("type").and_then(Value::as_str) {
        Some("input_text") => &["type", "text", "prompt_cache_breakpoint"],
        Some("output_text") => &["type", "text", "annotations", "logprobs"],
        Some("refusal") => &["type", "refusal"],
        Some("summary_text" | "reasoning_text" | "text") => &["type", "text"],
        Some("input_image") => &[
            "type",
            "image_url",
            "file_id",
            "detail",
            "prompt_cache_breakpoint",
        ],
        Some("input_audio") => &[
            "type",
            "audio_url",
            "input_audio",
            "prompt_cache_breakpoint",
        ],
        Some("input_file") => &[
            "type",
            "file_id",
            "file_url",
            "file_data",
            "filename",
            "detail",
            "prompt_cache_breakpoint",
        ],
        Some("output_audio") => &["type", "data", "transcript"],
        Some("encrypted_content") => &["type", "encrypted_content"],
        _ => DOCUMENTED_CONTENT_FIELDS,
    };
    canonicalize_breakpoint(object);
    canonicalize_audio(object);
    canonicalize_annotations(object);
    canonicalize_logprobs(object);
    reorder(object, fields);
}

fn canonicalize_breakpoint(object: &mut Map<String, Value>) {
    if let Some(breakpoint) = object
        .get_mut("prompt_cache_breakpoint")
        .and_then(Value::as_object_mut)
    {
        reorder(breakpoint, &["mode"]);
    }
}

fn canonicalize_audio(object: &mut Map<String, Value>) {
    if let Some(audio) = object.get_mut("input_audio").and_then(Value::as_object_mut) {
        reorder(audio, &["data", "format"]);
    }
}

fn canonicalize_annotations(object: &mut Map<String, Value>) {
    let Some(annotations) = object.get_mut("annotations").and_then(Value::as_array_mut) else {
        return;
    };
    for annotation in annotations {
        let Some(annotation) = annotation.as_object_mut() else {
            continue;
        };
        let fields: &[&str] = match annotation.get("type").and_then(Value::as_str) {
            Some("file_citation") => &["type", "file_id", "filename", "index"],
            Some("url_citation") => &["type", "url", "title", "start_index", "end_index"],
            Some("container_file_citation") => &[
                "type",
                "container_id",
                "file_id",
                "filename",
                "start_index",
                "end_index",
            ],
            Some("file_path") => &["type", "file_id", "index"],
            _ => &[
                "type",
                "file_id",
                "filename",
                "index",
                "url",
                "title",
                "start_index",
                "end_index",
                "container_id",
            ],
        };
        reorder(annotation, fields);
    }
}

fn canonicalize_logprobs(object: &mut Map<String, Value>) {
    let Some(logprobs) = object.get_mut("logprobs").and_then(Value::as_array_mut) else {
        return;
    };
    for logprob in logprobs {
        let Some(logprob) = logprob.as_object_mut() else {
            continue;
        };
        if let Some(top) = logprob
            .get_mut("top_logprobs")
            .and_then(Value::as_array_mut)
        {
            for entry in top {
                if let Some(entry) = entry.as_object_mut() {
                    reorder(entry, &["token", "bytes", "logprob"]);
                }
            }
        }
        reorder(logprob, &["token", "bytes", "logprob", "top_logprobs"]);
    }
}

fn reorder(object: &mut Map<String, Value>, order: &[&str]) {
    let mut existing = std::mem::take(object);
    for name in order {
        if let Some(value) = existing.remove(*name) {
            object.insert((*name).to_string(), value);
        }
    }
}
