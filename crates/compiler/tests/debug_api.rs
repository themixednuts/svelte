use oxc_allocator::Allocator;
use oxc_ast_visit::utf8_to_utf16::Utf8ToUtf16;
use oxc_parser::{ParseOptions as OxcParseOptions, Parser};
use oxc_span::SourceType;
use svelte_compiler::{Compiler, ParseMode, ParseOptions, SourceId, SourceText, parse_svelte};

#[path = "support/debug/mod.rs"]
mod debug_support;

use debug_support::load_suite_cases;

#[test]
#[ignore]
fn dump_attribute_empty_case() {
    let source = "<div a=\"\" b={''} c='' d=\"{''}\" ></div>";
    let doc = Compiler::new()
        .parse(
            source,
            ParseOptions {
                mode: ParseMode::Legacy,
                loose: false,
                ..Default::default()
            },
        )
        .expect("parse should succeed");
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&doc).expect("serialize should succeed")
    );
}

#[test]
#[ignore]
fn dump_each_block_case() {
    let source = "{#each animals as animal}\n\t<p>{animal}</p>\n{/each}";
    let doc = Compiler::new()
        .parse(
            source,
            ParseOptions {
                mode: ParseMode::Legacy,
                loose: false,
                ..Default::default()
            },
        )
        .expect("parse should succeed");
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&doc).expect("serialize should succeed")
    );
}

#[test]
#[ignore]
fn dump_attribute_unquoted_case() {
    let source = "<div class=foo></div>\n<a href=/>home</a>\n<a href=/foo>home</a>";
    let doc = Compiler::new()
        .parse(
            source,
            ParseOptions {
                mode: ParseMode::Legacy,
                loose: false,
                ..Default::default()
            },
        )
        .expect("parse should succeed");
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&doc).expect("serialize should succeed")
    );
}

#[test]
#[ignore]
fn dump_raw_mustaches_cst() {
    let source = "<p> {@html raw1} {@html raw2} </p>";
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let cst = parse_svelte(source_text).expect("parse cst should succeed");
    let root = cst.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        eprintln!(
            "root child kind={} start={} end={} text={:?}",
            child.kind(),
            child.start_byte(),
            child.end_byte(),
            child.utf8_text(source.as_bytes()).unwrap_or_default()
        );
        if child.kind() == "element" {
            let mut inner = child.walk();
            for n in child.named_children(&mut inner) {
                eprintln!(
                    "  element child kind={} start={} end={} text={:?}",
                    n.kind(),
                    n.start_byte(),
                    n.end_byte(),
                    n.utf8_text(source.as_bytes()).unwrap_or_default()
                );
            }
        }
    }
}

#[test]
#[ignore]
fn dump_modern_if_block_case() {
    let source = "{#if x > 10}\n\t<p>x is greater than 10</p>\n{:else if x < 5}\n\t<p>x is less than 5</p>\n{/if}";
    let doc = Compiler::new()
        .parse(
            source,
            ParseOptions {
                mode: ParseMode::Modern,
                loose: false,
                ..Default::default()
            },
        )
        .expect("parse should succeed");
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&doc).expect("serialize should succeed")
    );
}

#[test]
#[ignore]
fn dump_modern_if_block_cst() {
    let source = "{#if x > 10}\n\t<p>x is greater than 10</p>\n{:else if x < 5}\n\t<p>x is less than 5</p>\n{/if}";
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let cst = parse_svelte(source_text).expect("parse cst should succeed");

    let root = cst.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        eprintln!(
            "root child kind={} start={} end={} text={:?}",
            child.kind(),
            child.start_byte(),
            child.end_byte(),
            child.utf8_text(source.as_bytes()).unwrap_or_default()
        );

        if child.kind() == "block" {
            let mut block_cursor = child.walk();
            for block_child in child.named_children(&mut block_cursor) {
                eprintln!(
                    "  block child kind={} start={} end={} text={:?}",
                    block_child.kind(),
                    block_child.start_byte(),
                    block_child.end_byte(),
                    block_child.utf8_text(source.as_bytes()).unwrap_or_default()
                );

                if block_child.kind() == "block_start" {
                    let parsed = block_child.child_by_field_name("expression");
                    eprintln!("    parsed start expr? {}", parsed.is_some());
                }
                if block_child.kind() == "block_branch" {
                    let parsed = block_child.child_by_field_name("expression");
                    eprintln!("    parsed branch expr? {}", parsed.is_some());
                }
            }
        }
    }
}

