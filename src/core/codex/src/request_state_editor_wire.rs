use super::RequestStateEditor;
use super::touch_day;
use anyhow::Result;
use uuid::Uuid;

use crate::request_state_types::WireIdDomain;
use crate::request_state_types::WireIdEntry;
use crate::request_state_types::WireIdOrigin;
use crate::request_state_types::WireIdOwner;
use crate::request_state_types::validate_wire_id;

impl RequestStateEditor<'_> {
    pub(crate) fn existing_wire_from_downstream(
        &mut self,
        domain: WireIdDomain,
        downstream_id: &str,
    ) -> Result<Option<String>> {
        validate_wire_id(downstream_id)?;
        let downstream_lookup = self.keys.wire_downstream(domain, downstream_id);
        let Some(entry) = self.scope().wire_ids.get(&downstream_lookup) else {
            return Ok(None);
        };
        anyhow::ensure!(
            entry.domain == domain && entry.downstream_id == downstream_id,
            "wire ID downstream collision"
        );
        let upstream_id = entry.upstream_id.clone();
        self.touch_wire(&downstream_lookup)?;
        Ok(Some(upstream_id))
    }

    pub(crate) fn bind_wire_pair(
        &mut self,
        domain: WireIdDomain,
        downstream_id: &str,
        upstream_id: &str,
    ) -> Result<()> {
        validate_wire_id(downstream_id)?;
        validate_wire_id(upstream_id)?;
        if downstream_id == upstream_id {
            return Ok(());
        }
        let downstream_lookup = self.keys.wire_downstream(domain, downstream_id);
        if let Some(entry) = self.scope().wire_ids.get(&downstream_lookup) {
            anyhow::ensure!(
                entry.domain == domain
                    && entry.downstream_id == downstream_id
                    && entry.upstream_id == upstream_id,
                "wire ID downstream relationship changed for {domain:?}"
            );
            self.touch_wire(&downstream_lookup)?;
            return Ok(());
        }
        let upstream_lookup = self.keys.wire_upstream(domain, upstream_id);
        if let Some(existing_downstream_lookup) = self
            .scope()
            .wire_upstream_index
            .get(&upstream_lookup)
            .cloned()
        {
            // Conflicting root carriers intentionally converge. The first canonical carrier is
            // the reversible downstream representation for the projected identifier.
            self.touch_wire(&existing_downstream_lookup)?;
            return Ok(());
        }
        self.insert_wire_pair(domain, WireIdOrigin::Downstream, downstream_id, upstream_id)?;
        Ok(())
    }

    pub(crate) fn wire_from_downstream(
        &mut self,
        domain: WireIdDomain,
        downstream_id: &str,
    ) -> Result<String> {
        validate_wire_id(downstream_id)?;
        let downstream_lookup = self.keys.wire_downstream(domain, downstream_id);
        if let Some(entry) = self.scope().wire_ids.get(&downstream_lookup) {
            anyhow::ensure!(
                entry.domain == domain && entry.downstream_id == downstream_id,
                "wire ID downstream collision"
            );
            let upstream_id = entry.upstream_id.clone();
            self.touch_wire(&downstream_lookup)?;
            return Ok(upstream_id);
        }
        let upstream_id = self.unique_wire_alias(domain, downstream_id, false)?;
        self.insert_wire_pair(
            domain,
            WireIdOrigin::Downstream,
            downstream_id,
            &upstream_id,
        )
    }

    pub(crate) fn wire_from_upstream(
        &mut self,
        domain: WireIdDomain,
        upstream_id: &str,
    ) -> Result<String> {
        validate_wire_id(upstream_id)?;
        let upstream_lookup = self.keys.wire_upstream(domain, upstream_id);
        if let Some(downstream_lookup) = self
            .scope()
            .wire_upstream_index
            .get(&upstream_lookup)
            .cloned()
        {
            let entry = self
                .scope()
                .wire_ids
                .get(&downstream_lookup)
                .ok_or_else(|| anyhow::anyhow!("wire ID reverse index is dangling"))?;
            anyhow::ensure!(
                entry.domain == domain && entry.upstream_id == upstream_id,
                "wire ID upstream collision"
            );
            let downstream_id = entry.downstream_id.clone();
            self.touch_wire(&downstream_lookup)?;
            return Ok(downstream_id);
        }
        let downstream_id = self.unique_wire_alias(domain, upstream_id, true)?;
        self.insert_wire_pair(domain, WireIdOrigin::Upstream, &downstream_id, upstream_id)
            .map(|_| downstream_id)
    }

    pub(crate) fn wire_from_upstream_response(
        &mut self,
        upstream_id: &str,
        owner: Option<&WireIdOwner>,
    ) -> Result<String> {
        let downstream_id = self.wire_from_upstream(WireIdDomain::Response, upstream_id)?;
        if let Some(owner) = owner {
            self.set_response_owner(upstream_id, owner)?;
        }
        Ok(downstream_id)
    }

    pub(crate) fn response_owner_from_downstream(
        &mut self,
        downstream_id: &str,
    ) -> Result<Option<WireIdOwner>> {
        validate_wire_id(downstream_id)?;
        let downstream_lookup = self
            .keys
            .wire_downstream(WireIdDomain::Response, downstream_id);
        let Some(entry) = self.scope().wire_ids.get(&downstream_lookup) else {
            return Ok(None);
        };
        anyhow::ensure!(
            entry.domain == WireIdDomain::Response && entry.downstream_id == downstream_id,
            "response wire ID downstream collision"
        );
        let owner = entry.owner.clone();
        self.touch_wire(&downstream_lookup)?;
        if let Some(owner) = &owner {
            self.protect_owner(owner)?;
        }
        Ok(owner)
    }

    fn insert_wire_pair(
        &mut self,
        domain: WireIdDomain,
        origin: WireIdOrigin,
        downstream_id: &str,
        upstream_id: &str,
    ) -> Result<String> {
        let downstream_lookup = self.keys.wire_downstream(domain, downstream_id);
        let upstream_lookup = self.keys.wire_upstream(domain, upstream_id);
        anyhow::ensure!(
            !self.scope().wire_ids.contains_key(&downstream_lookup)
                && !self
                    .scope()
                    .wire_upstream_index
                    .contains_key(&upstream_lookup),
            "wire ID bijection collision"
        );
        let entry = WireIdEntry {
            domain,
            origin,
            downstream_id: downstream_id.to_string(),
            upstream_id: upstream_id.to_string(),
            upstream_lookup: upstream_lookup.clone(),
            owner: None,
            last_seen_day: self.day,
        };
        let scope = self.scope_mut();
        scope
            .wire_upstream_index
            .insert(upstream_lookup, downstream_lookup.clone());
        scope.wire_ids.insert(downstream_lookup.clone(), entry);
        self.changed = true;
        self.protected
            .wire_ids
            .insert((self.scope_key.clone(), downstream_lookup));
        Ok(upstream_id.to_string())
    }

    fn set_response_owner(&mut self, upstream_id: &str, owner: &WireIdOwner) -> Result<()> {
        self.protect_owner(owner)?;
        let upstream_lookup = self.keys.wire_upstream(WireIdDomain::Response, upstream_id);
        let downstream_lookup = self
            .scope()
            .wire_upstream_index
            .get(&upstream_lookup)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("response wire ID mapping is missing"))?;
        let changed = {
            let entry = self
                .scope_mut()
                .wire_ids
                .get_mut(&downstream_lookup)
                .ok_or_else(|| anyhow::anyhow!("response wire ID reverse index is dangling"))?;
            anyhow::ensure!(
                entry.domain == WireIdDomain::Response && entry.upstream_id == upstream_id,
                "response wire ID upstream collision"
            );
            match &entry.owner {
                Some(existing) => {
                    anyhow::ensure!(
                        existing == owner,
                        "response request identity relationship changed"
                    );
                    false
                }
                None => {
                    entry.owner = Some(owner.clone());
                    true
                }
            }
        };
        self.changed |= changed;
        self.protected
            .wire_ids
            .insert((self.scope_key.clone(), downstream_lookup));
        Ok(())
    }

    fn protect_owner(&mut self, owner: &WireIdOwner) -> Result<()> {
        let (_, conversation) = self
            .conversation_by_id(&owner.session_id)
            .ok_or_else(|| anyhow::anyhow!("response owner session is missing"))?;
        anyhow::ensure!(
            conversation.id == owner.session_id,
            "response owner session changed"
        );
        if owner.thread_id != owner.session_id {
            let (_, thread) = self
                .child_thread_by_id(&owner.thread_id)
                .ok_or_else(|| anyhow::anyhow!("response owner thread is missing"))?;
            anyhow::ensure!(
                thread.session_id == owner.session_id,
                "response owner thread crosses sessions"
            );
        }
        Ok(())
    }

    fn unique_wire_alias(
        &self,
        domain: WireIdDomain,
        source: &str,
        downstream: bool,
    ) -> Result<String> {
        for _ in 0..8 {
            let candidate = wire_alias(domain, source);
            let lookup = if downstream {
                self.keys.wire_downstream(domain, &candidate)
            } else {
                self.keys.wire_upstream(domain, &candidate)
            };
            let vacant = if downstream {
                !self.scope().wire_ids.contains_key(&lookup)
            } else {
                !self.scope().wire_upstream_index.contains_key(&lookup)
            };
            if vacant && candidate != source {
                return Ok(candidate);
            }
        }
        anyhow::bail!("unable to allocate unique wire ID pseudonym")
    }

    fn touch_wire(&mut self, downstream_lookup: &str) -> Result<()> {
        let day = self.day;
        let entry = self
            .scope_mut()
            .wire_ids
            .get_mut(downstream_lookup)
            .ok_or_else(|| anyhow::anyhow!("wire ID mapping is missing"))?;
        self.changed |= touch_day(&mut entry.last_seen_day, day);
        self.protected
            .wire_ids
            .insert((self.scope_key.clone(), downstream_lookup.to_string()));
        Ok(())
    }
}

