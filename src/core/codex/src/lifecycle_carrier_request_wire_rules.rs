use super::*;

pub(super) const RULES: &[CarrierRule] = &[
    // Request reversible wire carriers and their schema-owned traversal edges.
    wire_provider_reference(
        CarrierContainer::TopLevel,
        "previous_response_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Response),
    ),
    wire_provider_reference(
        CarrierContainer::TopLevel,
        "response_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Response),
    ),
    wire(
        CarrierDirection::Request,
        CarrierContainer::TopLevel,
        "stream_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Stream),
    ),
    wire_reference(
        CarrierContainer::TopLevel,
        "item_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Item),
    ),
    wire_reference(
        CarrierContainer::TopLevel,
        "output_item_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Item),
    ),
    wire_reference(
        CarrierContainer::TopLevel,
        "call_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Call),
    ),
    wire_reference(
        CarrierContainer::TopLevel,
        "approval_request_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Approval),
    ),
    wire_reference(
        CarrierContainer::TopLevel,
        "approval_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Approval),
    ),
    wire_provider_reference(
        CarrierContainer::TopLevel,
        "conversation",
        CarrierShape::Conversation,
        Some(WireIdDomain::Conversation),
    ),
    wire(
        CarrierDirection::Request,
        CarrierContainer::TopLevel,
        "item",
        CarrierShape::ItemObject,
        None,
    ),
    wire(
        CarrierDirection::Request,
        CarrierContainer::TopLevel,
        "items",
        CarrierShape::ItemArray,
        None,
    ),
    wire(
        CarrierDirection::Request,
        CarrierContainer::TopLevel,
        "input",
        CarrierShape::ItemArray,
        None,
    ),
    wire_contextual(
        CarrierContainer::Item,
        "id",
        CarrierShape::TypedItemId,
        Some(WireIdDomain::Item),
    ),
    wire_reference(
        CarrierContainer::Item,
        "item_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Item),
    ),
    wire_reference(
        CarrierContainer::Item,
        "output_item_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Item),
    ),
    wire_contextual(
        CarrierContainer::Item,
        "call_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Call),
    ),
    wire_reference(
        CarrierContainer::Item,
        "approval_request_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Approval),
    ),
    wire_provider_reference(
        CarrierContainer::Item,
        "response_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Response),
    ),
    wire(
        CarrierDirection::Request,
        CarrierContainer::Item,
        "caller",
        CarrierShape::CallerObject,
        None,
    ),
    wire(
        CarrierDirection::Request,
        CarrierContainer::Item,
        "pending_safety_checks",
        CarrierShape::SafetyCheckArray,
        None,
    ),
    wire(
        CarrierDirection::Request,
        CarrierContainer::Item,
        "acknowledged_safety_checks",
        CarrierShape::SafetyCheckArray,
        None,
    ),
    wire_reference(
        CarrierContainer::Caller,
        "caller_id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Item),
    ),
    wire_reference(
        CarrierContainer::SafetyCheck,
        "id",
        CarrierShape::Scalar,
        Some(WireIdDomain::Approval),
    ),
];

pub(super) fn contextual_mapping(name: &str, item_type: Option<&str>) -> RequestWireMapping {
    match name {
        "id" if item_type == Some("item_reference") => RequestWireMapping::RequireExisting,
        "id" => RequestWireMapping::Allocate,
        "call_id"
            if matches!(
                item_type,
                Some(
                    "function_call"
                        | "custom_tool_call"
                        | "computer_call"
                        | "local_shell_call"
                        | "shell_call"
                        | "apply_patch_call"
                        | "tool_search_call"
                        | "program"
                )
            ) =>
        {
            RequestWireMapping::Allocate
        }
        "call_id" => RequestWireMapping::RequireExisting,
        _ => RequestWireMapping::RequireExisting,
    }
}
