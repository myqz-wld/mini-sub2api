//! Fixed native categories with bounded usage-window metadata; no provider text.
#[derive(Clone, Copy, Debug)]
pub(crate) enum NativeErrorCategory {
    ServerOverloaded,
    SlowDown,
    MisalignmentPolicy,
    CyberPolicy,
    BioPolicy,
    UsageLimitReached(Option<u16>),
    UsageNotIncluded,
    QuotaExceeded,
}

impl NativeErrorCategory {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::ServerOverloaded => "server_is_overloaded",
            Self::SlowDown => "slow_down",
            Self::MisalignmentPolicy => "misalignment_policy_violation",
            Self::CyberPolicy => "cyber_policy",
            Self::BioPolicy => "bio_policy",
            Self::UsageLimitReached(_) => "usage_limit_reached",
            Self::UsageNotIncluded => "usage_not_included",
            Self::QuotaExceeded => "insufficient_quota",
        }
    }

    pub(crate) fn error_type(self) -> Option<&'static str> {
        match self {
            Self::UsageLimitReached(_) | Self::UsageNotIncluded | Self::QuotaExceeded => {
                Some(self.code())
            }
            _ => None,
        }
    }

    pub(crate) fn limit_window_minutes(self) -> Option<u16> {
        match self {
            Self::UsageLimitReached(minutes) => minutes,
            _ => None,
        }
    }

    pub(crate) fn public_message(self) -> &'static str {
        match self {
            Self::ServerOverloaded => "The upstream service is overloaded.",
            Self::SlowDown => "The upstream service requested a slower request rate.",
            Self::MisalignmentPolicy | Self::CyberPolicy | Self::BioPolicy => {
                "The upstream service rejected the request under its policy."
            }
            Self::UsageLimitReached(_) => "The upstream usage limit has been reached.",
            Self::UsageNotIncluded => "The selected credential does not include this usage.",
            Self::QuotaExceeded => "The upstream quota has been exhausted.",
        }
    }
}
