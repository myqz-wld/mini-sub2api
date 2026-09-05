use super::*;

impl Inner {
    pub(crate) fn expire(&mut self, now: Instant) {
        let active: HashSet<_> = self
            .operations
            .values()
            .map(|op| (op.scope.clone(), op.record.identity.session_id.clone()))
            .chain(
                self.reservations
                    .values()
                    .map(|p| (p.scope.clone(), p.session.clone())),
            )
            .collect();
        for (key, scope) in &mut self.scopes {
            let expired: HashSet<_> = scope
                .sessions
                .iter()
                .filter(|(session, state)| {
                    !active.contains(&(key.clone(), (*session).clone()))
                        && now.saturating_duration_since(state.last_business) >= self.ttl
                })
                .map(|(session, _)| session.clone())
                .collect();
            let mut changed = false;
            for record in scope.records.values_mut() {
                if expired.contains(&record.identity.session_id) {
                    changed |= record.history.take().is_some();
                    record.settings = None;
                }
            }
            for tracked in self
                .baselines
                .values()
                .filter(|b| b.scope == *key && expired.contains(&b.session))
            {
                if let Some(state) = tracked.state.upgrade()
                    && let Ok(mut state) = state.try_lock()
                {
                    state.expire_baseline();
                }
            }
            if changed {
                scope.rebuild_index();
            }
            if !active.iter().any(|(scope_key, _)| scope_key == key) {
                scope.records.retain(|_, r| {
                    r.history.is_some()
                        || r.socket.is_some()
                        || !expired.contains(&r.identity.session_id)
                });
                scope.prune_metadata();
            }
            scope.interner.sweep();
            scope.settings_pool.retain(|_, values| {
                values.retain(|v| v.strong_count() > 0);
                !values.is_empty()
            });
        }
        self.scopes.retain(|key, scope| {
            !scope.sessions.is_empty()
                || !scope.records.is_empty()
                || !scope.bindings.is_empty()
                || active.iter().any(|(scope, _)| scope == key)
        });
        self.baselines.retain(|_, b| b.state.strong_count() > 0);
    }

    pub(crate) fn reserved(&self, scope: Option<&str>, session: Option<&str>) -> usize {
        self.operations
            .values()
            .filter(|op| {
                scope.is_none_or(|scope| op.scope == scope)
                    && session.is_none_or(|session| op.record.identity.session_id == session)
            })
            .map(|op| op.reserved)
            .sum::<usize>()
            + self
                .reservations
                .values()
                .filter(|p| {
                    scope.is_none_or(|key| p.scope == key)
                        && session.is_none_or(|owner| p.session == owner)
                })
                .map(|p| p.reserved)
                .sum::<usize>()
    }

    fn baseline_cost(&self, scope: Option<&str>, session: Option<&str>) -> usize {
        self.baselines
            .values()
            .filter(|b| {
                scope.is_none_or(|key| b.scope == key)
                    && session.is_none_or(|owner| b.session == owner)
            })
            .filter_map(|b| b.state.upgrade())
            .map(|b| {
                b.try_lock()
                    .map_or(self.busy_baseline_bytes, |b| b.retained_bytes())
            })
            .sum()
    }

    pub(crate) fn fits(
        &self,
        limits: &InferenceLimits,
        key: &str,
        session: &str,
        extra: usize,
    ) -> bool {
        let scope = self.scopes.get(key);
        let global = self.scopes.values().map(Scope::cost).sum::<usize>()
            + self.reserved(None, None)
            + self.baseline_cost(None, None);
        let keyed = scope.map_or(0, Scope::cost)
            + self.reserved(Some(key), None)
            + self.baseline_cost(Some(key), None);
        let local = scope.map_or(0, |scope| scope.session_cost(session))
            + self.reserved(Some(key), Some(session))
            + self.baseline_cost(Some(key), Some(session));
        global.saturating_add(extra) <= limits.global_bytes
            && keyed.saturating_add(extra) <= limits.key_bytes
            && local.saturating_add(extra) <= limits.session_bytes
    }

    pub(crate) fn make_room(
        &mut self,
        limits: &InferenceLimits,
        key: &str,
        session: &str,
        extra: usize,
    ) -> bool {
        while !self.fits(limits, key, session, extra) {
            let candidate = self
                .scopes
                .iter()
                .flat_map(|(scope_key, scope)| {
                    scope
                        .records
                        .iter()
                        .filter(|(_, record)| record.history.is_some())
                        .map(move |(id, record)| {
                            (
                                scope_key.clone(),
                                id.clone(),
                                record.identity.session_id.clone(),
                                record.last_used,
                            )
                        })
                })
                .filter(|(scope_key, _, session, _)| {
                    !self.operations.values().any(|op| {
                        op.scope == *scope_key && op.record.identity.session_id == *session
                    }) && !self
                        .reservations
                        .values()
                        .any(|p| p.scope == *scope_key && p.session == *session)
                })
                .min_by_key(|(scope, _, owner, seen)| (scope != key, owner != session, *seen));
            let Some((scope_key, id, _, _)) = candidate else {
                if self.reclaim_comparison(key, session) {
                    continue;
                }
                let stale = self
                    .scopes
                    .iter()
                    .flat_map(|(scope_key, scope)| {
                        scope
                            .records
                            .iter()
                            .filter(|(_, r)| r.socket.is_none())
                            .map(move |(id, r)| {
                                (
                                    scope_key.clone(),
                                    id.clone(),
                                    r.identity.session_id.clone(),
                                    r.last_used,
                                )
                            })
                    })
                    .filter(|(scope_key, _, owner, _)| {
                        !self.operations.values().any(|op| {
                            op.scope == *scope_key && op.record.identity.session_id == *owner
                        }) && !self
                            .reservations
                            .values()
                            .any(|p| p.scope == *scope_key && p.session == *owner)
                    })
                    .min_by_key(|(scope, _, owner, seen)| (scope != key, owner != session, *seen));
                let Some((scope_key, id, _, _)) = stale else {
                    return false;
                };
                let scope = self
                    .scopes
                    .get_mut(&scope_key)
                    .expect("metadata eviction scope");
                scope.records.remove(&id);
                if !self.operations.values().any(|op| op.scope == scope_key)
                    && !self.reservations.values().any(|p| p.scope == scope_key)
                {
                    scope.prune_metadata();
                }
                scope.rebuild_index();
                scope.interner.sweep();
                continue;
            };
            let scope = self.scopes.get_mut(&scope_key).expect("eviction scope");
            let record = scope.records.get_mut(&id).expect("eviction record");
            record.history = None;
            record.settings = None;
            scope.rebuild_index();
            scope.interner.sweep();
        }
        true
    }

    fn reclaim_comparison(&self, key: &str, session: &str) -> bool {
        let mut eligible: Vec<_> = self
            .baselines
            .values()
            .filter(|baseline| {
                !self.operations.values().any(|op| {
                    op.scope == baseline.scope && op.record.identity.session_id == baseline.session
                }) && !self.reservations.values().any(|pending| {
                    pending.scope == baseline.scope && pending.session == baseline.session
                })
            })
            .collect();
        eligible.sort_by_key(|baseline| {
            (
                baseline.scope != key,
                baseline.session != session,
                self.scopes
                    .get(&baseline.scope)
                    .and_then(|s| s.sessions.get(&baseline.session))
                    .map(|s| s.last_business),
            )
        });
        for baseline in eligible {
            if let Some(state) = baseline.state.upgrade()
                && let Ok(mut state) = state.try_lock()
                && state.retained_bytes() > 0
            {
                state.abandon_cached_bodies();
                return true;
            }
        }
        false
    }
}
