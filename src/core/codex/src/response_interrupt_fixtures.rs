use serde_json::{Value, json};

// Each sequence ends at the first invalid event. Both context publication and socket reuse
// must reject the same history-integrity violations, including tool dependency carriers.
pub(crate) fn inconsistent_outputs(response_id: &str) -> Vec<(String, Vec<Value>)> {
    let done = json!({"type":"response.output_item.done","output_index":0,"item":{
        "type":"message","id":"msg_partial","status":"completed","role":"assistant",
        "content":[{"type":"output_text","text":"must not reappear"}]}});
    let delta = json!({"type":"response.output_text.delta","output_index":0,
        "item_id":"msg_partial","delta":"partial"});
    let discard = json!({"type":"response.output_item.interrupted","output_index":0,
        "response_id":response_id,"item_id":"msg_partial"});
    let mut cases = Vec::new();
    for status in [
        "in_progress",
        "incomplete",
        "queued",
        "searching",
        "generating",
        "interpreting",
    ] {
        for kind in ["message", "function_call"] {
            let mut event = done.clone();
            if kind == "function_call" {
                event["item"] = json!({"type":kind,"id":"msg_partial","call_id":"call_partial",
                    "name":"probe","arguments":"{}"});
            }
            event["item"]["status"] = status.into();
            cases.push((format!("unfinished_{kind}_{status}"), vec![event]));
        }
    }
    for index in [0, 1] {
        let mut partial = delta.clone();
        partial["output_index"] = index.into();
        let mut discarded = discard.clone();
        discarded["output_index"] = index.into();
        cases.push((
            format!("completed_then_discarded_{index}"),
            vec![done.clone(), partial, discarded],
        ));
    }
    let mut index_only = delta.clone();
    index_only.as_object_mut().unwrap().remove("item_id");
    for kind in [
        "response.output_item.added",
        "response.output_text.delta",
        "response.output_item.done",
    ] {
        for has_index in [false, true] {
            let mut revived = if kind == "response.output_text.delta" {
                delta.clone()
            } else {
                done.clone()
            };
            revived["type"] = kind.into();
            if has_index {
                revived["output_index"] = 1.into();
            } else {
                revived.as_object_mut().unwrap().remove("output_index");
            }
            cases.push((
                format!("discarded_id_revived_{kind}_{has_index}"),
                vec![index_only.clone(), discard.clone(), revived],
            ));
        }
    }
    cases
}

pub(crate) fn inconsistent_terminals(
    response_id: &str,
) -> Vec<(String, Vec<Value>, Option<Value>)> {
    let mut cases = Vec::new();
    for with_id in [false, true] {
        let mut done = json!({"type":"response.output_item.done","item":{
            "type":"message","role":"assistant","status":"completed","content":[]}});
        let mut delta =
            json!({"type":"response.output_text.delta","output_index":0,"delta":"partial"});
        let mut discard = json!({"type":"response.output_item.interrupted","output_index":0,"response_id":response_id});
        if with_id {
            done["item"]["id"] = "msg_partial".into();
        } else {
            delta["item_id"] = "msg_partial".into();
            discard["item_id"] = "msg_partial".into();
        }
        for footer in [None, Some(json!([]))] {
            cases.push((
                format!("discarded_implicit_index_{with_id}"),
                vec![done.clone(), delta.clone(), discard.clone()],
                footer,
            ));
        }
    }
    let done = json!({"type":"response.output_item.done","output_index":0,"item":{
        "type":"message","role":"assistant","status":"completed","content":[]}});
    let mut revived = done["item"].clone();
    revived["id"] = "msg_discarded".into();
    cases.push(("footer_assigns_discarded_id".into(), vec![done,
        json!({"type":"response.output_text.delta","output_index":1,"item_id":"msg_discarded","delta":"partial"}),
        json!({"type":"response.output_item.interrupted","output_index":1,"item_id":"msg_discarded","response_id":response_id})],
        Some(json!([revived]))));
    cases
}
