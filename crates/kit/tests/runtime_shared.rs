use serde_json::json;
use svelte_kit::{
    Error, RuntimeSharedError, create_remote_key, validate_depends, validate_load_response,
};

#[test]
fn warns_for_special_depends_schemes() {
    assert_eq!(
        validate_depends("/blog", "moz-icon:foo"),
        Some(
            "/blog: Calling `depends('moz-icon:foo')` will throw an error in Firefox because `moz-icon` is a special URI scheme"
                .to_string()
        )
    );
    assert_eq!(validate_depends("/blog", "custom:foo"), None);
}

#[test]
fn validates_load_response_shape() {
    validate_load_response(&json!({ "ok": true }), Some("in +page.js")).expect("plain object");
    validate_load_response(&json!(null), Some("in +page.js")).expect("null is allowed");

    let error = validate_load_response(&json!(["bad"]), Some("in +page.js"))
        .expect_err("arrays should fail");
    assert!(matches!(
        error,
        Error::RuntimeShared(RuntimeSharedError::InvalidLoadResponse { ref kind, .. })
            if kind == "an array"
    ));
    assert_eq!(
        error.to_string(),
        "a load function in +page.js returned an array, but must return a plain object at the top level (i.e. `return {...}`)"
    );
}

#[test]
fn creates_remote_keys() {
    assert_eq!(
        create_remote_key("hash/name", "payload"),
        "hash/name/payload"
    );
}
