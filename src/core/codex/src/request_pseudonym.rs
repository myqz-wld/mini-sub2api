use crate::ascii_json::to_ascii_json_string;
use hmac::Hmac;
use hmac::Mac;
use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use serde_json::Map;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

const DERIVATION_DOMAIN: &[u8] = b"mini-sub2api/request-pseudonym/v1";
const INSTALLATION_HEADER: &str = "x-codex-installation-id";
const TURN_METADATA_HEADER: &str = "x-codex-turn-metadata";
const WINDOW_HEADER: &str = "x-codex-window-id";

#[derive(Clone, Copy)]
enum IdentityDomain {
    Installation,
    Session,
    Thread,
    Turn,
    Window,
    ClientRequest,
    PromptCache,
}

impl IdentityDomain {
    fn label(self) -> &'static [u8] {
        match self {
            Self::Installation => b"installation",
            Self::Session => b"session",
            Self::Thread => b"thread",
            Self::Turn => b"turn",
            Self::Window => b"window",
            Self::ClientRequest => b"client-request",
            Self::PromptCache => b"prompt-cache",
        }
    }
}

/// Deterministically isolates caller identifiers by credential and downstream key.
///
/// The stable upstream account namespace derives the HMAC key; a stateless scope derived from the
/// downstream key verifier and the field domain are included in every message. The first 128
/// digest bits are encoded as an RFC 9562 UUIDv8 so projected values keep the UUID-shaped wire
/// contract without copying sub2api's namespace-UUID construction or relying on machine state.
pub(crate) struct RequestPseudonymizer {
    key: [u8; 32],
    downstream_scope: String,
}

impl RequestPseudonymizer {
    pub(crate) fn new(account_namespace: &str, downstream_scope: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(DERIVATION_DOMAIN);
        digest.update([0]);
        digest.update(account_namespace.as_bytes());
        Self {
            key: digest.finalize().into(),
            downstream_scope: downstream_scope.to_string(),
        }
    }

    pub(crate) fn converged_installation_id(account_namespace: &str) -> String {
        let pseudonymizer = Self::new(account_namespace, "account-device");
        pseudonymizer.id(IdentityDomain::Installation, "account-device")
    }

    pub(crate) fn apply(
        &self,
        headers: &mut HeaderMap,
        object: &mut Map<String, Value>,
    ) -> Result<(), ()> {
        self.apply_headers(headers)?;
        self.apply_body(object)
    }

    fn apply_headers(&self, headers: &mut HeaderMap) -> Result<(), ()> {
        for (name, domain) in [
            (INSTALLATION_HEADER, IdentityDomain::Installation),
            ("session-id", IdentityDomain::Session),
            ("session_id", IdentityDomain::Session),
            ("conversation_id", IdentityDomain::Session),
            ("thread-id", IdentityDomain::Thread),
            ("x-codex-parent-thread-id", IdentityDomain::Thread),
            ("x-client-request-id", IdentityDomain::ClientRequest),
        ] {
            self.rewrite_header(headers, name, domain)?;
        }
        self.rewrite_window_header(headers, WINDOW_HEADER)?;
        self.rewrite_serialized_header(headers, TURN_METADATA_HEADER)
    }

