use super::*;

#[tokio::test]
async fn long_http_routing_token_survives_tool_return_and_resets_on_new_turn() {
    let token = "synthetic-routing-token-".repeat(40);
    let captures = Arc::new(Mutex::new(Vec::<(HeaderMap, Value)>::new()));
    let upstream = spawn_loopback(Router::new().route("/responses", post({
        let captures = captures.clone();
        let token = token.clone();
        move |headers: HeaderMap, body: Bytes| {
            let captures = captures.clone();
            let token = token.clone();
            async move {
                let value: Value = serde_json::from_slice(&zstd::stream::decode_all(body.as_ref()).unwrap()).unwrap();
                let mut captures = captures.lock().await;
                captures.push((headers, value));
                let n = captures.len();
                let output = if n == 1 {
                    json!({"type":"function_call","id":"fc_probe","call_id":"call_probe","name":"probe","arguments":"{}"})
                } else {
                    json!({"type":"message","id":format!("msg_{n}"),"role":"assistant","content":[{"type":"output_text","text":"synthetic answer"}]})
                };
                let response = json!({"id":format!("resp_routing_{n}"),"output":[output]});
                let events = [json!({"type":"response.created","response":{"id":response["id"]}}),
                    json!({"type":"response.output_item.done","output_index":0,"item":output}),
                    json!({"type":"response.completed","response":response})];
                let sse = events.into_iter().map(|event| format!("data: {event}\n\n")).collect::<String>();
                Response::builder().header("content-type","text/event-stream").header("x-codex-turn-state",if n == 1 {token} else {"later-token".into()}).body(Body::from(sse)).unwrap()
            }
        }
    }))).await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    let first = request(
        &state,
        &account,
        json!({"model":"gpt-5.5","input":[user("call the synthetic probe")],"stream":false}),
        HeaderMap::new(),
    )
    .await;
    let second = request(&state, &account, json!({"model":"gpt-5.5","previous_response_id":first["id"],"input":[{"type":"function_call_output","call_id":first["output"][0]["call_id"],"output":"synthetic result"}],"stream":false}), HeaderMap::new()).await;
    request(&state, &account, json!({"model":"gpt-5.5","previous_response_id":second["id"],"input":[user("new user turn")],"stream":false}), HeaderMap::new()).await;
    let captured = captures.lock().await;
    assert_eq!(captured.len(), 3);
    assert!(captured[0].0.get("x-codex-turn-state").is_none());
    assert_eq!(captured[1].0["x-codex-turn-state"], token);
    assert!(captured[2].0.get("x-codex-turn-state").is_none());
    for (_, body) in captured.iter() {
        assert!(body["client_metadata"].get("x-codex-turn-state").is_none());
    }
}
