use svelte_kit::{
    GENERATED_COMMENT, MUTATIVE_METHODS, PAGE_METHODS_PUBLIC, SVELTE_KIT_ASSETS, endpoint_methods,
};

#[test]
fn top_level_constants_match_upstream_values() {
    assert_eq!(SVELTE_KIT_ASSETS, "/_svelte_kit_assets");
    assert_eq!(
        GENERATED_COMMENT,
        "// this file is generated — do not edit it\n"
    );
    assert_eq!(
        endpoint_methods(),
        &["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"]
    );
    assert_eq!(MUTATIVE_METHODS, &["POST", "PUT", "PATCH", "DELETE"]);
    assert_eq!(PAGE_METHODS_PUBLIC, &["GET", "POST", "HEAD"]);
}
