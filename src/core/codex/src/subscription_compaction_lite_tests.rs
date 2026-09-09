use super::*;

#[tokio::test]
async fn in_band_lite_compaction_retains_the_actual_optional_setup_block() {
    for developers in 0..=2 {
        let (_temp, store) = store();
        let mut source = vec![
            json!({"type":"additional_tools","role":"developer","tools":[
                {"type":"function","name":"probe","parameters":{"type":"object","properties":{}}}]
            }),
        ];
        for _ in 0..developers {
            source.push(message("developer", "caller prefix kept exactly"));
        }
        source.push(input("covered user"));
        let mut first = request(json!(source));
        first.as_object_mut().unwrap().remove("instructions");
        let first = prepare(&store, first).await.unwrap();
        let response = finish(
            &store,
            first,
            vec![
                compacted("inline Lite checkpoint"),
                message("assistant", "retained answer"),
            ],
            true,
        )
        .await;
        let cached = history(&store, &response).expect("Lite checkpoint preserves setup");
        assert_eq!(cached.len, developers + 3);
        let values = cached.values();
        assert_eq!(values[0]["type"], "additional_tools");
        for item in &values[1..=developers] {
            assert_caller_item_with_generated_metadata(
                item,
                &message("developer", "caller prefix kept exactly"),
            );
        }
        let mut next = delta(&response, vec![input("new user")]);
        next.as_object_mut().unwrap().remove("instructions");
        let prepared = prepare(&store, next).await.unwrap();
        let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert!(wire.get("instructions").is_none() && wire.get("tools").is_none());
        let input = wire["input"].as_array().unwrap();
        assert_eq!(input[0]["type"], "additional_tools");
        assert_eq!(input[0]["tools"][0]["name"], "probe");
        assert_eq!(
            input
                .iter()
                .filter(|item| item["role"] == "developer" && item["type"] == "message")
                .count(),
            developers
        );
        assert_eq!(
            input
                .iter()
                .filter(|item| item["type"] == "compaction")
                .count(),
            1
        );
    }
}
