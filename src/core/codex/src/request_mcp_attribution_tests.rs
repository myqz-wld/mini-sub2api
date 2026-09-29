use super::*;
use serde_json::json;

fn source(turn: &str, name: &str) -> Value {
    json!({"status":"complete","sources":[{"server_name":name,"tool_name":"lookup","first_turn_id":turn}]})
}

#[tokio::test]
async fn attribution_maps_only_existing_scoped_turns_and_survives_reload() {
    let temp = tempfile::tempdir().unwrap();
    let namespace = "mcp-attribution";
    let owner = "acct_mcp";
    for reload in [false, true] {
        let store = crate::request_state_store::RequestStateStore::new(temp.path().to_path_buf());
        let value = source("synthetic-turn", "synthetic-server");
        let attribution = read(
            json!({"client_metadata":{KEY:value.to_string()}})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let emitted = store
            .edit(namespace, owner, "key-a", move |editor| {
                let turn = editor.wire_from_downstream(WireIdDomain::Turn, "synthetic-turn")?;
                let mut metadata = Map::new();
                project(editor, Some(&attribution), &mut metadata);
                let value: Value = serde_json::from_str(metadata[KEY].as_str().unwrap())?;
                assert_eq!(value["sources"][0]["first_turn_id"], turn);
                assert_eq!(value["sources"][0]["server_name"], "synthetic-server");
                Ok(value)
            })
            .await
            .unwrap();
        assert_ne!(emitted["sources"][0]["first_turn_id"], "synthetic-turn");
        let attribution = read(
            json!({"client_metadata":{KEY:value.to_string()}})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        store
            .edit(namespace, owner, "key-b", move |editor| {
                let mut metadata = Map::new();
                project(editor, Some(&attribution), &mut metadata);
                let value: Value = serde_json::from_str(metadata[KEY].as_str().unwrap())?;
                assert_eq!(
                    value,
                    json!({"status":"attribution_error","error_reason":"source_invalid"})
                );
                Ok(())
            })
            .await
            .unwrap();
        let _ = reload;
    }
}

#[test]
fn attribution_bounds_serialized_utf8_and_does_not_invent_provenance() {
    assert!(read(json!({}).as_object().unwrap()).is_none());
    let value = source("turn", "");
    let base = serde_json::to_string(&value).unwrap().len();
    for extra in [0, 1] {
        let value = source("turn", &"x".repeat(MAXIMUM - base + extra));
        let attribution = read(
            json!({"client_metadata":{KEY:value.to_string()}})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let value = serde_json::to_value(attribution).unwrap();
        assert_eq!(
            value["status"],
            if extra == 0 {
                "complete"
            } else {
                "attribution_error"
            }
        );
    }
    for bad in [
        json!(17),
        json!("invalid"),
        json!(source("turn", &"🦀".repeat(MAXIMUM / 4)).to_string()),
    ] {
        let value = read(json!({"client_metadata":{KEY:bad}}).as_object().unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(value).unwrap()["status"],
            "attribution_error"
        );
    }
    let raw = json!({"request_kind":"memory","mcp_attribution":"synthetic-private"}).to_string();
    let header = crate::request_identity::turn_metadata::bounded_turn_metadata(&raw).unwrap();
    assert!(!header.contains(KEY));
}
