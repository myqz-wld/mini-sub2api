use super::*;

fn entry(scope: &str, branch: &str, token: &str, now: i64) -> Entry {
    Entry {
        scope: scope.into(),
        session: "synthetic-session".into(),
        thread: "synthetic-thread".into(),
        turn: "synthetic-turn".into(),
        branch: branch.into(),
        token: token.into(),
        seen_ms: now,
    }
}

fn fixture() -> (tempfile::TempDir, String) {
    let temp = tempfile::tempdir().unwrap();
    let state_ref = LookupKeyFactory::account_state_ref("synthetic-namespace");
    std::fs::write(
        state_path(temp.path(), &state_ref),
        b"synthetic identity marker",
    )
    .unwrap();
    (temp, state_ref)
}

#[test]
fn routing_roundtrip_preserves_empty_first_token_and_isolates_scopes_and_turns() {
    let (temp, state_ref) = fixture();
    let original = entry("scope-a", "synthetic-thread", "", 1000);
    assert_eq!(
        update(temp.path(), &state_ref, original.clone(), true, false).unwrap(),
        Some("".into())
    );
    let mut restart = original.clone();
    restart.token = "ignored-caller-value".into();
    assert_eq!(
        update(temp.path(), &state_ref, restart.clone(), false, true).unwrap(),
        Some("".into())
    );
    assert!(update(temp.path(), &state_ref, restart, true, false).is_err());
    let other = entry("scope-b", "synthetic-thread", "", 1000);
    assert!(
        update(temp.path(), &state_ref, other, false, true)
            .unwrap()
            .is_none()
    );
    let mut other = original.clone();
    other.turn = "synthetic-other-turn".into();
    assert!(
        update(temp.path(), &state_ref, other, false, true)
            .unwrap()
            .is_none()
    );
    let mut expired = original;
    expired.seen_ms += TTL_MS;
    assert!(
        update(temp.path(), &state_ref, expired, false, true)
            .unwrap()
            .is_none()
    );
}

#[test]
fn ambiguous_forks_are_not_selected_for_baseless_restart() {
    let (temp, state_ref) = fixture();
    for (branch, token) in [("fork-a", "token-a"), ("fork-b", "token-b")] {
        update(
            temp.path(),
            &state_ref,
            entry("scope", branch, token, 1000),
            true,
            false,
        )
        .unwrap();
    }
    assert!(
        update(
            temp.path(),
            &state_ref,
            entry("scope", "synthetic-thread", "", 1000),
            false,
            true
        )
        .is_err()
    );
    assert!(
        update(
            temp.path(),
            &state_ref,
            entry("scope", "new-independent-branch", "", 1000),
            false,
            true
        )
        .unwrap()
        .is_none()
    );
    update(
        temp.path(),
        &state_ref,
        entry("scope", "synthetic-thread", "token-a", 1000),
        true,
        false,
    )
    .unwrap();
    assert!(
        update(
            temp.path(),
            &state_ref,
            entry("scope", "synthetic-thread", "", 1000),
            false,
            true
        )
        .is_err()
    );
}

#[test]
fn routing_storage_is_bounded_private_and_cannot_resurrect_deleted_owners() {
    let (temp, state_ref) = fixture();
    let path = temp.path().join(format!("{state_ref}{SUFFIX}"));
    let mut state = RoutingState::default();
    for index in 0..160 {
        let entry = entry(
            "scope",
            &format!("branch-{index}"),
            &"x".repeat(64 * 1024),
            index,
        );
        state.entries.insert(entry.key(), entry);
    }
    write(temp.path(), &path, &mut state, None).unwrap();
    assert!(state.entries.len() < 160);
    assert!(std::fs::metadata(&path).unwrap().len() <= MAX_BYTES as u64);
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(read(&path).unwrap().entries.len(), state.entries.len());
    std::fs::remove_file(state_path(temp.path(), &state_ref)).unwrap();
    assert!(
        update(
            temp.path(),
            &state_ref,
            entry("scope", "branch", "token", 1000),
            true,
            false
        )
        .is_err()
    );
    remove(temp.path(), &state_ref).unwrap();
    assert!(!path.exists());
}

#[test]
fn entry_capacity_evicts_oldest_and_preserves_the_current_write() {
    let (temp, state_ref) = fixture();
    let path = temp.path().join(format!("{state_ref}{SUFFIX}"));
    let mut state = RoutingState::default();
    for index in 0..=MAX_ENTRIES {
        let value = entry("scope", &format!("branch-{index}"), "token", index as i64);
        state.entries.insert(value.key(), value);
    }
    let protected = entry("scope", "branch-0", "token", 0).key();
    let oldest_unprotected = entry("scope", "branch-1", "token", 1).key();
    write(temp.path(), &path, &mut state, Some(&protected)).unwrap();
    let restored = read(&path).unwrap();
    assert_eq!(restored.entries.len(), MAX_ENTRIES);
    assert!(restored.entries.contains_key(&protected));
    assert!(!restored.entries.contains_key(&oldest_unprotected));
    assert!(std::fs::metadata(&path).unwrap().len() < MAX_BYTES as u64);
}

#[test]
fn old_installations_start_empty_and_malformed_sidecars_fail_closed() {
    let (temp, state_ref) = fixture();
    let path = temp.path().join(format!("{state_ref}{SUFFIX}"));
    assert!(read(&path).unwrap().entries.is_empty());
    std::fs::write(&path, b"{\"version\":2,\"entries\":{}}").unwrap();
    assert!(read(&path).is_err());
    #[cfg(unix)]
    {
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("synthetic-missing-target", &path).unwrap();
        assert!(read(&path).is_err());
    }
}

#[test]
fn final_owner_removal_and_orphan_cleanup_remove_routing_sidecars() {
    for orphan in [false, true] {
        let (temp, state_ref) = fixture();
        update(
            temp.path(),
            &state_ref,
            entry("scope", "branch", "token", 1000),
            true,
            false,
        )
        .unwrap();
        let path = temp.path().join(format!("{state_ref}{SUFFIX}"));
        if orphan {
            std::fs::remove_file(state_path(temp.path(), &state_ref)).unwrap();
            super::super::cleanup_orphan_request_states(temp.path()).unwrap();
        } else {
            RequestStateStore::new(temp.path().into())
                .remove_credential_owner(&state_ref, "acct_synthetic_removed")
                .unwrap();
        }
        assert!(!path.exists());
    }
}