#[test]
#[ignore]
fn dump_oxc_script_estree() {
    let source = "\n\tlet count = $state(0);\n";
    let allocator = Allocator::default();
    let source_type = SourceType::ts();
    let mut parsed = Parser::new(&allocator, source, source_type)
        .with_options(OxcParseOptions {
            parse_regular_expression: true,
            ..OxcParseOptions::default()
        })
        .parse()
        .program;
    Utf8ToUtf16::new(source).convert_program(&mut parsed);
    eprintln!("{}", parsed.to_pretty_estree_ts_json(true));
}

#[test]
#[ignore]
fn dump_modern_root_kinds() {
    let source = "<script lang=\"ts\"></script>\n\n{#snippet foo(msg: string)}\n\t<p>{msg}</p>\n{/snippet}\n\n{@render foo(msg)}";
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let cst = parse_svelte(source_text).expect("parse cst should succeed");
    let root = cst.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        eprintln!(
            "root child kind={} start={} end={} text={:?}",
            child.kind(),
            child.start_byte(),
            child.end_byte(),
            child.utf8_text(source.as_bytes()).unwrap_or_default()
        );

        if child.kind() == "block" || child.kind() == "tag" {
            let mut inner = child.walk();
            for n in child.named_children(&mut inner) {
                eprintln!(
                    "  child kind={} start={} end={} text={:?}",
                    n.kind(),
                    n.start_byte(),
                    n.end_byte(),
                    n.utf8_text(source.as_bytes()).unwrap_or_default()
                );
            }
        }
    }
}

