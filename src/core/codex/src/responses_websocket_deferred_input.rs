use crate::websocket_delivery::{internal_close, is_response_create};
use axum::extract::ws::{Message, WebSocket};
use futures_util::StreamExt;
use std::{collections::VecDeque, future::Future};

const MAX_DEFERRED_PENDING_MESSAGES: usize = 1024;
const DEFERRED_PENDING_MESSAGE_OVERHEAD: usize = 64;

pub(super) async fn wait_deferred<T>(
    internal: &mut WebSocket,
    pending: &mut VecDeque<Message>,
    pending_cost: &mut usize,
    future: impl Future<Output = T>,
) -> Option<T> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            output = &mut future => return Some(output),
            message = internal.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        match is_response_create(&text) {
                            Ok(true) => {
                                let _ = internal.send(internal_close(1008)).await;
                                return None;
                            }
                            Ok(false) => {
                                let Some(message_cost) = text.len().checked_add(DEFERRED_PENDING_MESSAGE_OVERHEAD) else {
                                    let _ = internal.send(internal_close(1009)).await;
                                    return None;
                                };
                                let Some(next) = pending_cost.checked_add(message_cost) else {
                                    let _ = internal.send(internal_close(1009)).await;
                                    return None;
                                };
                                if pending.len() >= MAX_DEFERRED_PENDING_MESSAGES
                                    || next > crate::inference_limits::get().request_bytes
                                {
                                    let _ = internal.send(internal_close(1009)).await;
                                    return None;
                                }
                                *pending_cost = next;
                                pending.push_back(Message::Text(text));
                            }
                            Err(()) => {
                                let _ = internal.send(internal_close(1002)).await;
                                return None;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if internal.send(Message::Pong(payload)).await.is_err() {
                            return None;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Binary(_))) => {
                        let _ = internal.send(internal_close(1003)).await;
                        return None;
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
                }
            }
        }
    }
}

pub(super) async fn first_create(internal: &mut WebSocket) -> Result<String, u16> {
    while let Some(message) = internal.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let text = text.to_string();
                return match is_response_create(&text) {
                    Ok(true) => Ok(text),
                    _ => Err(1002),
                };
            }
            Ok(Message::Ping(payload)) => {
                internal
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|_| 1011_u16)?;
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Binary(_)) => return Err(1003),
            Ok(Message::Close(_)) | Err(_) => return Err(1001),
        }
    }
    Err(1001)
}
