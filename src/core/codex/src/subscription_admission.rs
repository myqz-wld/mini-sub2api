use super::*;

impl ContextStore {
    pub(crate) fn admit(
        &self,
        plan: ContextPlan,
        identity: &ResolvedRequestIdentity,
        upstream_format: Format,
        compaction: Option<crate::request_compaction::PendingCompaction>,
        lineage: super::HistoryLineage,
    ) -> Result<Operation, Error> {
        let mut inner = self.inner.lock().map_err(|_| Error::StateUnavailable)?;
        let now = Instant::now();
        inner.expire(now);
        let session = &identity.session_id;
        let branch = plan
            .branch
            .clone()
            .unwrap_or_else(|| identity.thread_id.clone());
        let lane = format!(
            "{}:{}:{}:{}",
            session,
            identity.thread_id,
            identity.turn_id.as_deref().unwrap_or(""),
            branch
        );
        if inner.operations.values().any(|operation| {
            operation.scope == plan.scope
                && (operation.lane == lane
                    || plan
                        .socket
                        .as_ref()
                        .is_some_and(|socket| operation.record.socket.as_ref() == Some(socket)))
        }) {
            return Err(Error::StateUnavailable);
        }
        let reserved = inner
            .reservations
            .remove(&plan.admission.0.id)
            .ok_or(Error::StateUnavailable)?
            .reserved
            .saturating_add(plan.input_metadata_charge)
            .saturating_add(lineage.cost);
        if !inner.make_room(&self.limits, &plan.scope, session, reserved) {
            return Err(Error::StateUnavailable);
        }
        let scope = inner.scopes.entry(plan.scope.clone()).or_default();
        while scope
            .records
            .values()
            .filter(|r| r.identity.session_id == *session)
            .count()
            >= self.limits.session_records
        {
            let victim = scope
                .records
                .iter()
                .filter(|(_, r)| r.identity.session_id == *session && r.socket.is_none())
                .min_by_key(|(_, r)| r.last_used)
                .map(|(id, _)| id.clone())
                .ok_or(Error::StateUnavailable)?;
            scope.records.remove(&victim);
            scope.rebuild_index();
        }
        let publication = Publication {
            raw_session: plan.evidence.session.clone(),
            selected_session: plan.session.clone(),
            raw_turn: plan.evidence.turn.clone(),
        };
        let parent = plan.baseline.as_ref().and_then(|r| r.history.clone());
        let hidden = parent
            .as_ref()
            .map(|history| history.hidden_reasoning())
            .unwrap_or_default();
        let delta = plan.evidence.previous.is_some();
        let effective_input = plan.restored_input.unwrap_or(plan.evidence.input);
        let history = if plan.external_context || (delta && parent.is_none()) {
            None
        } else {
            let shared_len = if delta {
                0
            } else {
                parent
                    .as_ref()
                    .filter(|base| {
                        base.len <= effective_input.len()
                            && base
                                .items()
                                .iter()
                                .zip(&effective_input)
                                .all(|(saved, current)| saved.value == *current)
                    })
                    .map_or(0, |base| base.len)
            };
            let input = effective_input
                .into_iter()
                .skip(shared_len)
                .map(|item| scope.interner.intern(item))
                .collect();
            Some(History::extend_with_hidden(
                if delta || shared_len > 0 {
                    parent
                } else {
                    None
                },
                input,
                &hidden,
            ))
        };
        let has_history = history.is_some();
        let settings_hash = Sha256::digest(canonical(&plan.settings)).into();
        let setup_hash = crate::subscription_index::setup_hash(&plan.settings);
        let stored_settings =
            has_history.then(|| scope.intern_settings(plan.settings, settings_hash));
        let record = Record {
            identity: identity.clone(),
            branch,
            caller_format: plan.caller_format,
            upstream_format,
            setup_hash,
            settings: stored_settings,
            history,
            dependencies: plan.dependencies,
            lineage,
            startup: (identity.request_kind == "prewarm" && plan.socket.is_some()).then(|| {
                crate::subscription_startup::StartupRouting::new(plan.admission.0.id.clone())
            }),
            socket: plan.socket,
            completed: false,
            compaction,
            compaction_key: None,
            last_used: now,
        };
        let id = plan.admission.0.id.clone();
        inner.operations.insert(
            id.clone(),
            Active {
                scope: plan.scope,
                record,
                publication: Some(publication),
                lane,
                reserved,
                output: BTreeMap::new(),
                observed_items: BTreeMap::new(),
                output_lifecycle: Default::default(),
                interruption: Default::default(),
                compaction_output: Default::default(),
                dependencies_available: true,
                output_bytes: 0,
                buffer_charge: 0,
                output_available: true,
                response_id: None,
            },
        );
        Ok(plan.admission)
    }
}

impl ContextStore {
    pub(crate) fn commit_admission(&self, operation: &Operation) -> Result<(), Error> {
        let mut inner = self.inner.lock().map_err(|_| Error::StateUnavailable)?;
        let active = inner
            .operations
            .get_mut(&operation.0.id)
            .ok_or(Error::StateUnavailable)?;
        let Some(publication) = active.publication.take() else {
            return Ok(());
        };
        let (key, identity, socket, branch) = (
            active.scope.clone(),
            active.record.identity.clone(),
            active.record.socket.clone(),
            active.record.branch.clone(),
        );
        let session = &identity.session_id;
        let routing_key = identity
            .turn_id
            .as_deref()
            .filter(|turn| !turn.is_empty())
            .map(|turn| format!("{branch}:{turn}"));
        if socket.as_ref().is_some_and(|socket| {
            inner.scopes.get(&key).is_some_and(|scope| {
                scope.records.values().any(|record| {
                    record.socket.as_ref() == Some(socket) && record.startup.is_some()
                })
            })
        }) && !inner.make_room(
            &self.limits,
            &key,
            session,
            1024 + 2 * routing_key.as_ref().map_or(0, String::len),
        ) {
            return Err(Error::StateUnavailable);
        }
        let now = Instant::now();
        let scope = inner.scopes.entry(key).or_default();
        let explicit = publication.raw_session.is_some();
        for raw in [
            publication.raw_session,
            publication.selected_session,
            Some(session.clone()),
        ]
        .into_iter()
        .flatten()
        {
            scope.aliases.insert(raw, session.clone());
        }
        if let Some(socket) = socket {
            scope.adopt_startup(&socket, &identity.thread_id, routing_key.as_deref());
            scope.bindings.insert(socket, session.clone());
        }
        for turn in [publication.raw_turn, identity.turn_id]
            .into_iter()
            .flatten()
        {
            scope.turns.insert(turn, session.clone());
        }
        scope
            .sessions
            .entry(session.clone())
            .and_modify(|s| {
                s.last_business = now;
                s.explicit |= explicit;
            })
            .or_insert(Session {
                explicit,
                last_business: now,
            });
        if explicit {
            scope.rebuild_index();
        }
        Ok(())
    }
}
