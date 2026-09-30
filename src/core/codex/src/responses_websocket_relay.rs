use super::*;

pub(crate) async fn relay(
    internal: WebSocket,
    upstream: UpstreamWebSocket,
    context: RelayContext,
    initial: Option<UpstreamMessage>,
) {
    let RelayContext {
        mut headers,
        account_ref,
        state_namespace,
        pseudonym_scope,
        profile,
        continuation,
        mut pending,
        vault,
        fingerprint,
        auth_binding,
        mut identity,
        operation,
    } = context;
    let (mut internal_write, mut internal_read) = internal.split();
    let (mut upstream_write, mut upstream_read) = upstream.split();
    let delivery = WebSocketDeliveryTracker::default();
    let continuation = Arc::new(StdMutex::new(continuation));
    if let (Some(namespace), Some(identity)) = (state_namespace.as_deref(), identity.as_ref()) {
        vault.request_state().contexts.track_baseline(
            crate::subscription_context::ContextStore::scope_key(namespace, &pseudonym_scope),
            identity,
            &continuation,
        );
    }
    let response_state = state_namespace.as_deref().and_then(|namespace| {
        profile.uses_identity_state().then(|| {
            ResponseStateContext::new(
                &account_ref,
                namespace,
                &pseudonym_scope,
                vault.request_state(),
                identity.as_ref(),
                None,
            )
            .with_gateway_request_id(&header_text(&headers, REQUEST_ID_HEADER).unwrap_or_default())
            .with_operation(operation)
        })
    });
    let exit = {
        let client_continuation = Arc::clone(&continuation);
        let client_response_state = response_state.clone();
        let client_to_upstream = async {
            if let Some(initial) = initial {
                if !relay_helpers::identity_is_current(
                    &vault,
                    &account_ref,
                    &fingerprint,
                    &auth_binding,
                )
                .await
                {
                    return RelayExit::StaleFingerprint;
                }
                if let Err(exit) = initial::send(
                    &mut internal_read,
                    &mut upstream_write,
                    initial,
                    &mut pending,
                    &client_continuation,
                    &delivery,
                )
                .await
                {
                    return exit;
                }
            }
            loop {
                let message = if let Some(message) = pending.pop_front() {
                    Ok(message)
                } else {
                    let Some(message) = internal_read.next().await else {
                        break;
                    };
                    message
                };
                let mut create_attempt = false;
                let outbound = match message {
                    Ok(InternalMessage::Text(text)) => {
                        let text = text.to_string();
                        let is_create = match is_response_create(&text) {
                            Ok(is_create) => is_create,
                            Err(()) => return RelayExit::Protocol,
                        };
                        let is_interrupt =
                            !is_create && crate::response_interrupt::is_control(&text);
                        if delivery.is_retired() {
                            return RelayExit::Failure(delivery.failure());
                        }
                        if !is_create && !is_interrupt && profile.emulates_codex() {
                            crate::ignored_fields::websocket_control(&headers);
                            continue;
                        }
                        if (is_create || is_interrupt)
                            && !relay_helpers::identity_is_current(
                                &vault,
                                &account_ref,
                                &fingerprint,
                                &auth_binding,
                            )
                            .await
                        {
                            return RelayExit::StaleFingerprint;
                        }
                        if is_create && public_create_in_flight(&client_continuation) {
                            return RelayExit::Policy;
                        }
                        vault.request_state().contexts.enforce_baseline_budget();
                        let prepared = prepare_client_text(
                            text,
                            &mut headers,
                            &account_ref,
                            state_namespace.as_deref(),
                            profile,
                            &pseudonym_scope,
                            &fingerprint,
                            vault.request_state(),
                            &mut identity,
                        )
                        .await;
                        match prepared {
                            Err(ClientPrepareError::StateUnavailable) => {
                                return RelayExit::StateUnavailable(
                                    delivery.failure_for_phase(FailurePhase::Internal),
                                );
                            }
                            Err(ClientPrepareError::Protocol) => return RelayExit::Protocol,
                            Ok(prepared) => {
                                if is_interrupt
                                    && !relay_helpers::prepare_interrupt(
                                        &client_continuation,
                                        &prepared.text,
                                    )
                                    && profile.emulates_codex()
                                {
                                    return RelayExit::Protocol;
                                }
                                if is_create
                                    && let Some(state) = client_response_state.as_ref()
                                    && state.update_operation(prepared.operation.clone()).is_err()
                                {
                                    return RelayExit::StateUnavailable(
                                        delivery.failure_for_phase(FailurePhase::Internal),
                                    );
                                }
                                let planned = if let Some(value) = prepared.create_value.as_ref() {
                                    let mut continuation = continuation_guard(&client_continuation);
                                    continuation.mark_rebuilt_reference(prepared.rebuilt_reference);
                                    if profile == UpstreamProfile::ApiKeyPassthrough {
                                        continuation.plan_public_create(value);
                                        Ok((prepared.text, None))
                                    } else {
                                        crate::responses_websocket_emulation::plan_public_text_with_observations(
                                            &mut continuation,
                                            value,
                                            &prepared.synthesized_item_ids,
                                            prepared.pending_compaction.clone(),
                                            crate::inference_limits::get().request_bytes,
                                        )
                                    }
                                } else {
                                    Ok((prepared.text, None))
                                };
                                match planned {
                                    Ok((mut prepared, frame)) => {
                                        if let Some(frame) = frame {
                                            let (Some(namespace), Some(identity)) =
                                                (state_namespace.as_deref(), identity.as_ref())
                                            else {
                                                return RelayExit::Protocol;
                                            };
                                            prepared = match crate::request_state_editor::tool_observations::finalize_frame(
                                                vault.request_state(), namespace, &account_ref, &pseudonym_scope,
                                                &identity.thread_id, frame, crate::inference_limits::get().request_bytes,
                                            ).await {
                                                Ok(text) => text,
                                                Err(_) => return RelayExit::StateUnavailable(delivery.failure_for_phase(FailurePhase::Internal)),
                                            };
                                        }
                                        if is_create
                                            && let Some(state) = client_response_state.as_ref()
                                            && state.update_identity(identity.as_ref()).is_err()
                                        {
                                            return RelayExit::StateUnavailable(
                                                delivery.failure_for_phase(FailurePhase::Internal),
                                            );
                                        }
                                        create_attempt = is_create;
                                        UpstreamMessage::Text(prepared.into())
                                    }
                                    Err(()) => upstream_close(UpstreamCloseCode::Protocol),
                                }
                            }
                        }
                    }
                    Ok(InternalMessage::Binary(_)) => {
                        upstream_close(UpstreamCloseCode::Unsupported)
                    }
                    Ok(InternalMessage::Ping(payload)) => UpstreamMessage::Ping(payload),
                    Ok(InternalMessage::Pong(payload)) => UpstreamMessage::Pong(payload),
                    Ok(InternalMessage::Close(frame)) => {
                        let (code, reason) = frame
                            .map(|frame| (allowed_close_code(frame.code), frame.reason.to_string()))
                            .unwrap_or((UpstreamCloseCode::Normal, String::new()));
                        UpstreamMessage::Close(Some(UpstreamCloseFrame {
                            code,
                            reason: reason.into(),
                        }))
                    }
                    Err(_) => upstream_close(UpstreamCloseCode::Away),
                };
                let terminal = matches!(outbound, UpstreamMessage::Close(_));
                if create_attempt {
                    if delivery.is_retired() {
                        return RelayExit::Failure(delivery.failure());
                    }
                    if !continuation_guard(&client_continuation).mark_public_create_attempted() {
                        return RelayExit::Policy;
                    }
                    delivery.mark_attempted();
                }
                if upstream_write.send(outbound).await.is_err() {
                    if create_attempt {
                        continuation_guard(&client_continuation).fail_public_create();
                    }
                    return if terminal {
                        RelayExit::Complete
                    } else {
                        RelayExit::Failure(delivery.failure())
                    };
                }
                if terminal {
                    return RelayExit::Complete;
                }
            }
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::Away))
                .await;
            RelayExit::Complete
        };
        let server_continuation = Arc::clone(&continuation);
        let upstream_to_client = async {
            let mut inbound = inbound::Inbound::default();
            while let Some(message) = inbound.next(&mut upstream_read).await {
                let (outbound, terminal_event) = match message {
                    Ok(UpstreamMessage::Text(text)) => {
                        vault.request_state().contexts.enforce_baseline_budget();
                        let (text, terminal) = match inbound
                            .translate(
                                text.to_string(),
                                response_state.as_ref(),
                                &server_continuation,
                                &delivery,
                            )
                            .await
                        {
                            Ok(Some(event)) => event,
                            Ok(None) => continue,
                            Err(_) => return RelayExit::Failure(delivery.failure()),
                        };
                        (InternalMessage::Text(text.into()), terminal)
                    }
                    Ok(UpstreamMessage::Binary(_)) | Ok(UpstreamMessage::Frame(_)) => {
                        return RelayExit::Failure(delivery.failure());
                    }
                    Ok(UpstreamMessage::Ping(payload)) => (InternalMessage::Ping(payload), None),
                    Ok(UpstreamMessage::Pong(payload)) => (InternalMessage::Pong(payload), None),
                    Ok(UpstreamMessage::Close(frame)) => {
                        let failure = delivery.failure();
                        if failure.delivery_state
                            != mini_sub2api_protocol_v1::DeliveryState::NotDelivered
                        {
                            return RelayExit::Failure(failure);
                        }
                        (
                            InternalMessage::Close(frame.map(|frame| InternalCloseFrame {
                                code: u16::from(frame.code),
                                reason: frame.reason.to_string().into(),
                            })),
                            None,
                        )
                    }
                    Err(_) => return RelayExit::Failure(delivery.failure()),
                };
                let terminal = matches!(outbound, InternalMessage::Close(_));
                if internal_write.send(outbound).await.is_err() {
                    return RelayExit::Complete;
                }
                if let Some(generation) = terminal_event {
                    delivery.mark_terminal(generation);
                }
                if terminal {
                    return RelayExit::Complete;
                }
            }
            RelayExit::Failure(delivery.failure())
        };
        tokio::select! {
            biased;
            exit = client_to_upstream => exit,
            exit = upstream_to_client => exit,
        }
    };
    match exit {
        RelayExit::Complete => continuation_guard(&continuation).reset(),
        RelayExit::StaleFingerprint => {
            continuation_guard(&continuation).reset();
            let _ = internal_write.send(internal_close(1012)).await;
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::Restart))
                .await;
        }
        RelayExit::Failure(metadata) => {
            continuation_guard(&continuation).reset();
            let _ = internal_write.send(failure_close(metadata)).await;
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::Restart))
                .await;
        }
        RelayExit::StateUnavailable(metadata) => {
            continuation_guard(&continuation).reset();
            let _ = internal_write.send(failure_close(metadata)).await;
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::Restart))
                .await;
        }
        RelayExit::Policy | RelayExit::Protocol => {
            let code = if matches!(exit, RelayExit::Protocol) {
                1002
            } else {
                1008
            };
            continuation_guard(&continuation).reset();
            let _ = internal_write.send(internal_close(code)).await;
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::from(code)))
                .await;
        }
        RelayExit::TooLarge => {
            continuation_guard(&continuation).reset();
            let _ = internal_write.send(internal_close(1009)).await;
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::Size))
                .await;
        }
    }
}
