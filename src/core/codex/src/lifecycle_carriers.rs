use crate::request_state_types::WireIdDomain;

pub(crate) const INSTALLATION_HEADER: &str = "x-codex-installation-id";
pub(crate) const TURN_METADATA_HEADER: &str = "x-codex-turn-metadata";
pub(crate) const WINDOW_HEADER: &str = "x-codex-window-id";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CarrierDirection {
    Request,
    Response,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CarrierUse {
    Evidence,
    Projection,
    Wire,
    TurnMetadata,
    HeaderPolicy,
    OpaqueBoundary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CarrierContainer {
    TopLevel,
    ResponseObject,
    Item,
    Caller,
    SafetyCheck,
    ClientMetadata,
    IdentityMetadata,
    ItemPassthroughMetadata,
    TurnMetadata,
    HeaderTurnMetadata,
    Header,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CarrierShape {
    Scalar,
    Conversation,
    TypedItemId,
    OwnedResponseId,
    TerminalResponseId,
    Window,
    SerializedTurnMetadata,
    ItemObject,
    ItemArray,
    ResponseObject,
    IdentityMetadataObject,
    CallerObject,
    SafetyCheckArray,
    Opaque,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CarrierAction {
    RelationshipProjection,
    ReversibleWireId,
    Opaque,
    PublicStrip,
    GatewayRequestAlias,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestWireMapping {
    Allocate,
    RequireExisting,
    RequireProviderExisting,
    Contextual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RelationshipCarrier {
    Installation,
    Session,
    ResponsesConversation,
    Thread,
    ParentThread,
    ForkedFromThread,
    Turn,
    RootTurn,
    ParentTurn,
    Window,
    RequestKind,
    Subagent,
    TurnStartedAt,
    ItemTurn,
    ClientRequest,
}

const ALLOW_EMPTY: u16 = 1 << 0;
const SKIP_AFTER_HEADER_PROJECTION: u16 = 1 << 1;
const HEADER_VISIBLE: u16 = 1 << 2;
const NORMAL_REQUIRED: u16 = 1 << 3;
const PREWARM_REQUIRED_STRING: u16 = 1 << 4;
const PREWARM_REQUIRED_BOOL: u16 = 1 << 5;
const REQUIRE_EXISTING_WIRE: u16 = 1 << 6;
const REQUIRE_PROVIDER_EXISTING_WIRE: u16 = 1 << 7;
const CONTEXTUAL_WIRE: u16 = 1 << 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CarrierRule {
    pub(crate) direction: CarrierDirection,
    pub(crate) use_case: CarrierUse,
    pub(crate) container: CarrierContainer,
    pub(crate) name: &'static str,
    pub(crate) shape: CarrierShape,
    pub(crate) domain: Option<WireIdDomain>,
    pub(crate) relationship: Option<RelationshipCarrier>,
    pub(crate) action: CarrierAction,
    pub(crate) priority: u8,
    flags: u16,
}

impl CarrierRule {
    pub(crate) const fn allow_empty(self) -> bool {
        self.flags & ALLOW_EMPTY != 0
    }

    pub(crate) const fn skip_after_header_projection(self) -> bool {
        self.flags & SKIP_AFTER_HEADER_PROJECTION != 0
    }

    pub(crate) const fn header_visible(self) -> bool {
        self.flags & HEADER_VISIBLE != 0
    }

    pub(crate) const fn normal_required(self) -> bool {
        self.flags & NORMAL_REQUIRED != 0
    }

    pub(crate) const fn prewarm_required_string(self) -> bool {
        self.flags & PREWARM_REQUIRED_STRING != 0
    }

    pub(crate) const fn prewarm_required_bool(self) -> bool {
        self.flags & PREWARM_REQUIRED_BOOL != 0
    }

    pub(crate) const fn request_wire_mapping(self) -> RequestWireMapping {
        if self.flags & CONTEXTUAL_WIRE != 0 {
            RequestWireMapping::Contextual
        } else if self.flags & REQUIRE_PROVIDER_EXISTING_WIRE != 0 {
            RequestWireMapping::RequireProviderExisting
        } else if self.flags & REQUIRE_EXISTING_WIRE != 0 {
            RequestWireMapping::RequireExisting
        } else {
            RequestWireMapping::Allocate
        }
    }
}

#[allow(clippy::too_many_arguments)]
const fn rule(
    direction: CarrierDirection,
    use_case: CarrierUse,
    container: CarrierContainer,
    name: &'static str,
    shape: CarrierShape,
    domain: Option<WireIdDomain>,
    relationship: Option<RelationshipCarrier>,
    action: CarrierAction,
    priority: u8,
    flags: u16,
) -> CarrierRule {
    CarrierRule {
        direction,
        use_case,
        container,
        name,
        shape,
        domain,
        relationship,
        action,
        priority,
        flags,
    }
}

const fn wire(
    direction: CarrierDirection,
    container: CarrierContainer,
    name: &'static str,
    shape: CarrierShape,
    domain: Option<WireIdDomain>,
) -> CarrierRule {
    rule(
        direction,
        CarrierUse::Wire,
        container,
        name,
        shape,
        domain,
        None,
        CarrierAction::ReversibleWireId,
        0,
        0,
    )
}

const fn wire_reference(
    container: CarrierContainer,
    name: &'static str,
    shape: CarrierShape,
    domain: Option<WireIdDomain>,
) -> CarrierRule {
    rule(
        CarrierDirection::Request,
        CarrierUse::Wire,
        container,
        name,
        shape,
        domain,
        None,
        CarrierAction::ReversibleWireId,
        0,
        REQUIRE_EXISTING_WIRE,
    )
}

const fn wire_provider_reference(
    container: CarrierContainer,
    name: &'static str,
    shape: CarrierShape,
    domain: Option<WireIdDomain>,
) -> CarrierRule {
    rule(
        CarrierDirection::Request,
        CarrierUse::Wire,
        container,
        name,
        shape,
        domain,
        None,
        CarrierAction::ReversibleWireId,
        0,
        REQUIRE_PROVIDER_EXISTING_WIRE,
    )
}

const fn wire_contextual(
    container: CarrierContainer,
    name: &'static str,
    shape: CarrierShape,
    domain: Option<WireIdDomain>,
) -> CarrierRule {
    rule(
        CarrierDirection::Request,
        CarrierUse::Wire,
        container,
        name,
        shape,
        domain,
        None,
        CarrierAction::ReversibleWireId,
        0,
        CONTEXTUAL_WIRE,
    )
}

const fn evidence(
    container: CarrierContainer,
    name: &'static str,
    shape: CarrierShape,
    relationship: RelationshipCarrier,
    priority: u8,
    flags: u16,
) -> CarrierRule {
    rule(
        CarrierDirection::Request,
        CarrierUse::Evidence,
        container,
        name,
        shape,
        None,
        Some(relationship),
        CarrierAction::RelationshipProjection,
        priority,
        flags,
    )
}

const fn projection(
    container: CarrierContainer,
    name: &'static str,
    shape: CarrierShape,
    relationship: Option<RelationshipCarrier>,
) -> CarrierRule {
    rule(
        CarrierDirection::Request,
        CarrierUse::Projection,
        container,
        name,
        shape,
        None,
        relationship,
        CarrierAction::RelationshipProjection,
        0,
        0,
    )
}

const fn metadata(
    name: &'static str,
    relationship: Option<RelationshipCarrier>,
    action: CarrierAction,
    flags: u16,
) -> CarrierRule {
    rule(
        CarrierDirection::Request,
        CarrierUse::TurnMetadata,
        CarrierContainer::TurnMetadata,
        name,
        CarrierShape::Scalar,
        None,
        relationship,
        action,
        0,
        flags,
    )
}

const fn header(
    name: &'static str,
    domain: Option<WireIdDomain>,
    action: CarrierAction,
) -> CarrierRule {
    rule(
        CarrierDirection::Response,
        CarrierUse::HeaderPolicy,
        CarrierContainer::Header,
        name,
        CarrierShape::Scalar,
        domain,
        None,
        action,
        0,
        0,
    )
}

const fn opaque(
    direction: CarrierDirection,
    container: CarrierContainer,
    name: &'static str,
) -> CarrierRule {
    rule(
        direction,
        CarrierUse::OpaqueBoundary,
        container,
        name,
        CarrierShape::Opaque,
        None,
        None,
        CarrierAction::Opaque,
        0,
        0,
    )
}

#[path = "lifecycle_carrier_evidence_rules.rs"]
mod evidence_rule_data;
#[path = "lifecycle_carrier_policy_rules.rs"]
mod policy_rule_data;
#[path = "lifecycle_carrier_projection_rules.rs"]
mod projection_rule_data;
#[path = "lifecycle_carrier_request_wire_rules.rs"]
mod request_wire_rule_data;
#[path = "lifecycle_carrier_response_wire_rules.rs"]
mod response_wire_rule_data;

pub(crate) fn all_rules() -> impl Iterator<Item = &'static CarrierRule> {
    evidence_rule_data::RULES
        .iter()
        .chain(projection_rule_data::RULES)
        .chain(request_wire_rule_data::RULES)
        .chain(response_wire_rule_data::RULES)
        .chain(policy_rule_data::RULES)
}

pub(crate) fn rules_for(
    direction: CarrierDirection,
    use_case: CarrierUse,
    container: CarrierContainer,
) -> impl Iterator<Item = &'static CarrierRule> {
    all_rules().filter(move |rule| {
        rule.direction == direction && rule.use_case == use_case && rule.container == container
    })
}

pub(crate) fn wire_rules(
    direction: CarrierDirection,
    container: CarrierContainer,
) -> impl Iterator<Item = &'static CarrierRule> {
    rules_for(direction, CarrierUse::Wire, container)
}

pub(crate) fn request_wire_mapping(
    rule: &CarrierRule,
    item_type: Option<&str>,
) -> RequestWireMapping {
    match rule.request_wire_mapping() {
        RequestWireMapping::Contextual => {
            request_wire_rule_data::contextual_mapping(rule.name, item_type)
        }
        mapping => mapping,
    }
}

pub(crate) fn evidence_rules(
    relationship: RelationshipCarrier,
) -> impl Iterator<Item = &'static CarrierRule> {
    all_rules().filter(move |rule| {
        rule.use_case == CarrierUse::Evidence && rule.relationship == Some(relationship)
    })
}

pub(crate) fn projection_rules(
    container: CarrierContainer,
) -> impl Iterator<Item = &'static CarrierRule> {
    rules_for(CarrierDirection::Request, CarrierUse::Projection, container)
}

pub(crate) fn turn_metadata_rules() -> impl Iterator<Item = &'static CarrierRule> {
    rules_for(
        CarrierDirection::Request,
        CarrierUse::TurnMetadata,
        CarrierContainer::TurnMetadata,
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn response_header_action(name: &str) -> CarrierAction {
    rules_for(
        CarrierDirection::Response,
        CarrierUse::HeaderPolicy,
        CarrierContainer::Header,
    )
    .find(|rule| rule.name.eq_ignore_ascii_case(name))
    .map_or(CarrierAction::PublicStrip, |rule| rule.action)
}

#[cfg(test)]
#[path = "lifecycle_carriers_tests.rs"]
mod tests;
