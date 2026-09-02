use super::*;

pub(super) const RULES: &[CarrierRule] = &[
    // Public response-header policy. Unknown names default to PublicStrip.
    header("content-type", None, CarrierAction::Opaque),
    header("content-encoding", None, CarrierAction::Opaque),
    header("cache-control", None, CarrierAction::Opaque),
    header("retry-after", None, CarrierAction::Opaque),
    header("retry-after-ms", None, CarrierAction::Opaque),
    header("server-timing", None, CarrierAction::Opaque),
    header("openai-model", None, CarrierAction::Opaque),
    header("openai-processing-ms", None, CarrierAction::Opaque),
    header("openai-version", None, CarrierAction::Opaque),
    header("x-models-etag", None, CarrierAction::Opaque),
    header("x-reasoning-included", None, CarrierAction::Opaque),
    header("x-codex-turn-state", None, CarrierAction::Opaque),
    header("x-ratelimit-limit-requests", None, CarrierAction::Opaque),
    header(
        "x-ratelimit-remaining-requests",
        None,
        CarrierAction::Opaque,
    ),
    header("x-ratelimit-reset-requests", None, CarrierAction::Opaque),
    header("x-ratelimit-limit-tokens", None, CarrierAction::Opaque),
    header("x-ratelimit-remaining-tokens", None, CarrierAction::Opaque),
    header("x-ratelimit-reset-tokens", None, CarrierAction::Opaque),
    header("x-request-id", None, CarrierAction::GatewayRequestAlias),
    header(
        "openai-request-id",
        None,
        CarrierAction::GatewayRequestAlias,
    ),
    header("request-id", None, CarrierAction::GatewayRequestAlias),
    header(
        INSTALLATION_HEADER,
        Some(WireIdDomain::Installation),
        CarrierAction::PublicStrip,
    ),
    header(TURN_METADATA_HEADER, None, CarrierAction::PublicStrip),
    header(
        WINDOW_HEADER,
        Some(WireIdDomain::Thread),
        CarrierAction::PublicStrip,
    ),
    header(
        "session-id",
        Some(WireIdDomain::Session),
        CarrierAction::PublicStrip,
    ),
    header(
        "thread-id",
        Some(WireIdDomain::Thread),
        CarrierAction::PublicStrip,
    ),
    header(
        "x-client-request-id",
        Some(WireIdDomain::Thread),
        CarrierAction::PublicStrip,
    ),
    header(
        "x-codex-parent-thread-id",
        Some(WireIdDomain::Thread),
        CarrierAction::PublicStrip,
    ),
    header(
        "session_id",
        Some(WireIdDomain::Session),
        CarrierAction::PublicStrip,
    ),
    header(
        "conversation_id",
        Some(WireIdDomain::Session),
        CarrierAction::PublicStrip,
    ),
    header("x-openai-subagent", None, CarrierAction::PublicStrip),
    // Explicit non-traversal boundaries; all unlisted fields are opaque as well.
    opaque(
        CarrierDirection::Request,
        CarrierContainer::TopLevel,
        "tools",
    ),
    opaque(
        CarrierDirection::Request,
        CarrierContainer::TopLevel,
        "metadata",
    ),
    opaque(
        CarrierDirection::Request,
        CarrierContainer::TopLevel,
        "text",
    ),
    opaque(CarrierDirection::Request, CarrierContainer::Item, "content"),
    opaque(
        CarrierDirection::Request,
        CarrierContainer::Item,
        "arguments",
    ),
    opaque(CarrierDirection::Request, CarrierContainer::Item, "output"),
    opaque(
        CarrierDirection::Request,
        CarrierContainer::Item,
        "encrypted_content",
    ),
    opaque(
        CarrierDirection::Response,
        CarrierContainer::TopLevel,
        "metadata",
    ),
    opaque(
        CarrierDirection::Response,
        CarrierContainer::Item,
        "content",
    ),
    opaque(
        CarrierDirection::Response,
        CarrierContainer::Item,
        "arguments",
    ),
    opaque(CarrierDirection::Response, CarrierContainer::Item, "output"),
    opaque(
        CarrierDirection::Response,
        CarrierContainer::Item,
        "encrypted_content",
    ),
];
