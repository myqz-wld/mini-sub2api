use super::*;

#[tokio::test]
async fn control_mutation_cannot_republish_history_from_before_the_control() {
    for kind in [
        "response.inject",
        "response.append_input_item",
        "future.control",
    ] {
        for explicit in [false, true] {
            let (_temp, store) = store();
            let socket = store.contexts.open_socket().unwrap();
            let first = websocket_request(
                &store,
                request(json!([input("original")])),
                &socket.id,
                None,
            )
            .await
            .unwrap();
            let response = response_context(&store, &first);
            let created = response
                .translate_value(
                    json!({"type":"response.created","response":{"id":"resp_control"}}),
                )
                .await
                .unwrap();
            let mut control = json!({"type":kind});
            if kind == "response.inject" {
                control["input"] = json!([input("interleaved")]);
            } else {
                control["item"] = input("interleaved");
            }
            if explicit {
                control["response_id"] = created["response"]["id"].clone();
            }
            store
                .contexts
                .prepare_control(
                    &ContextStore::scope_key(NAMESPACE, KEY),
                    first.resolved_identity.as_ref(),
                    &control,
                )
                .unwrap();
            let completed = response.translate_value(json!({"type":"response.completed","response":{"id":"resp_control","output":[]}})).await.unwrap();
            let mut next = request(json!([input("suffix")]));
            next["previous_response_id"] = completed["response"]["id"].clone();
            assert!(
                matches!(
                    prepare(&store, next).await,
                    Err(StatefulPrepareError::StateUnavailable)
                ),
                "control cannot reconstruct stale history: {kind}/{explicit}"
            );
        }
    }
}

#[tokio::test]
async fn body_control_requires_its_active_owner_but_transport_controls_stay_valid() {
    let (_temp, store) = store();
    let scope = ContextStore::scope_key(NAMESPACE, KEY);
    assert!(
        store
            .contexts
            .prepare_control(
                &scope,
                None,
                &json!({"type":"response.append_input_item","item":input("orphan")})
            )
            .is_err()
    );
    assert!(
        store
            .contexts
            .prepare_control(&scope, None, &json!({"type":"future.ping"}))
            .is_ok()
    );
    let socket = store.contexts.open_socket().unwrap();
    let first = websocket_request(
        &store,
        request(json!([input("original")])),
        &socket.id,
        None,
    )
    .await
    .unwrap();
    let response = response_context(&store, &first);
    let created = response
        .translate_value(json!({"type":"response.created","response":{"id":"resp_owner"}}))
        .await
        .unwrap();
    let mut wrong = first.resolved_identity.as_ref().unwrap().clone();
    wrong.thread_id = "other-thread".into();
    let control = json!({"type":"future.control","response_id":created["response"]["id"],"item":input("wrong owner")});
    assert!(
        store
            .contexts
            .prepare_control(&scope, Some(&wrong), &control)
            .is_err()
    );
}