fn wire_alias(domain: WireIdDomain, source: &str) -> String {
    if Uuid::parse_str(source).is_ok() {
        return if domain == WireIdDomain::Installation {
            Uuid::new_v4().to_string()
        } else {
            Uuid::now_v7().to_string()
        };
    }
    let prefix = source
        .split_once('_')
        .map(|(prefix, _)| prefix)
        .filter(|prefix| valid_wire_prefix(prefix))
        .unwrap_or_else(|| default_wire_prefix(domain));
    format!("{prefix}_{}", Uuid::now_v7())
}

fn valid_wire_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && prefix.len() <= 32
        && prefix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn default_wire_prefix(domain: WireIdDomain) -> &'static str {
    match domain {
        WireIdDomain::Installation => "installation",
        WireIdDomain::Session => "session",
        WireIdDomain::Thread => "thread",
        WireIdDomain::Turn => "turn",
        WireIdDomain::Response => "resp",
        WireIdDomain::Conversation => "conv",
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
    fn aliases_preserve_safe_prefixes_and_use_uuid_v7() {
        let alias = wire_alias(WireIdDomain::Response, "resp_provider-value");
        let (_, uuid) = alias.split_once('_').expect("prefixed alias");
        assert!(alias.starts_with("resp_"));
        assert_eq!(Uuid::parse_str(uuid).expect("UUID").get_version_num(), 7);

        let fallback = wire_alias(WireIdDomain::Call, "opaque-without-prefix");
        assert!(fallback.starts_with("call_"));
    }
}
