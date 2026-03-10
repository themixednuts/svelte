use svelte_compiler::{Compiler, ParseMode, ParseOptions};

#[test]
fn modern_parser_keeps_alignment_directives_and_debug_tag() {
    let source = "<div let:x style:color={c} transition:fade={t} animate:flip={a} use:act={u}></div>{@debug x, y}";
    let document = Compiler::new()
        .parse(
            source,
            ParseOptions {
                mode: ParseMode::Modern,
                loose: false,
                ..Default::default()
            },
        )
        .expect("modern parse should succeed");

    let json = serde_json::to_string(&document).expect("serialize modern ast");

    assert!(json.contains("\"type\":\"LetDirective\""));
    assert!(json.contains("\"type\":\"StyleDirective\""));
    assert!(json.contains("\"type\":\"TransitionDirective\""));
    assert!(json.contains("\"type\":\"AnimateDirective\""));
    assert!(json.contains("\"type\":\"UseDirective\""));
    assert!(json.contains("\"type\":\"DebugTag\""));
}

#[test]
fn legacy_parser_keeps_let_directive_and_debug_tag() {
    let source = "<div let:x></div>{@debug x}";
    let document = Compiler::new()
        .parse(
            source,
            ParseOptions {
                mode: ParseMode::Legacy,
                loose: false,
                ..Default::default()
            },
        )
        .expect("legacy parse should succeed");

    let json = serde_json::to_string(&document).expect("serialize legacy ast");

    assert!(json.contains("\"type\":\"Let\""));
    assert!(json.contains("\"type\":\"DebugTag\""));
}
