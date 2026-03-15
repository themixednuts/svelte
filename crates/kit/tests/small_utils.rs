use std::{cell::Cell, rc::Rc};

use svelte_kit::{HashValue, PromiseState, compact, forked, hash_values, once, with_resolvers};

#[test]
fn compact_removes_none_values() {
    let compacted = compact(vec![Some(1), None, Some(3), None]);
    assert_eq!(compacted, vec![1, 3]);
}

#[test]
fn hash_values_matches_upstream_djb2_output_shape() {
    assert_eq!(hash_values([HashValue::Str("abc")]).unwrap(), "375kp1");
    assert_eq!(
        hash_values([HashValue::Str("abc"), HashValue::Bytes(&[1, 2, 3])]).unwrap(),
        "pbv8px"
    );
}

#[test]
fn hash_values_rejects_empty_input_only_by_returning_valid_hash() {
    assert_eq!(
        hash_values(std::iter::empty::<HashValue<'_>>()).unwrap(),
        "45h"
    );
}

#[test]
fn once_only_invokes_callback_once() {
    let count = Rc::new(Cell::new(0));
    let count_for_callback = Rc::clone(&count);
    let mut callback = once(move || {
        count_for_callback.set(count_for_callback.get() + 1);
        String::from("ready")
    });

    assert_eq!(callback.call(), "ready");
    assert_eq!(callback.call(), "ready");
    assert_eq!(count.get(), 1);
}

#[tokio::test]
async fn with_resolvers_supports_resolve_and_reject() {
    let resolvers = with_resolvers::<u32, &'static str>();
    assert!(resolvers.resolve(42));
    assert_eq!(resolvers.promise().await, PromiseState::Resolved(42));

    let rejected = with_resolvers::<u32, &'static str>();
    assert!(rejected.reject("boom"));
    assert_eq!(rejected.promise().await, PromiseState::Rejected("boom"));
}

#[test]
fn forked_executes_callback_in_joined_thread() {
    let run = forked(|value: u32| value + 1);
    assert_eq!(run(41).expect("thread should finish"), 42);
}
