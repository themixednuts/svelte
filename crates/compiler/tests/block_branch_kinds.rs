use svelte_compiler::{Compiler, ParseMode, ParseOptions, ast};

fn parse_modern_json(source: &str) -> String {
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

    serde_json::to_string(&document).expect("serialize modern ast")
}

fn parse_legacy_document(source: &str) -> ast::Document {
    Compiler::new()
        .parse(
            source,
            ParseOptions {
                mode: ParseMode::Legacy,
                loose: false,
                ..Default::default()
            },
        )
        .expect("legacy parse should succeed")
}

#[test]
fn modern_if_only_uses_else_and_else_if_as_branches() {
    let json = parse_modern_json("{#if ok}before{:then value}after{:else}fallback{/if}");

    assert!(json.contains("\"type\":\"IfBlock\""));
    assert!(json.contains("\"data\":\"before\""));
    // With typed node grammar, {:then value}after is consumed by an ERROR node
    // and "after" is no longer a separate Text node.
    assert!(json.contains("\"data\":\"fallback\""));
    assert!(!json.contains("\"name\":\"value\""));
}

#[test]
fn modern_each_only_uses_else_as_branch() {
    let json =
        parse_modern_json("{#each items as item}body0{:then value}body1{:else}fallback{/each}");

    assert!(json.contains("\"type\":\"EachBlock\""));
    assert!(json.contains("\"data\":\"body0\""));
    // With typed node grammar, {:then value}body1 is consumed by an ERROR node
    // and "body1" is no longer a separate Text node.
    assert!(json.contains("\"data\":\"fallback\""));
    assert!(!json.contains("\"name\":\"value\""));
}

#[test]
fn modern_await_only_uses_then_and_catch_as_branches() {
    let json = parse_modern_json(
        "{#await promise}pending0{:else}pending1{:then resolved}then_body{/await}",
    );

    assert!(json.contains("\"type\":\"AwaitBlock\""));
    assert!(json.contains("\"data\":\"pending0\""));
    // With typed orphan-branch recovery, an invalid {:else} ends the recovered await block.
    // The remaining text is preserved, but the later {:then resolved} does not become an
    // await branch.
    assert!(json.contains("\"data\":\"pending1\""));
    assert!(json.contains("\"data\":\"then_body\""));
    assert!(!json.contains("\"name\":\"resolved\""));
}

#[test]
fn legacy_shorthand_await_branch_uses_cst_child_span() {
    let source = "{#await promise then value}<p>{value}</p>{/await}";
    let document = parse_legacy_document(source);

    let ast::Root::Legacy(root) = document.root else {
        panic!("expected legacy root");
    };
    let Some(ast::legacy::Node::AwaitBlock(block)) = root.html.children.first() else {
        panic!("expected top-level await block");
    };

    assert!(!block.then.skip);
    assert_eq!(block.then.start, source.find("<p>"));
    assert_eq!(block.then.end, source.find("{/await}"));
}
