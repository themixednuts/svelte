use svelte_kit::create_function_as_string;

#[test]
fn create_dynamic_string_escapes_backslashes() {
    let input = "div:after { content: '\\s'; }";
    let code =
        create_function_as_string("css", &[], input).expect("function string should be created");

    assert_eq!(
        code,
        "function css() { return `div:after { content: '\\\\s'; }`; }"
    );
}
