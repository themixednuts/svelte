use svelte_kit::resolve_app_module;

#[test]
fn app_navigation_is_importable_from_runtime_surface() {
    assert_eq!(
        resolve_app_module("$app/navigation"),
        Some("runtime/app/navigation.js")
    );
}
