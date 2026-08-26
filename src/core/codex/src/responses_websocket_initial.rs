use super::*;
use futures_util::future::poll_fn;
use futures_util::{Sink, SinkExt, Stream};
use std::pin::Pin;
use std::task::Poll;

const MAX_PENDING_MESSAGES: usize = 1024;
const PENDING_MESSAGE_OVERHEAD: usize = 64;

enum Gate {
    Internal(InternalMessage),
    InternalClosed,
    UpstreamReady,
    UpstreamFailed,
}

pub(super) async fn send<InternalRead, InternalError, UpstreamWrite>(
    internal_read: &mut InternalRead,
    upstream_write: &mut UpstreamWrite,
    initial: UpstreamMessage,
    pending: &mut VecDeque<InternalMessage>,
    continuation: &StdMutex<ResponsesWebSocketState>,
    delivery: &WebSocketDeliveryTracker,
) -> Result<(), RelayExit>
where
    InternalRead: Stream<Item = Result<InternalMessage, InternalError>> + Unpin,
    UpstreamWrite: Sink<UpstreamMessage> + Unpin,
{
    let mut pending_cost = pending.iter().try_fold(0_usize, |total, message| {
        total.checked_add(message_cost(message)?)
    });
    loop {
        let gate = poll_fn(|context| {
            match Pin::new(&mut *internal_read).poll_next(context) {
                Poll::Ready(Some(Ok(message))) => return Poll::Ready(Gate::Internal(message)),
                Poll::Ready(Some(Err(_)) | None) => return Poll::Ready(Gate::InternalClosed),
                Poll::Pending => {}
            }
            match Pin::new(&mut *upstream_write).poll_ready(context) {
                Poll::Ready(Ok(())) => Poll::Ready(Gate::UpstreamReady),
                Poll::Ready(Err(_)) => Poll::Ready(Gate::UpstreamFailed),
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
        match gate {
            Gate::Internal(InternalMessage::Close(_)) | Gate::InternalClosed => {
                return Err(RelayExit::Complete);
            }
            Gate::Internal(message @ InternalMessage::Text(_)) => {
                let InternalMessage::Text(text) = &message else {
                    unreachable!();
                };
                if is_response_create(text).unwrap_or(false) {
                    return Err(RelayExit::Policy);
                }
                let Some(next) =
                    pending_cost.and_then(|cost| cost.checked_add(message_cost(&message)?))
                else {
                    return Err(RelayExit::TooLarge);
                };
                if pending.len() >= MAX_PENDING_MESSAGES || next > MAX_WEBSOCKET_MESSAGE_BYTES {
                    return Err(RelayExit::TooLarge);
                }
                pending_cost = Some(next);
                pending.push_back(message);
            }
            Gate::Internal(message) => {
                let Some(next) =
                    pending_cost.and_then(|cost| cost.checked_add(message_cost(&message)?))
                else {
                    return Err(RelayExit::TooLarge);
                };
                if pending.len() >= MAX_PENDING_MESSAGES || next > MAX_WEBSOCKET_MESSAGE_BYTES {
                    return Err(RelayExit::TooLarge);
                }
                pending_cost = Some(next);
                pending.push_back(message);
            }
            Gate::UpstreamFailed => return Err(RelayExit::Failure(delivery.failure())),
            Gate::UpstreamReady => break,
        }
    }
    if !continuation_guard(continuation).mark_public_create_attempted() {
        return Err(RelayExit::Policy);
    }
    delivery.mark_attempted();
    if upstream_write.start_send_unpin(initial).is_err() || upstream_write.flush().await.is_err() {
        continuation_guard(continuation).fail_public_create();
        return Err(RelayExit::Failure(delivery.failure()));
    }
    Ok(())
}

fn message_cost(message: &InternalMessage) -> Option<usize> {
    let payload = match message {
        InternalMessage::Text(text) => text.len(),
        InternalMessage::Binary(bytes)
        | InternalMessage::Ping(bytes)
        | InternalMessage::Pong(bytes) => bytes.len(),
        InternalMessage::Close(_) => 0,
    };
    payload.checked_add(PENDING_MESSAGE_OVERHEAD)
}