    fn apply_body(&self, object: &mut Map<String, Value>) -> Result<(), ()> {
        if let Some(cache_key) = object.get_mut("prompt_cache_key") {
            self.rewrite_value(cache_key, IdentityDomain::PromptCache);
        }
        if let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) {
            for item in items {
                let Some(item_metadata) = item
                    .as_object_mut()
                    .and_then(|item| item.get_mut("internal_chat_message_metadata_passthrough"))
                    .and_then(Value::as_object_mut)
                else {
                    continue;
                };
                self.rewrite_metadata(item_metadata)?;
            }
        }
        let Some(metadata) = object
            .get_mut("client_metadata")
            .and_then(Value::as_object_mut)
        else {
            return Ok(());
        };
        self.rewrite_metadata(metadata)
    }

    fn rewrite_metadata(&self, metadata: &mut Map<String, Value>) -> Result<(), ()> {
        for (name, domain) in [
            (INSTALLATION_HEADER, IdentityDomain::Installation),
            ("installation_id", IdentityDomain::Installation),
            ("session_id", IdentityDomain::Session),
            ("conversation_id", IdentityDomain::Session),
            ("thread_id", IdentityDomain::Thread),
            ("forked_from_thread_id", IdentityDomain::Thread),
            ("parent_thread_id", IdentityDomain::Thread),
            ("x-codex-parent-thread-id", IdentityDomain::Thread),
            ("turn_id", IdentityDomain::Turn),
            ("parent_turn_id", IdentityDomain::Turn),
            ("root_turn_id", IdentityDomain::Turn),
        ] {
            if let Some(value) = metadata.get_mut(name) {
                self.rewrite_value(value, domain);
            }
        }
        if let Some(value) = metadata.get_mut(WINDOW_HEADER) {
            self.rewrite_window_value(value);
        }
        if let Some(value) = metadata.get_mut("window_id") {
            self.rewrite_window_value(value);
        }
        if let Some(value) = metadata.get_mut(TURN_METADATA_HEADER)
            && let Some(raw) = value.as_str()
        {
            *value = Value::String(self.rewrite_serialized_metadata(raw)?);
        }
        Ok(())
    }

    fn rewrite_header(
        &self,
        headers: &mut HeaderMap,
        name: &'static str,
        domain: IdentityDomain,
    ) -> Result<(), ()> {
        let Some(value) = headers.get(name) else {
            return Ok(());
        };
        let raw = value.to_str().map_err(|_| ())?.to_string();
        self.insert_header(headers, name, &self.id(domain, &raw));
        Ok(())
    }

    fn rewrite_window_header(&self, headers: &mut HeaderMap, name: &'static str) -> Result<(), ()> {
        let Some(value) = headers.get(name) else {
            return Ok(());
        };
        let raw = value.to_str().map_err(|_| ())?.to_string();
        self.insert_header(headers, name, &self.window_id(&raw));
        Ok(())
    }

    fn rewrite_serialized_header(
        &self,
        headers: &mut HeaderMap,
        name: &'static str,
    ) -> Result<(), ()> {
        let Some(value) = headers.get(name) else {
            return Ok(());
        };
        let raw = value.to_str().map_err(|_| ())?.to_string();
        let rewritten = self.rewrite_serialized_metadata(&raw)?;
        self.insert_header(headers, name, &rewritten);
        Ok(())
    }

    fn rewrite_serialized_metadata(&self, raw: &str) -> Result<String, ()> {
        let mut value = serde_json::from_str::<Value>(raw).map_err(|_| ())?;
        self.rewrite_metadata(value.as_object_mut().ok_or(())?)?;
        to_ascii_json_string(&value).map_err(|_| ())
    }

    fn rewrite_value(&self, value: &mut Value, domain: IdentityDomain) {
        let Some(raw) = value.as_str() else {
            return;
        };
        *value = Value::String(self.id(domain, raw));
    }

    fn rewrite_window_value(&self, value: &mut Value) {
        let Some(raw) = value.as_str() else {
            return;
        };
        *value = Value::String(self.window_id(raw));
    }

    fn window_id(&self, raw: &str) -> String {
        if raw.is_empty() {
            return String::new();
        }
        if let Some((thread, suffix)) = raw.rsplit_once(':')
            && !thread.is_empty()
            && !suffix.is_empty()
            && suffix.chars().all(|character| character.is_ascii_digit())
        {
            return format!("{}:{suffix}", self.id(IdentityDomain::Thread, thread));
        }
        self.id(IdentityDomain::Window, raw)
    }

    fn id(&self, domain: IdentityDomain, raw: &str) -> String {
        if raw.is_empty() {
            return String::new();
        }
        let mut mac = HmacSha256::new_from_slice(&self.key)
            .expect("HMAC-SHA256 accepts keys of every length");
        mac.update(DERIVATION_DOMAIN);
        mac.update(&[0]);
        mac.update(self.downstream_scope.as_bytes());
        mac.update(&[0]);
        mac.update(domain.label());
        mac.update(&[0]);
        mac.update(raw.as_bytes());
        let digest = mac.finalize().into_bytes();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x80;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid::from_bytes(bytes).to_string()
    }

    fn insert_header(&self, headers: &mut HeaderMap, name: &'static str, value: &str) {
        if let Ok(value) = HeaderValue::from_str(value) {
            headers.insert(HeaderName::from_static(name), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_pseudonyms_are_stable_scoped_and_uuid_v8() {
        let first = RequestPseudonymizer::new("acct_first", "key_first");
        let same = first.id(IdentityDomain::Thread, "thread-original");
        assert_eq!(same, first.id(IdentityDomain::Thread, "thread-original"));
        assert_ne!(same, first.id(IdentityDomain::Session, "thread-original"));
        assert_ne!(
            same,
            RequestPseudonymizer::new("acct_first", "key_second")
                .id(IdentityDomain::Thread, "thread-original")
        );
        assert_ne!(
            same,
            RequestPseudonymizer::new("acct_second", "key_first")
                .id(IdentityDomain::Thread, "thread-original")
        );
        assert_eq!(Uuid::parse_str(&same).expect("UUID").get_version_num(), 8);
    }

    #[test]
    fn converged_installation_is_stateless_and_account_scoped() {
        let first = RequestPseudonymizer::converged_installation_id("acct_first");
        assert_eq!(
            first,
            RequestPseudonymizer::converged_installation_id("acct_first")
        );
        assert_ne!(
            first,
            RequestPseudonymizer::converged_installation_id("acct_second")
        );
        assert_eq!(Uuid::parse_str(&first).expect("UUID").get_version_num(), 8);
    }

    #[test]
    fn identity_projection_rewrites_nested_metadata_and_preserves_window_suffix() {
        let pseudonymizer = RequestPseudonymizer::new("acct_test", "key_test");
        let mut headers = HeaderMap::new();
        headers.insert("thread-id", "thread-raw".parse().expect("header"));
        headers.insert(WINDOW_HEADER, "thread-raw:0".parse().expect("header"));
        headers.insert(
            TURN_METADATA_HEADER,
            r#"{"thread_id":"thread-raw","turn_id":"turn-raw","future":true}"#
                .parse()
                .expect("header"),
        );
        let mut body = serde_json::json!({
            "prompt_cache_key": "cache-raw",
            "client_metadata": {
                "thread_id": "thread-raw",
                "x-codex-window-id": "thread-raw:0",
                "x-codex-turn-metadata": "{\"thread_id\":\"thread-raw\",\"turn_id\":\"turn-raw\",\"future\":true}"
            }
        });
        pseudonymizer
            .apply(&mut headers, body.as_object_mut().expect("object"))
            .expect("identity projection");

        let thread = headers["thread-id"].to_str().expect("thread");
        assert_ne!(thread, "thread-raw");
        assert_eq!(
            headers[WINDOW_HEADER].to_str().expect("window"),
            format!("{thread}:0")
        );
        assert_eq!(body["client_metadata"]["thread_id"], thread);
        assert_eq!(
            body["client_metadata"][WINDOW_HEADER],
            format!("{thread}:0")
        );
        let metadata: Value = serde_json::from_str(
            body["client_metadata"][TURN_METADATA_HEADER]
                .as_str()
                .expect("turn metadata"),
        )
        .expect("metadata JSON");
        assert_eq!(metadata["thread_id"], thread);
        assert_ne!(metadata["turn_id"], "turn-raw");
        assert_eq!(metadata["future"], true);
        assert_ne!(body["prompt_cache_key"], "cache-raw");
    }
}