#[test]
#[ignore]
fn dump_attachment_start_tag_kinds() {
    let source = "<div {@attach (node) => {}} {@attach (node) => {}}></div>";
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let cst = parse_svelte(source_text).expect("parse cst should succeed");
    let root = cst.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "element" {
            continue;
        }
        let mut element_cursor = child.walk();
        for element_child in child.named_children(&mut element_cursor) {
            eprintln!(
                "element child kind={} start={} end={} text={:?}",
                element_child.kind(),
                element_child.start_byte(),
                element_child.end_byte(),
                element_child
                    .utf8_text(source.as_bytes())
                    .unwrap_or_default()
            );

            if element_child.kind() == "start_tag" {
                let mut tag_cursor = element_child.walk();
                for tag_child in element_child.named_children(&mut tag_cursor) {
                    eprintln!(
                        "  start_tag child kind={} start={} end={} text={:?}",
                        tag_child.kind(),
                        tag_child.start_byte(),
                        tag_child.end_byte(),
                        tag_child.utf8_text(source.as_bytes()).unwrap_or_default()
                    );

                    if tag_child.kind() == "attribute" {
                        let mut attr_cursor = tag_child.walk();
                        for attr_child in tag_child.named_children(&mut attr_cursor) {
                            eprintln!(
                                "    attribute child kind={} start={} end={} text={:?}",
                                attr_child.kind(),
                                attr_child.start_byte(),
                                attr_child.end_byte(),
                                attr_child.utf8_text(source.as_bytes()).unwrap_or_default()
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
#[ignore]
fn dump_each_block_cst() {
    let source = "{#each arr as [key, value = 'default']}\n\t<div>{key}: {value}</div>\n{/each}";
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let cst = parse_svelte(source_text).expect("parse cst should succeed");
    let root = cst.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        eprintln!(
            "root child kind={} start={} end={} text={:?}",
            child.kind(),
            child.start_byte(),
            child.end_byte(),
            child.utf8_text(source.as_bytes()).unwrap_or_default()
        );
        if child.kind() == "block" {
            let mut inner = child.walk();
            for n in child.named_children(&mut inner) {
                eprintln!(
                    "  child kind={} start={} end={} text={:?}",
                    n.kind(),
                    n.start_byte(),
                    n.end_byte(),
                    n.utf8_text(source.as_bytes()).unwrap_or_default()
                );
                if n.kind() == "block_start" {
                    for field in ["kind", "expression", "binding", "key"] {
                        if let Some(field_node) = n.child_by_field_name(field) {
                            eprintln!(
                                "    field {} kind={} text={:?}",
                                field,
                                field_node.kind(),
                                field_node.utf8_text(source.as_bytes()).unwrap_or_default()
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
#[ignore]
fn dump_await_block_cst() {
    let source =
        "{#await x. then y}{/await}\n{#await x.}{:then y}{/await}\n{#await x.}{:catch e}{/await}";
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let cst = parse_svelte(source_text).expect("parse cst should succeed");
    let root = cst.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        eprintln!(
            "root child kind={} start={} end={} text={:?}",
            child.kind(),
            child.start_byte(),
            child.end_byte(),
            child.utf8_text(source.as_bytes()).unwrap_or_default()
        );
        if child.kind() == "block" {
            let mut inner = child.walk();
            for n in child.named_children(&mut inner) {
                eprintln!(
                    "  child kind={} start={} end={} text={:?}",
                    n.kind(),
                    n.start_byte(),
                    n.end_byte(),
                    n.utf8_text(source.as_bytes()).unwrap_or_default()
                );
                for field in [
                    "kind",
                    "expression",
                    "value",
                    "error",
                    "then",
                    "catch",
                    "binding",
                ] {
                    if let Some(field_node) = n.child_by_field_name(field) {
                        eprintln!(
                            "    field {} kind={} text={:?}",
                            field,
                            field_node.kind(),
                            field_node.utf8_text(source.as_bytes()).unwrap_or_default()
                        );
                    }
                }
            }
        }
    }
}

#[test]
#[ignore]
fn dump_comment_before_function_binding_cst() {
    let source =
        "<input bind:value={\n\t/** ( */\n\t() => value,\n\t(v) => value = v.toLowerCase()\n} />";
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let cst = parse_svelte(source_text).expect("parse cst should succeed");
    let root = cst.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "element" {
            continue;
        }
        let mut ec = child.walk();
        for echild in child.named_children(&mut ec) {
            eprintln!(
                "element child kind={} start={} end={} text={:?}",
                echild.kind(),
                echild.start_byte(),
                echild.end_byte(),
                echild.utf8_text(source.as_bytes()).unwrap_or_default()
            );
            if echild.kind() == "self_closing_tag" || echild.kind() == "start_tag" {
                let mut tc = echild.walk();
                for tchild in echild.named_children(&mut tc) {
                    eprintln!(
                        "  tag child kind={} start={} end={} text={:?}",
                        tchild.kind(),
                        tchild.start_byte(),
                        tchild.end_byte(),
                        tchild.utf8_text(source.as_bytes()).unwrap_or_default()
                    );
                    if tchild.kind() == "attribute" {
                        let mut ac = tchild.walk();
                        for achild in tchild.named_children(&mut ac) {
                            eprintln!(
                                "    attr child kind={} start={} end={} text={:?}",
                                achild.kind(),
                                achild.start_byte(),
                                achild.end_byte(),
                                achild.utf8_text(source.as_bytes()).unwrap_or_default()
                            );
                            if achild.kind() == "expression" {
                                let mut xc = achild.walk();
                                for xchild in achild.named_children(&mut xc) {
                                    eprintln!(
                                        "      expr child kind={} start={} end={} text={:?}",
                                        xchild.kind(),
                                        xchild.start_byte(),
                                        xchild.end_byte(),
                                        xchild.utf8_text(source.as_bytes()).unwrap_or_default()
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
#[ignore]
fn dump_modern_typescript_event_fixture() {
    let source = "<script lang=\"ts\">\n\tlet count = $state(0);\n</script>\n\n<button\n\ton:click={(e: MouseEvent) => {\n\t\tconst next: number = count + 1;\n\t\tcount = next;\n\t}}\n>clicks: {count}</button>\n";
    let doc = Compiler::new()
        .parse(
            source,
            ParseOptions {
                mode: ParseMode::Modern,
                loose: false,
                ..Default::default()
            },
        )
        .expect("parse should succeed");
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&doc).expect("serialize should succeed")
    );
}

#[test]
#[ignore]
fn dump_typescript_event_cst() {
    let source = "<button\n\ton:click={(e: MouseEvent) => {\n\t\tconst next: number = count + 1;\n\t\tcount = next;\n\t}}\n>clicks: {count}</button>\n";
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let cst = parse_svelte(source_text).expect("parse cst should succeed");
    let root = cst.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "element" {
            continue;
        }
        let mut ec = child.walk();
        for echild in child.named_children(&mut ec) {
            if echild.kind() != "start_tag" {
                continue;
            }
            let mut tc = echild.walk();
            for tchild in echild.named_children(&mut tc) {
                eprintln!(
                    "start-tag child kind={} text={:?}",
                    tchild.kind(),
                    tchild.utf8_text(source.as_bytes()).unwrap_or_default()
                );
                if tchild.kind() == "attribute" {
                    let mut ac = tchild.walk();
                    for achild in tchild.named_children(&mut ac) {
                        eprintln!(
                            "  attr child kind={} text={:?}",
                            achild.kind(),
                            achild.utf8_text(source.as_bytes()).unwrap_or_default()
                        );
                        if achild.kind() == "expression" {
                            let mut xc = achild.walk();
                            for x in achild.named_children(&mut xc) {
                                eprintln!(
                                    "    expr child kind={} text={:?}",
                                    x.kind(),
                                    x.utf8_text(source.as_bytes()).unwrap_or_default()
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
#[ignore]
fn dump_modern_options_fixture() {
    let source = "<svelte:options customElement=\"my-custom-element\" runes={true} />\n\n<script module lang=\"ts\">\n</script>\n\n<script lang=\"ts\" generics=\"T extends { foo: number }\">\n</script>\n";
    let doc = Compiler::new()
        .parse(
            source,
            ParseOptions {
                mode: ParseMode::Modern,
                loose: false,
                ..Default::default()
            },
        )
        .expect("parse should succeed");
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&doc).expect("serialize should succeed")
    );
}

#[test]
#[ignore]
fn debug_diff_modern_options_fixture() {
    debug_diff_modern_case("options");
}

#[test]
#[ignore]
fn debug_diff_modern_snippets_fixture() {
    debug_diff_modern_case("snippets");
}

#[test]
#[ignore]
fn debug_diff_modern_each_fixture() {
    debug_diff_modern_case("each-block-object-pattern");
}

#[test]
#[ignore]
fn debug_diff_modern_fixture_from_env() {
    let case_name = std::env::var("MODERN_FIXTURE").expect("set MODERN_FIXTURE env var");
    debug_diff_modern_case(&case_name);
}

#[test]
#[ignore]
fn debug_diff_legacy_fixture_from_env() {
    let case_name = std::env::var("LEGACY_FIXTURE").expect("set LEGACY_FIXTURE env var");
    debug_diff_legacy_case(&case_name);
}

fn debug_diff_modern_case(case_name: &str) {
    let cases = load_suite_cases("parser-modern").expect("discover modern parser cases");
    let case = cases
        .into_iter()
        .find(|fixture| fixture.name == case_name)
        .unwrap_or_else(|| panic!("{case_name} fixture exists"));

    let input = case
        .read_required_text("input.svelte")
        .expect("read input.svelte")
        .replace('\r', "");
    let input = input.trim_end().to_string();
    let expected_text = case
        .read_required_text("output.json")
        .expect("read output.json");

    let actual = Compiler::new()
        .parse(
            &input,
            ParseOptions {
                mode: ParseMode::Modern,
                loose: case_name.starts_with("loose-"),
                ..Default::default()
            },
        )
        .expect("parse should succeed");

    let actual_json = serde_json::to_value(actual).expect("serialize actual");
    let expected_json =
        serde_json::from_str::<serde_json::Value>(&expected_text).expect("parse expected json");

    if actual_json == expected_json {
        eprintln!("{case_name} fixture matches exactly");
        return;
    }

    eprintln!(
        "{case_name} first diff: {}",
        first_json_diff_path(&actual_json, &expected_json)
    );
    eprintln!(
        "actual: {}",
        serde_json::to_string_pretty(&actual_json).expect("serialize actual pretty")
    );
}

fn debug_diff_legacy_case(case_name: &str) {
    let cases = load_suite_cases("parser-legacy").expect("discover legacy parser cases");
    let case = cases
        .into_iter()
        .find(|fixture| fixture.name == case_name)
        .unwrap_or_else(|| panic!("{case_name} fixture exists"));

    let input = case
        .read_required_text("input.svelte")
        .expect("read input.svelte")
        .replace('\r', "");
    let input = input.trim_end().to_string();
    let expected_text = case
        .read_required_text("output.json")
        .expect("read output.json");

    let actual = Compiler::new()
        .parse(
            &input,
            ParseOptions {
                mode: ParseMode::Legacy,
                loose: case_name.starts_with("loose-"),
                ..Default::default()
            },
        )
        .expect("parse should succeed");

    let actual_json = serde_json::to_value(actual).expect("serialize actual");
    let expected_json =
        serde_json::from_str::<serde_json::Value>(&expected_text).expect("parse expected json");

    if actual_json == expected_json {
        eprintln!("{case_name} fixture matches exactly");
        return;
    }

    eprintln!(
        "{case_name} first diff: {}",
        first_json_diff_path(&actual_json, &expected_json)
    );
    eprintln!(
        "actual: {}",
        serde_json::to_string_pretty(&actual_json).expect("serialize actual pretty")
    );
}

#[test]
#[ignore]
fn debug_dump_legacy_cst_case_from_env() {
    let case_name = std::env::var("LEGACY_FIXTURE").expect("set LEGACY_FIXTURE env var");
    let cases = load_suite_cases("parser-legacy").expect("discover legacy parser cases");
    let case = cases
        .into_iter()
        .find(|fixture| fixture.name == case_name)
        .unwrap_or_else(|| panic!("{case_name} fixture exists"));
    let input = case
        .read_required_text("input.svelte")
        .expect("read input.svelte")
        .replace('\r', "");
    let input = input.trim_end().to_string();

    let source_text = SourceText::new(SourceId::new(0), &input, None);
    let cst = parse_svelte(source_text).expect("parse cst should succeed");
    let root = cst.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        eprintln!(
            "root child kind={} start={} end={} text={:?}",
            child.kind(),
            child.start_byte(),
            child.end_byte(),
            child.utf8_text(input.as_bytes()).unwrap_or_default()
        );
        if child.kind() == "block" || child.kind() == "ERROR" || child.kind() == "element" {
            let mut inner = child.walk();
            for n in child.named_children(&mut inner) {
                eprintln!(
                    "  child kind={} start={} end={} text={:?}",
                    n.kind(),
                    n.start_byte(),
                    n.end_byte(),
                    n.utf8_text(input.as_bytes()).unwrap_or_default()
                );
                for field in [
                    "kind",
                    "expression",
                    "expression_value",
                    "binding",
                    "shorthand",
                ] {
                    if let Some(field_node) = n.child_by_field_name(field) {
                        eprintln!(
                            "    field {} kind={} text={:?}",
                            field,
                            field_node.kind(),
                            field_node.utf8_text(input.as_bytes()).unwrap_or_default()
                        );
                    }
                }
            }
        }
    }
}

#[test]
#[ignore]
fn debug_dump_modern_cst_case_from_env() {
    let case_name = std::env::var("MODERN_FIXTURE").expect("set MODERN_FIXTURE env var");
    let cases = load_suite_cases("parser-modern").expect("discover modern parser cases");
    let case = cases
        .into_iter()
        .find(|fixture| fixture.name == case_name)
        .unwrap_or_else(|| panic!("{case_name} fixture exists"));
    let input = case
        .read_required_text("input.svelte")
        .expect("read input.svelte")
        .replace('\r', "");
    let input = input.trim_end().to_string();

    let source_text = SourceText::new(SourceId::new(0), &input, None);
    let cst = parse_svelte(source_text).expect("parse cst should succeed");
    let root = cst.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        eprintln!(
            "root child kind={} start={} end={} text={:?}",
            child.kind(),
            child.start_byte(),
            child.end_byte(),
            child.utf8_text(input.as_bytes()).unwrap_or_default()
        );
        if child.kind() == "element" {
            let mut el_cursor = child.walk();
            for el_child in child.named_children(&mut el_cursor) {
                eprintln!(
                    "  element child kind={} start={} end={} text={:?}",
                    el_child.kind(),
                    el_child.start_byte(),
                    el_child.end_byte(),
                    el_child.utf8_text(input.as_bytes()).unwrap_or_default()
                );
                if el_child.kind() == "ERROR" {
                    let mut err_cursor = el_child.walk();
                    for err_child in el_child.named_children(&mut err_cursor) {
                        eprintln!(
                            "    element ERROR child kind={} start={} end={} text={:?}",
                            err_child.kind(),
                            err_child.start_byte(),
                            err_child.end_byte(),
                            err_child.utf8_text(input.as_bytes()).unwrap_or_default()
                        );
                    }
                }
            }
        }
        if child.kind() == "block" || child.kind() == "ERROR" {
            let mut inner = child.walk();
            for n in child.named_children(&mut inner) {
                eprintln!(
                    "  child kind={} start={} end={} text={:?}",
                    n.kind(),
                    n.start_byte(),
                    n.end_byte(),
                    n.utf8_text(input.as_bytes()).unwrap_or_default()
                );
                if n.kind() == "ERROR" {
                    let mut nested = n.walk();
                    for nn in n.named_children(&mut nested) {
                        eprintln!(
                            "    nested kind={} start={} end={} text={:?}",
                            nn.kind(),
                            nn.start_byte(),
                            nn.end_byte(),
                            nn.utf8_text(input.as_bytes()).unwrap_or_default()
                        );
                    }
                }
                for field in ["kind", "expression", "expression_value", "binding"] {
                    if let Some(field_node) = n.child_by_field_name(field) {
                        eprintln!(
                            "    field {} kind={} text={:?}",
                            field,
                            field_node.kind(),
                            field_node.utf8_text(input.as_bytes()).unwrap_or_default()
                        );
                    }
                }
            }
        }
    }
}

fn first_json_diff_path(actual: &serde_json::Value, expected: &serde_json::Value) -> String {
    if actual == expected {
        return "<equal>".to_string();
    }

    match (actual, expected) {
        (serde_json::Value::Object(a), serde_json::Value::Object(e)) => {
            let mut keys = a.keys().chain(e.keys()).collect::<Vec<_>>();
            keys.sort();
            keys.dedup();
            for key in keys {
                match (a.get(key), e.get(key)) {
                    (Some(av), Some(ev)) => {
                        if av != ev {
                            let child = first_json_diff_path(av, ev);
                            return if child == "<value>" {
                                format!(".{key}")
                            } else {
                                format!(".{key}{child}")
                            };
                        }
                    }
                    _ => return format!(".{key}"),
                }
            }
            "<value>".to_string()
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(e)) => {
            if a.len() != e.len() {
                return "[len]".to_string();
            }
            for (index, (av, ev)) in a.iter().zip(e.iter()).enumerate() {
                if av != ev {
                    let child = first_json_diff_path(av, ev);
                    return if child == "<value>" {
                        format!("[{index}]")
                    } else {
                        format!("[{index}]{child}")
                    };
                }
            }
            "<value>".to_string()
        }
        _ => "<value>".to_string(),
    }
}

#[test]
#[ignore]
fn dump_comment_event_handler_cst() {
    let cases = [
        ("basic-attr", "<div x={a}></div>"),
        ("comment-attr", "<div x={// c\na}></div>"),
        ("basic-directive", "<div on:click={x}></div>"),
        ("comment-directive", "<div on:click={// c\nx}></div>"),
        ("block-comment-expr", "{/* c */ a}"),
        ("line-comment-expr", "{// c\na}"),
    ];

    for (label, source) in cases {
        eprintln!("\n=== {label} ===");
        let source_text = SourceText::new(SourceId::new(0), source, None);
        let cst = parse_svelte(source_text).expect("parse cst should succeed");
        let root = cst.root_node();
        fn dump(source: &str, node: tree_sitter::Node, indent: usize) {
            let prefix = "  ".repeat(indent);
            let text = node.utf8_text(source.as_bytes()).unwrap_or_default();
            let short = if text.len() > 60 { &text[..60] } else { text };
            eprintln!(
                "{prefix}kind={} start={} end={} text={:?}",
                node.kind(),
                node.start_byte(),
                node.end_byte(),
                short
            );
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                dump(source, child, indent + 1);
            }
        }
        dump(source, root, 0);
    }
}
