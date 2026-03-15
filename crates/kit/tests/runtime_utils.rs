use svelte_kit::get_relative_path;

#[test]
fn computes_relative_path_without_node_path_helpers() {
    assert_eq!(
        get_relative_path("src/routes/+page.svelte", "src/lib/utils.ts"),
        "../lib/utils.ts"
    );
    assert_eq!(
        get_relative_path(
            "src/routes/blog/+page.svelte",
            "src/routes/blog/[slug]/+page.svelte"
        ),
        "[slug]/+page.svelte"
    );
    assert_eq!(
        get_relative_path(
            "src\\routes\\blog\\+page.svelte",
            "src\\lib\\server\\thing.ts"
        ),
        "..\\..\\lib\\server\\thing.ts".replace('\\', "/")
    );
}
