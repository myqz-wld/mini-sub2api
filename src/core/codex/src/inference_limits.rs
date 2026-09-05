use mini_sub2api_protocol_v1::limits::InferenceLimits;
use std::sync::LazyLock;
static LIMITS: LazyLock<InferenceLimits> =
    LazyLock::new(|| InferenceLimits::load().expect("valid operator inference limits"));
pub(crate) fn get() -> &'static InferenceLimits {
    &LIMITS
}
