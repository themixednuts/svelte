use serde_json::json;
use svelte_kit::json_stringify;

#[test]
fn json_stringify_matches_json_stringify_shape() {
    assert_eq!(
        json_stringify(&json!({ "answer": 42, "ok": true })).unwrap(),
        r#"{"answer":42,"ok":true}"#
    );
}
