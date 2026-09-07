use super::*;
use serde_json::json;

#[test]
fn compressed_terminals_match_a_reference_scan_under_divergence_and_interior_insertions() {
    let mut trie = Radix::default();
    let mut entries = Vec::new();
    let mut seed = 17_u64;
    for index in 0..300 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let path: Vec<_> = (0..1 + (seed % 12))
            .map(|bit| 1 + ((seed >> (bit * 3)) & 7))
            .collect();
        trie.insert(&path, index.to_string());
        entries.push((path, index.to_string()));
    }
    for (path, _) in &entries {
        let mut query = path.clone();
        query.extend([3, 7, 7]);
        let mut expected: Vec<_> = entries
            .iter()
            .filter(|(path, _)| query.starts_with(path))
            .map(|(path, id)| (path.len(), id.clone()))
            .collect();
        let mut actual = trie.prefixes(&query);
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
    }
}

#[test]
fn exact_interning_never_treats_omitted_ids_as_a_transitive_wildcard() {
    let mut pool = Interner::default();
    let item = |id| json!({"type":"message","role":"assistant","id":id,"content":[]});
    let a = pool.intern(item("a"));
    let b = pool.intern(item("b"));
    let no_id = json!({"type":"message","role":"assistant","content":[]});
    let omitted = pool.intern(no_id.clone());
    assert_eq!(a.key.id, b.key.id);
    assert_eq!(a.key.id, omitted.key.id);
    assert!(!Arc::ptr_eq(&a, &b));
    assert!(Arc::ptr_eq(&a, &pool.intern(item("a"))));
    assert!(ids_compatible(&no_id, &a.value));
    assert!(ids_compatible(&no_id, &b.value));
    assert!(!ids_compatible(&a.value, &b.value));
    let history = History::extend(None, vec![a.clone(), a]);
    assert_eq!(history.values().len(), 2);
    let cost = pool.cost();
    drop((history, b, omitted));
    pool.sweep();
    assert!(pool.cost() < cost);
}

#[test]
fn call_id_compatibility_keeps_full_items_distinct_and_preserves_reference_ids() {
    let mut pool = Interner::default();
    let call = |id| {
        json!({"type":"function_call","id":id,"call_id":"call",
        "name":"tool","arguments":"{}","status":"completed"})
    };
    let a = pool.intern(call("a"));
    let b = pool.intern(call("b"));
    let omitted = json!({"type":"function_call","call_id":"call","name":"tool","arguments":"{}"});
    let c = pool.intern(omitted.clone());
    assert_eq!(a.key.id, b.key.id);
    assert_eq!(a.key.id, c.key.id);
    assert!(!Arc::ptr_eq(&a, &b) && !Arc::ptr_eq(&a, &c));
    assert!(ids_compatible(&omitted, &a.value));
    assert!(ids_compatible(&omitted, &b.value));
    assert!(!ids_compatible(&a.value, &b.value));
    assert!(!ids_compatible(&a.value, &omitted));
    for kind in [
        "reasoning",
        "additional_tools",
        "item_reference",
        "file_search_call",
        "program",
    ] {
        let supplied = json!({"type":kind,"id":"required-id","call_id":"call","content":[]});
        let mut missing = supplied.clone();
        missing.as_object_mut().unwrap().remove("id");
        assert!(candidate_key(&supplied) != candidate_key(&missing));
        assert!(!ids_compatible(&missing, &supplied));
    }
    for call_id in [Value::Null, json!(""), json!(" \n"), json!({"id":"nested"})] {
        let supplied = json!({"type":"function_call","call_id":call_id,"id":"required-id"});
        let mut missing = supplied.clone();
        missing.as_object_mut().unwrap().remove("id");
        assert!(candidate_key(&supplied) != candidate_key(&missing));
    }
}

#[test]
fn empty_output_decoration_is_schema_bounded_and_does_not_change_exact_storage() {
    let mut pool = Interner::default();
    let full = json!({"type":"message","role":"assistant","content":[
        {"type":"output_text","text":"answer","annotations":[],"logprobs":[]} ]});
    let reduced = json!({"type":"message","role":"assistant","content":[
        {"type":"output_text","text":"answer"} ]});
    let a = pool.intern(full.clone());
    let b = pool.intern(reduced.clone());
    assert_eq!(a.key.id, b.key.id);
    assert!(!Arc::ptr_eq(&a, &b));
    assert!(a.value == full && b.value == reduced);
    assert!(!completion_items_compatible(&full, &reduced));
    for (role, kind) in [
        ("user", "input_text"),
        ("developer", "output_text"),
        ("assistant", "refusal"),
    ] {
        let mut full = full.clone();
        let mut reduced = reduced.clone();
        for value in [&mut full, &mut reduced] {
            value["role"] = role.into();
            value["content"][0]["type"] = kind.into();
        }
        assert!(candidate_key(&full) != candidate_key(&reduced));
    }
}

#[test]
fn digest_bucket_hits_always_verify_exact_bytes() {
    let mut pool = Interner::default();
    let a = pool.intern(json!({"type":"message","role":"user","content":[]}));
    let b = json!({"type":"message","role":"assistant","content":[]});
    // Inject a collision into both maps without changing the real SHA implementation.
    pool.keys.insert(
        Sha256::digest(candidate_key(&b)).into(),
        vec![Arc::downgrade(&a.key)],
    );
    pool.items.insert(
        Sha256::digest(canonical(&b)).into(),
        vec![Arc::downgrade(&a)],
    );
    let b = pool.intern(b);
    assert_ne!(a.key.id, b.key.id);
    assert!(!Arc::ptr_eq(&a, &b));
}

#[test]
fn compressed_repeated_history_preserves_all_8192_terminals_and_drops_iteratively() {
    use std::collections::HashSet;
    let start = std::time::Instant::now();
    let mut interner = Interner::default();
    let item =
        interner.intern(json!({"type":"message","role":"user","content":"repeat".repeat(512)}));
    let mut history = None;
    let mut index = Radix::default();
    let mut path = Vec::new();
    for n in 1..=8192 {
        history = Some(History::extend(history, vec![item.clone()]));
        path.push(item.key.id);
        index.insert(&path, format!("response-{n}"));
    }
    let history = history.unwrap();
    assert_eq!(history.len, 8192);
    assert_eq!(index.prefixes(&path).len(), 8192);
    assert_eq!(
        history.values().len(),
        8192,
        "repeated items must never collapse"
    );
    let retained =
        interner.cost() + index.cost() + history.retained_cost(&mut HashSet::new(), None);
    let unshared = (8192_u64 * 8193 / 2) * canonical(&item.value).len() as u64;
    assert!(retained as u64 * 100 < unshared);
    eprintln!(
        "index probe: terminals=8192 retained_estimate_bytes={retained} unshared_snapshot_bytes={unshared} elapsed_ms={}",
        start.elapsed().as_millis()
    );
    drop(history);
    drop(index);
    drop(item);
    interner.sweep();
    assert_eq!(interner.cost(), 0);
}
