use base64::Engine;
use hmac::Hmac;
use hmac::Mac;
use sha2::Digest;
use sha2::Sha256;

use crate::request_state_types::WireIdDomain;

type HmacSha256 = Hmac<Sha256>;

const KEY_DOMAIN: &[u8] = b"mini-sub2api/request-state-key/v1";
const FILE_DOMAIN: &[u8] = b"mini-sub2api/request-state-file/v1";

#[derive(Clone)]
pub(crate) struct LookupKeyFactory {
    key: [u8; 32],
    scope: String,
}

impl LookupKeyFactory {
    pub(crate) fn new(account_namespace: &str, downstream_scope: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(KEY_DOMAIN);
        digest.update([0]);
        digest.update(account_namespace.as_bytes());
        Self {
            key: digest.finalize().into(),
            scope: downstream_scope.to_string(),
        }
    }

    pub(crate) fn account_state_ref(account_namespace: &str) -> String {
        let mut digest = Sha256::new();
        digest.update(FILE_DOMAIN);
        digest.update([0]);
        digest.update(account_namespace.as_bytes());
        format!("rs_{}", encode(digest.finalize().as_slice()))
    }

    pub(crate) fn scope_key(&self) -> String {
        self.lookup("scope", &[self.scope.as_bytes()])
    }

    pub(crate) fn identity(&self, kind: &str, raw: &str) -> String {
        self.lookup(kind, &[raw.as_bytes()])
    }

    pub(crate) fn derived(&self, kind: &str, components: &[&[u8]]) -> String {
        self.lookup(kind, components)
    }

    pub(crate) fn wire_downstream(&self, domain: WireIdDomain, raw: &str) -> String {
        self.lookup(
            "wire-downstream",
            &[wire_domain(domain).as_bytes(), raw.as_bytes()],
        )
    }

    pub(crate) fn wire_upstream(&self, domain: WireIdDomain, raw: &str) -> String {
        self.lookup(
            "wire-upstream",
            &[wire_domain(domain).as_bytes(), raw.as_bytes()],
        )
    }

    fn lookup(&self, kind: &str, components: &[&[u8]]) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("SHA-256 HMAC key length");
        update_component(&mut mac, self.scope.as_bytes());
        update_component(&mut mac, kind.as_bytes());
        for component in components {
            update_component(&mut mac, component);
        }
        format!("lk_{}", encode(mac.finalize().into_bytes().as_slice()))
    }
}

fn update_component(mac: &mut HmacSha256, value: &[u8]) {
    mac.update(&(value.len() as u64).to_be_bytes());
    mac.update(value);
}

fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn wire_domain(domain: WireIdDomain) -> &'static str {
    match domain {
        WireIdDomain::Installation => "installation",
        WireIdDomain::Session => "session",
        WireIdDomain::Thread => "thread",
        WireIdDomain::Turn => "turn",
        WireIdDomain::Response => "response",
        WireIdDomain::Conversation => "conversation",
        WireIdDomain::Stream => "stream",
        WireIdDomain::Item => "item",
        WireIdDomain::Call => "call",
        WireIdDomain::Approval => "approval",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_stable_scoped_and_domain_separated() {
        let first = LookupKeyFactory::new("account-a", "scope-a");
        let same = LookupKeyFactory::new("account-a", "scope-a");
        let other_scope = LookupKeyFactory::new("account-a", "scope-b");
        let other_account = LookupKeyFactory::new("account-b", "scope-a");

        let key = first.identity("conversation", "raw-id");
        assert_eq!(key, same.identity("conversation", "raw-id"));
        assert_ne!(key, first.identity("turn", "raw-id"));
        assert_ne!(key, other_scope.identity("conversation", "raw-id"));
        assert_ne!(key, other_account.identity("conversation", "raw-id"));
        assert_eq!(key.len(), 46);
    }

    #[test]
    fn wire_directions_and_domains_do_not_alias() {
        let keys = LookupKeyFactory::new("account", "scope");
        let downstream = keys.wire_downstream(WireIdDomain::Response, "resp_1");
        assert_ne!(
            downstream,
            keys.wire_upstream(WireIdDomain::Response, "resp_1")
        );
        assert_ne!(
            downstream,
            keys.wire_downstream(WireIdDomain::Item, "resp_1")
        );
    }

    #[test]
    fn account_state_refs_hide_the_namespace() {
        let reference = LookupKeyFactory::account_state_ref("sensitive-account-id");
        assert!(reference.starts_with("rs_"));
        assert_eq!(reference.len(), 46);
        assert!(!reference.contains("sensitive"));
        assert_ne!(
            reference,
            LookupKeyFactory::account_state_ref("other-account-id")
        );
    }
}
