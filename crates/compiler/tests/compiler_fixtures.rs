#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::collapsible_if,
    clippy::disallowed_names,
    clippy::explicit_iter_loop,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::match_same_arms,
    clippy::needless_pass_by_value,
    clippy::option_option,
    clippy::redundant_closure_for_method_calls,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::unnecessary_lazy_evaluations,
    clippy::unnecessary_wraps,
    clippy::while_let_on_iterator
)]

use std::{collections::BTreeMap, sync::Arc};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use svelte_compiler::{
    CompileOptions, ErrorMode, FragmentStrategy, GenerateTarget, MigrateOptions, ParseMode,
    ParseOptions, PreprocessAttribute, PreprocessAttributeValue, PreprocessOptions,
    PreprocessOutput, PreprocessResult, PreprocessorGroup, PrintOptions, SourceMap, compile,
    compile_module, migrate, parse, parse_css, preprocess, print,
};
#[path = "support/fixture/mod.rs"]
mod fixture_support;
#[path = "support/repo/mod.rs"]
mod repo_support;

use fixture_support::{
    FixtureCase, discover_suite_cases, discover_suite_cases_by_name, load_test_config,
};
use repo_support::detect_repo_root;

const COMPILER_MAPPED_JS_SUITES: &[&str] = &[
    "compiler-errors",
    "css",
    "migrate",
    "parser-legacy",
    "parser-modern",
    "preprocess",
    "print",
    "snapshot",
    "sourcemaps",
    "validator",
];

const EXPLICITLY_UNPORTED_JS_SUITES: &[&str] = &[
    "hydration",
    "manual",
    "motion",
    "runtime-browser",
    "runtime-legacy",
    "runtime-production",
    "runtime-runes",
    "runtime-xhtml",
    "server-side-rendering",
    "signals",
    "store",
    "types",
];

const UNPORTED_JS_COMPILE_SMOKE_SUITES: &[&str] = &[
    "hydration",
    "runtime-browser",
    "runtime-legacy",
    "runtime-production",
    "runtime-runes",
    "runtime-xhtml",
    "server-side-rendering",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum FixtureJson {
    Object(BTreeMap<String, FixtureJson>),
    Array(Vec<FixtureJson>),
    String(String),
    Number(serde_json::Number),
    Bool(bool),
    Null,
}

fn to_fixture_json<T: Serialize>(value: &T) -> FixtureJson {
    let encoded = serde_json::to_vec(value).expect("serialize to json");
    serde_json::from_slice::<FixtureJson>(&encoded).expect("deserialize into fixture json")
}

#[test]
fn parser_modern_suite_ported() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "parser-modern").expect("discover parser-modern");

    let mut failures = Vec::new();

    for case in cases {
        let input = normalize_source(
            case.read_required_text("input.svelte")
                .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name)),
        );
        let expected_text = case
            .read_required_text("output.json")
            .unwrap_or_else(|err| panic!("{} missing output.json: {err}", case.name));

        let options = ParseOptions {
            mode: ParseMode::Modern,
            loose: case.name.starts_with("loose-"),
            ..Default::default()
        };

        match parse(&input, options.clone()) {
            Ok(ast) => {
                let actual_json = to_fixture_json(&ast);

                match serde_json::from_str::<FixtureJson>(&expected_text) {
                    Ok(expected_json) => {
                        if actual_json != expected_json {
                            if case.name.starts_with("loose-") {
                                let actual_str =
                                    serde_json::to_string_pretty(&actual_json).unwrap();
                                std::fs::write(case.path.join("_actual.json"), &actual_str).ok();
                                eprintln!(
                                    "=== {} modern mismatch, wrote _actual.json ===",
                                    case.name
                                );
                            }
                            failures.push(format!(
                                "{}: modern parse output mismatch against output.json",
                                case.name
                            ));
                        }
                    }
                    Err(err) => failures.push(format!("{}: invalid output.json: {err}", case.name)),
                }

                if !options.loose {
                    match print(
                        &ast,
                        PrintOptions {
                            preserve_whitespace: true,
                            ..Default::default()
                        },
                    ) {
                        Ok(printed) => match parse(&printed.code, options.clone()) {
                            Ok(reparsed) => {
                                if actual_json != to_fixture_json(&reparsed) {
                                    failures.push(format!(
                                        "{}: parse->print->parse roundtrip mismatch",
                                        case.name
                                    ));
                                }
                            }
                            Err(err) => failures.push(format!(
                                "{}: reparsing printed output failed: {err}",
                                case.name
                            )),
                        },
                        Err(err) => failures.push(format!("{}: print failed: {err}", case.name)),
                    }
                }
            }
            Err(err) => failures.push(format!("{}: parse failed: {err}", case.name)),
        }
    }

    assert_no_failures("parser-modern", failures);
}

#[test]
fn parser_legacy_suite_ported() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "parser-legacy").expect("discover parser-legacy");

    let mut failures = Vec::new();

    for case in cases {
        if should_skip_case(&case, &mut failures) {
            continue;
        }
        let input = normalize_source(
            case.read_required_text("input.svelte")
                .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name)),
        );
        let expected_text = case
            .read_required_text("output.json")
            .unwrap_or_else(|err| panic!("{} missing output.json: {err}", case.name));

        let options = ParseOptions {
            mode: ParseMode::Legacy,
            loose: case.name.starts_with("loose-"),
            ..Default::default()
        };

        match parse(&input, options) {
            Ok(ast) => {
                let actual_json = to_fixture_json(&ast);

                match serde_json::from_str::<FixtureJson>(&expected_text) {
                    Ok(expected_json) => {
                        if actual_json != expected_json {
                            let actual_str = serde_json::to_string_pretty(&actual_json).unwrap();
                            std::fs::write(case.path.join("_actual_legacy.json"), &actual_str).ok();
                            eprintln!(
                                "=== {} legacy mismatch, wrote _actual_legacy.json ===",
                                case.name
                            );
                            failures.push(format!(
                                "{}: legacy parse output mismatch against output.json",
                                case.name
                            ));
                        }
                    }
                    Err(err) => failures.push(format!("{}: invalid output.json: {err}", case.name)),
                }
            }
            Err(err) => failures.push(format!("{}: parse failed: {err}", case.name)),
        }
    }

    assert_no_failures("parser-legacy", failures);
}

#[test]
#[ignore = "debug helper; run explicitly"]
fn debug_single_parser_legacy_fixture() {
    let fixture_name = std::env::var("SVELTE_FIXTURE").expect("set SVELTE_FIXTURE=<fixture-name>");
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "parser-legacy").expect("discover parser-legacy");
    let case = cases
        .into_iter()
        .find(|case| case.name == fixture_name)
        .unwrap_or_else(|| panic!("fixture not found: {fixture_name}"));

    let input = normalize_source(
        case.read_required_text("input.svelte")
            .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name)),
    );
    let expected_text = case
        .read_required_text("output.json")
        .unwrap_or_else(|err| panic!("{} missing output.json: {err}", case.name));

    let options = ParseOptions {
        mode: ParseMode::Legacy,
        loose: case.name.starts_with("loose-"),
        ..Default::default()
    };

    let actual = parse(&input, options).expect("parse failed");
    let actual_json = to_fixture_json(&actual);
    let expected_json =
        serde_json::from_str::<FixtureJson>(&expected_text).expect("invalid output.json");

    if actual_json != expected_json {
        println!(
            "=== EXPECTED ({}) ===\n{}",
            case.name,
            serde_json::to_string_pretty(&expected_json).expect("serialize expected json")
        );
        println!(
            "=== ACTUAL ({}) ===\n{}",
            case.name,
            serde_json::to_string_pretty(&actual_json).expect("serialize actual json")
        );
        panic!("fixture mismatch");
    }
}

#[test]
#[ignore = "debug helper; run explicitly"]
fn debug_single_parser_modern_fixture() {
    let fixture_name = std::env::var("SVELTE_FIXTURE").expect("set SVELTE_FIXTURE=<fixture-name>");
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "parser-modern").expect("discover parser-modern");
    let case = cases
        .into_iter()
        .find(|case| case.name == fixture_name)
        .unwrap_or_else(|| panic!("fixture not found: {fixture_name}"));

    let input = normalize_source(
        case.read_required_text("input.svelte")
            .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name)),
    );
    let expected_text = case
        .read_required_text("output.json")
        .unwrap_or_else(|err| panic!("{} missing output.json: {err}", case.name));

    let options = ParseOptions {
        mode: ParseMode::Modern,
        loose: case.name.starts_with("loose-"),
        ..Default::default()
    };

    let actual = parse(&input, options).expect("parse failed");
    let actual_json = to_fixture_json(&actual);
    let expected_json =
        serde_json::from_str::<FixtureJson>(&expected_text).expect("invalid output.json");

    if actual_json != expected_json {
        println!(
            "=== EXPECTED ({}) ===\n{}",
            case.name,
            serde_json::to_string_pretty(&expected_json).expect("serialize expected json")
        );
        println!(
            "=== ACTUAL ({}) ===\n{}",
            case.name,
            serde_json::to_string_pretty(&actual_json).expect("serialize actual json")
        );
        panic!("fixture mismatch");
    }
}

#[test]
#[ignore = "debug helper; run explicitly"]
fn debug_single_print_fixture() {
    let fixture_name = std::env::var("SVELTE_FIXTURE").expect("set SVELTE_FIXTURE=<fixture-name>");
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "print").expect("discover print");
    let case = cases
        .into_iter()
        .find(|case| case.name == fixture_name)
        .unwrap_or_else(|| panic!("fixture not found: {fixture_name}"));

    let input = case
        .read_required_text("input.svelte")
        .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name));
    let expected = case
        .read_required_text("output.svelte")
        .unwrap_or_else(|err| panic!("{} missing output.svelte: {err}", case.name));

    let ast = parse(
        &input,
        ParseOptions {
            mode: ParseMode::Modern,
            loose: false,
            ..Default::default()
        },
    )
    .expect("parse failed");
    let ast_json = serde_json::to_string_pretty(&ast).expect("serialize ast");
    let actual = print(&ast, PrintOptions::default()).expect("print failed");

    println!("=== INPUT ===\n{}", normalize_newlines(&input));
    println!("=== AST ===\n{}", ast_json);
    println!("=== EXPECTED ===\n{}", normalize_newlines(&expected));
    println!("=== ACTUAL ===\n{}", normalize_newlines(&actual.code));
}

#[test]
#[ignore = "debug helper; run explicitly"]
fn debug_single_parser_modern_roundtrip_fixture() {
    let fixture_name = std::env::var("SVELTE_FIXTURE").expect("set SVELTE_FIXTURE=<fixture-name>");
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "parser-modern").expect("discover parser-modern");
    let case = cases
        .into_iter()
        .find(|case| case.name == fixture_name)
        .unwrap_or_else(|| panic!("fixture not found: {fixture_name}"));

    let input = normalize_source(
        case.read_required_text("input.svelte")
            .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name)),
    );

    let ast = parse(
        &input,
        ParseOptions {
            mode: ParseMode::Modern,
            loose: false,
            ..Default::default()
        },
    )
    .expect("parse failed");
    let printed = print(&ast, PrintOptions::default()).expect("print failed");
    let reparsed = parse(
        &printed.code,
        ParseOptions {
            mode: ParseMode::Modern,
            loose: false,
            ..Default::default()
        },
    )
    .expect("reparse failed");

    println!("=== INPUT ===\n{}", normalize_newlines(&input));
    println!("=== PRINTED ===\n{}", normalize_newlines(&printed.code));
    println!(
        "=== AST ROOT ===\n{}",
        serde_json::to_string_pretty(&ast.root).expect("json")
    );
    println!(
        "=== REPARSED AST ROOT ===\n{}",
        serde_json::to_string_pretty(&reparsed.root).expect("json")
    );
}

#[test]
fn compiler_errors_suite_ported() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases =
        discover_suite_cases(&repo_root, "compiler-errors").expect("discover compiler-errors");

    let mut failures = Vec::new();

    for case in cases {
        let config_source = case
            .read_required_text("_config.js")
            .unwrap_or_else(|err| panic!("{} missing _config.js: {err}", case.name));

        let expected = parse_compiler_error_config(&config_source)
            .unwrap_or_else(|err| panic!("{} invalid _config.js error config: {err}", case.name));

        let mut had_entry = false;

        if case.has_file("main.svelte") {
            had_entry = true;
            run_expected_error_compile(
                &case,
                "main.svelte",
                expected.async_mode,
                &expected,
                &mut failures,
            );
        }

        if case.has_file("main.svelte.js") {
            had_entry = true;
            run_expected_error_compile_module(&case, "main.svelte.js", &expected, &mut failures);
        }

        if !had_entry {
            failures.push(format!(
                "{}: missing main.svelte or main.svelte.js fixture input",
                case.name
            ));
        }
    }

    assert_no_failures("compiler-errors", failures);
}

#[test]
fn migrate_self_closing_elements_fixture_ported() {
    assert_migrate_fixture("self-closing-elements");
}

#[test]
fn migrate_svelte_ignore_fixture_ported() {
    assert_migrate_fixture("svelte-ignore");
}

#[test]
fn migrate_svelte_element_fixture_ported() {
    assert_migrate_fixture("svelte-element");
}

#[test]
fn migrate_script_context_module_fixture_ported() {
    assert_migrate_fixture("script-context-module");
}

#[test]
fn migrate_remove_blocks_whitespace_fixture_ported() {
    assert_migrate_fixture("remove-blocks-whitespace");
}

#[test]
fn migrate_svelte_self_skip_filename_fixture_ported() {
    assert_migrate_fixture("svelte-self-skip-filename");
}

#[test]
fn migrate_svelte_self_fixture_ported() {
    assert_migrate_fixture("svelte-self");
}

#[test]
fn migrate_svelte_self_name_conflict_fixture_ported() {
    assert_migrate_fixture("svelte-self-name-conflict");
}

#[test]
fn migrate_impossible_before_after_update_fixture_ported() {
    assert_migrate_fixture("impossible-migrate-beforeUpdate-afterUpdate");
}

#[test]
fn migrate_impossible_prop_non_identifier_fixture_ported() {
    assert_migrate_fixture("impossible-migrate-prop-non-identifier");
}

#[test]
fn migrate_impossible_prop_and_dollar_props_fixture_ported() {
    assert_migrate_fixture("impossible-migrate-prop-and-$$props");
}

#[test]
fn migrate_impossible_slot_change_name_fixture_ported() {
    assert_migrate_fixture("impossible-migrate-slot-change-name");
}

#[test]
fn migrate_impossible_slot_non_identifier_fixture_ported() {
    assert_migrate_fixture("impossible-migrate-slot-non-identifier");
}

#[test]
fn migrate_impossible_derived_slot_name_conflict_fixture_ported() {
    assert_migrate_fixture("impossible-migrate-$derived-derived-var-3");
}

#[test]
fn migrate_impossible_bindable_name_conflict_fixture_ported() {
    assert_migrate_fixture("impossible-migrate-$bindable-bindable-var-1");
}

#[test]
fn migrate_impossible_props_name_conflict_fixture_ported() {
    assert_migrate_fixture("impossible-migrate-$props-props-var-1");
}

#[test]
fn migrate_impossible_state_name_conflict_fixture_ported() {
    assert_migrate_fixture("impossible-migrate-$state-state-var-1");
    assert_migrate_fixture("impossible-migrate-$state-state-var-2");
    assert_migrate_fixture("impossible-migrate-$state-state-var-3");
}

#[test]
fn migrate_impossible_derived_name_conflict_fixture_ported() {
    assert_migrate_fixture("impossible-migrate-$derived-derived-var-1");
    assert_migrate_fixture("impossible-migrate-$derived-derived-var-2");
    assert_migrate_fixture("impossible-migrate-$derived-derived-var-4");
}

#[test]
fn migrate_impossible_parse_error_fixture_ported() {
    assert_migrate_fixture("impossible-migrate-with-errors");
}

#[test]
fn migrate_css_ignore_fixture_ported() {
    assert_migrate_fixture("css-ignore");
}

#[test]
fn migrate_not_prepend_props_to_export_let_fixture_ported() {
    assert_migrate_fixture("not-prepend-props-to-export-let");
}

#[test]
fn migrate_not_blank_css_if_error_fixture_ported() {
    assert_migrate_fixture("not-blank-css-if-error");
}

#[test]
fn migrate_import_type_dollar_prefix_fixture_ported() {
    assert_migrate_fixture("import-type-$-prefix");
}

#[test]
fn migrate_props_fixture_ported() {
    assert_migrate_fixture("props");
}

#[test]
fn migrate_props_rest_props_fixture_ported() {
    assert_migrate_fixture("props-rest-props");
}

#[test]
fn migrate_props_and_labeled_fixture_ported() {
    assert_migrate_fixture("props-and-labeled");
}

#[test]
fn migrate_props_export_alias_fixture_ported() {
    assert_migrate_fixture("props-export-alias");
}

#[test]
fn migrate_props_interface_fixture_ported() {
    assert_migrate_fixture("props-interface");
}

#[test]
fn migrate_props_rest_props_ts_fixture_ported() {
    assert_migrate_fixture("props-rest-props-ts");
}

#[test]
fn migrate_props_rest_props_jsdoc_fixture_ported() {
    assert_migrate_fixture("props-rest-props-jsdoc");
}

#[test]
fn migrate_unused_before_after_update_fixture_ported() {
    assert_migrate_fixture("unused-beforeUpdate-afterUpdate");
}

#[test]
fn migrate_unused_before_after_update_extra_imports_fixture_ported() {
    assert_migrate_fixture("unused-beforeUpdate-afterUpdate-extra-imports");
}

#[test]
fn migrate_state_ts_fixture_ported() {
    assert_migrate_fixture("state-ts");
}

#[test]
fn migrate_state_no_initial_fixture_ported() {
    assert_migrate_fixture("state-no-initial");
}

#[test]
fn migrate_labeled_statement_reassign_state_fixture_ported() {
    assert_migrate_fixture("labeled-statement-reassign-state");
}

#[test]
fn migrate_single_assignment_labeled_fixture_ported() {
    assert_migrate_fixture("single-assignment-labeled");
}

#[test]
fn migrate_export_props_multiple_declarations_fixture_ported() {
    assert_migrate_fixture("export-props-multiple-declarations");
}

#[test]
fn migrate_derivations_fixture_ported() {
    assert_migrate_fixture("derivations");
}

#[test]
fn migrate_reassigned_deriveds_fixture_ported() {
    assert_migrate_fixture("reassigned-deriveds");
}

#[test]
fn migrate_state_and_derivations_sequence_fixture_ported() {
    assert_migrate_fixture("state-and-derivations-sequence");
}

#[test]
fn migrate_named_slots_fixture_ported() {
    assert_migrate_fixture("named-slots");
}

#[test]
fn migrate_slots_fixture_ported() {
    assert_migrate_fixture("slots");
}

#[test]
fn migrate_slot_use_ts_fixture_ported() {
    assert_migrate_fixture("slot-use_ts");
}

#[test]
fn migrate_slot_use_ts_2_fixture_ported() {
    assert_migrate_fixture("slot-use_ts-2");
}

#[test]
fn migrate_slot_use_ts_3_fixture_ported() {
    assert_migrate_fixture("slot-use_ts-3");
}

#[test]
fn migrate_effects_fixture_ported() {
    assert_migrate_fixture("effects");
}

#[test]
fn migrate_effects_with_alias_run_fixture_ported() {
    assert_migrate_fixture("effects-with-alias-run");
}

#[test]
fn migrate_slots_below_imports_fixture_ported() {
    assert_migrate_fixture("slots-below-imports");
}

#[test]
fn migrate_slots_multiple_fixture_ported() {
    assert_migrate_fixture("slots-multiple");
}

#[test]
fn migrate_slots_with_dollar_props_fixture_ported() {
    assert_migrate_fixture("slots-with-$$props");
}

#[test]
fn migrate_self_closing_named_slot_fixture_ported() {
    assert_migrate_fixture("self-closing-named-slot");
}

#[test]
fn migrate_slots_custom_element_fixture_ported() {
    assert_migrate_fixture("slots-custom-element");
}

#[test]
fn migrate_accessors_fixture_ported() {
    assert_migrate_fixture("accessors");
}

#[test]
fn migrate_slots_used_as_variable_fixture_ported() {
    assert_migrate_fixture("$$slots-used-as-variable");
}

#[test]
fn migrate_slots_used_as_variable_dollar_props_fixture_ported() {
    assert_migrate_fixture("$$slots-used-as-variable-$$props");
}

#[test]
fn migrate_each_block_const_fixture_ported() {
    assert_migrate_fixture("each-block-const");
}

#[test]
fn migrate_slot_shadow_props_fixture_ported() {
    assert_migrate_fixture("slot-shadow-props");
}

#[test]
fn migrate_slot_non_identifier_fixture_ported() {
    assert_migrate_fixture("slot-non-identifier");
}

#[test]
fn migrate_slot_dont_mess_with_attributes_fixture_ported() {
    assert_migrate_fixture("slot-dont-mess-with-attributes");
}

#[test]
fn migrate_reactive_statements_inner_block_fixture_ported() {
    assert_migrate_fixture("reactive-statements-inner-block");
}

#[test]
fn migrate_event_handlers_fixture_ported() {
    assert_migrate_fixture("event-handlers");
}

#[test]
fn migrate_event_handlers_with_alias_fixture_ported() {
    assert_migrate_fixture("event-handlers-with-alias");
}

#[test]
fn migrate_is_not_where_has_fixture_ported() {
    assert_migrate_fixture("is-not-where-has");
}

#[test]
fn migrate_reactive_statements_reorder_1_fixture_ported() {
    assert_migrate_fixture("reactive-statements-reorder-1");
}

#[test]
fn migrate_reactive_statements_reorder_2_fixture_ported() {
    assert_migrate_fixture("reactive-statements-reorder-2");
}

#[test]
fn migrate_reactive_statements_reorder_not_deleting_additions_fixture_ported() {
    assert_migrate_fixture("reactive-statements-reorder-not-deleting-additions");
}

#[test]
fn migrate_jsdoc_with_comments_fixture_ported() {
    assert_migrate_fixture("jsdoc-with-comments");
}

#[test]
fn migrate_props_ts_fixture_ported() {
    assert_migrate_fixture("props-ts");
}

#[test]
fn migrate_reactive_statements_reorder_with_comments_fixture_ported() {
    assert_migrate_fixture("reactive-statements-reorder-with-comments");
}

#[test]
fn migrate_shadowed_forwarded_slot_fixture_ported() {
    assert_migrate_fixture("shadowed-forwarded-slot");
}

#[test]
fn migrate_slot_usages_fixture_ported() {
    assert_migrate_fixture("slot-usages");
}

#[test]
fn migrate_svelte_component_fixture_ported() {
    assert_migrate_fixture("svelte-component");
}

#[test]
fn migrate_suite_ported() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "migrate").expect("discover migrate suite");

    for case in cases {
        assert_migrate_fixture(&case.name);
    }
}

#[test]
fn preprocess_suite_ported() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "preprocess").expect("discover preprocess suite");

    for case in cases {
        assert_preprocess_fixture(&case.name);
    }
}

#[test]
fn preprocess_markup_fixture_ported() {
    assert_preprocess_fixture("markup");
}

#[test]
fn preprocess_style_fixture_ported() {
    assert_preprocess_fixture("style");
}

#[test]
fn preprocess_style_async_fixture_ported() {
    assert_preprocess_fixture("style-async");
}

#[test]
fn preprocess_script_fixture_ported() {
    assert_preprocess_fixture("script");
}

#[test]
fn preprocess_script_multiple_fixture_ported() {
    assert_preprocess_fixture("script-multiple");
}

#[test]
fn preprocess_script_self_closing_fixture_ported() {
    assert_preprocess_fixture("script-self-closing");
}

#[test]
fn preprocess_style_self_closing_fixture_ported() {
    assert_preprocess_fixture("style-self-closing");
}

#[test]
fn preprocess_dependencies_fixture_ported() {
    assert_preprocess_fixture("dependencies");
}

#[test]
fn preprocess_filename_fixture_ported() {
    assert_preprocess_fixture("filename");
}

#[test]
fn preprocess_style_attributes_fixture_ported() {
    assert_preprocess_fixture("style-attributes");
}

#[test]
fn preprocess_style_attributes_modified_fixture_ported() {
    assert_preprocess_fixture("style-attributes-modified");
}

#[test]
fn preprocess_style_attributes_modified_longer_fixture_ported() {
    assert_preprocess_fixture("style-attributes-modified-longer");
}

#[test]
fn preprocess_multiple_preprocessors_fixture_ported() {
    assert_preprocess_fixture("multiple-preprocessors");
}

#[test]
fn preprocess_comments_fixture_ported() {
    assert_preprocess_fixture("comments");
}

#[test]
fn preprocess_partial_names_fixture_ported() {
    assert_preprocess_fixture("partial-names");
}

#[test]
fn preprocess_ignores_null_fixture_ported() {
    assert_preprocess_fixture("ignores-null");
}

#[test]
fn preprocess_attributes_with_closing_tag_fixture_ported() {
    assert_preprocess_fixture("attributes-with-closing-tag");
}

#[test]
fn preprocess_attributes_with_equals_fixture_ported() {
    assert_preprocess_fixture("attributes-with-equals");
}

#[test]
fn preprocess_empty_sourcemap_fixture_ported() {
    assert_preprocess_fixture("empty-sourcemap");
}

fn assert_preprocess_fixture(fixture_name: &str) {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases =
        discover_suite_cases_by_name(&repo_root, "preprocess").expect("discover preprocess");
    let case = cases
        .into_iter()
        .find(|case| case.name == fixture_name)
        .unwrap_or_else(|| panic!("{fixture_name} fixture exists"));

    let input = case
        .read_required_text("input.svelte")
        .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name))
        .replace("\r\n", "\n");
    let expected = case
        .read_required_text("output.svelte")
        .unwrap_or_else(|err| panic!("{} missing output.svelte: {err}", case.name))
        .replace("\r\n", "\n");

    let actual = preprocess(&input, preprocess_fixture_options(fixture_name))
        .unwrap_or_else(|err| panic!("{} preprocess failed: {err}", case.name));

    assert_eq!(actual.code.as_ref(), expected, "{}", fixture_name);
    assert_eq!(
        actual.dependencies.as_ref(),
        preprocess_fixture_dependencies(fixture_name).as_slice(),
        "{} dependencies",
        fixture_name
    );
    assert_eq!(
        actual.map,
        preprocess_fixture_map(fixture_name),
        "{} map",
        fixture_name
    );
}

fn preprocess_fixture_options(fixture_name: &str) -> PreprocessOptions {
    PreprocessOptions {
        filename: preprocess_fixture_filename(fixture_name),
        groups: preprocess_fixture_groups(fixture_name).into_boxed_slice(),
    }
}

fn preprocess_fixture_filename(fixture_name: &str) -> Option<camino::Utf8PathBuf> {
    match fixture_name {
        "filename" => Some(camino::Utf8PathBuf::from("file.svelte")),
        _ => Some(camino::Utf8PathBuf::from("input.svelte")),
    }
}

fn preprocess_fixture_groups(fixture_name: &str) -> Vec<PreprocessorGroup> {
    match fixture_name {
        "markup" => vec![PreprocessorGroup {
            markup: Some(Arc::new(|markup| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from(markup.content.replace("__NAME__", "world")),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "style" => vec![PreprocessorGroup {
            style: Some(Arc::new(|style| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from(style.content.replace("$brand", "purple")),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "style-async" => vec![PreprocessorGroup {
            style_async: Some(Arc::new(|style| {
                Box::pin(async move {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(style.content.replace("$brand", "purple")),
                        ..PreprocessOutput::default()
                    }))
                })
            })),
            ..PreprocessorGroup::default()
        }],
        "script" => vec![PreprocessorGroup {
            script: Some(Arc::new(|script| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from(script.content.replace("__THE_ANSWER__", "42")),
                    map: preprocess_fixture_map("script"),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "script-multiple" => vec![PreprocessorGroup {
            script: Some(Arc::new(|script| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from(script.content.to_lowercase()),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "script-self-closing" => vec![PreprocessorGroup {
            script: Some(Arc::new(|script| {
                let answer = string_attribute(script.attributes, "the-answer").unwrap_or_default();
                Ok(Some(PreprocessOutput {
                    code: Arc::from(format!("console.log(\"{answer}\");")),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "style-self-closing" => vec![PreprocessorGroup {
            style: Some(Arc::new(|style| {
                let color = string_attribute(style.attributes, "color").unwrap_or_default();
                Ok(Some(PreprocessOutput {
                    code: Arc::from(format!("div {{ color: {color}; }}")),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "dependencies" => vec![PreprocessorGroup {
            style: Some(Arc::new(|style| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from(
                        style
                            .content
                            .replace("@import './foo.css';", "/* removed */"),
                    ),
                    dependencies: vec![camino::Utf8PathBuf::from("./foo.css")].into_boxed_slice(),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "filename" => vec![PreprocessorGroup {
            markup: Some(Arc::new(|markup| {
                let filename = markup
                    .filename
                    .map(|filename| filename.as_str())
                    .unwrap_or_default();
                Ok(Some(PreprocessOutput {
                    code: Arc::from(markup.content.replace("__MARKUP_FILENAME__", filename)),
                    ..PreprocessOutput::default()
                }))
            })),
            style: Some(Arc::new(|style| {
                let filename = style
                    .filename
                    .map(|filename| filename.as_str())
                    .unwrap_or_default();
                Ok(Some(PreprocessOutput {
                    code: Arc::from(style.content.replace("__STYLE_FILENAME__", filename)),
                    ..PreprocessOutput::default()
                }))
            })),
            script: Some(Arc::new(|script| {
                let filename = script
                    .filename
                    .map(|filename| filename.as_str())
                    .unwrap_or_default();
                Ok(Some(PreprocessOutput {
                    code: Arc::from(script.content.replace("__SCRIPT_FILENAME__", filename)),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "style-attributes" => vec![PreprocessorGroup {
            style: Some(Arc::new(|_| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from("PROCESSED"),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "style-attributes-modified" => vec![PreprocessorGroup {
            style: Some(Arc::new(|_| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from("PROCESSED"),
                    attributes: Some(
                        vec![PreprocessAttribute {
                            name: Arc::from("sth"),
                            value: PreprocessAttributeValue::String(Arc::from("else")),
                        }]
                        .into_boxed_slice(),
                    ),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "style-attributes-modified-longer" => vec![PreprocessorGroup {
            style: Some(Arc::new(|_| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from("PROCESSED"),
                    attributes: Some(
                        vec![PreprocessAttribute {
                            name: Arc::from("sth"),
                            value: PreprocessAttributeValue::String(Arc::from(
                                "wayyyyyyyyyyyyy looooooonger",
                            )),
                        }]
                        .into_boxed_slice(),
                    ),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "multiple-preprocessors" => vec![
            PreprocessorGroup {
                markup: Some(Arc::new(|markup| {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(markup.content.replace("one", "two")),
                        ..PreprocessOutput::default()
                    }))
                })),
                script: Some(Arc::new(|script| {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(script.content.replace("two", "three")),
                        ..PreprocessOutput::default()
                    }))
                })),
                style: Some(Arc::new(|style| {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(style.content.replace("three", "style")),
                        ..PreprocessOutput::default()
                    }))
                })),
                ..PreprocessorGroup::default()
            },
            PreprocessorGroup {
                markup: Some(Arc::new(|markup| {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(markup.content.replace("two", "three")),
                        ..PreprocessOutput::default()
                    }))
                })),
                script: Some(Arc::new(|script| {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(script.content.replace("three", "script")),
                        ..PreprocessOutput::default()
                    }))
                })),
                style: Some(Arc::new(|style| {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(style.content.replace("three", "style")),
                        ..PreprocessOutput::default()
                    }))
                })),
                ..PreprocessorGroup::default()
            },
        ],
        "comments" => vec![PreprocessorGroup {
            script: Some(Arc::new(|script| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from(script.content.replace("one", "two")),
                    ..PreprocessOutput::default()
                }))
            })),
            style: Some(Arc::new(|style| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from(style.content.replace("one", "three")),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "partial-names" => vec![PreprocessorGroup {
            script: Some(Arc::new(|_| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from(""),
                    ..PreprocessOutput::default()
                }))
            })),
            style: Some(Arc::new(|_| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from(""),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        "ignores-null" => vec![PreprocessorGroup {
            script: Some(Arc::new(|_| Ok(None))),
            ..PreprocessorGroup::default()
        }],
        "attributes-with-closing-tag" => vec![PreprocessorGroup {
            script: Some(Arc::new(|script| {
                let generics = string_attribute(script.attributes, "generics").unwrap_or_default();
                if generics.contains('>') {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(""),
                        ..PreprocessOutput::default()
                    }))
                } else {
                    Ok(None)
                }
            })),
            ..PreprocessorGroup::default()
        }],
        "attributes-with-equals" => vec![PreprocessorGroup {
            style: Some(Arc::new(|style| {
                let value = string_attribute(style.attributes, "foo").unwrap_or_default();
                if value.contains('=') {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(""),
                        ..PreprocessOutput::default()
                    }))
                } else {
                    Ok(None)
                }
            })),
            ..PreprocessorGroup::default()
        }],
        "empty-sourcemap" => vec![PreprocessorGroup {
            style: Some(Arc::new(|style| {
                Ok(Some(PreprocessOutput {
                    code: Arc::from(style.content),
                    map: preprocess_fixture_map("empty-sourcemap"),
                    ..PreprocessOutput::default()
                }))
            })),
            ..PreprocessorGroup::default()
        }],
        other => panic!("unported preprocess fixture: {other}"),
    }
}

fn preprocess_fixture_dependencies(fixture_name: &str) -> Vec<camino::Utf8PathBuf> {
    match fixture_name {
        "dependencies" => vec![camino::Utf8PathBuf::from("./foo.css")],
        _ => Vec::new(),
    }
}

fn preprocess_fixture_map(fixture_name: &str) -> Option<SourceMap> {
    match fixture_name {
        "script" => Some(SourceMap {
            version: 3,
            file: None,
            source_root: None,
            sources: vec![Arc::from("input.svelte")].into_boxed_slice(),
            sources_content: None,
            names: Vec::<Arc<str>>::new().into_boxed_slice(),
            mappings: Arc::from(
                "AAAA,CAAC,MAAM;AACP,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,EAAc,CAAC;AAC5B,CAAC,CAAC,MAAM",
            ),
        }),
        "empty-sourcemap" => Some(SourceMap {
            mappings: Arc::from(""),
            ..SourceMap::default()
        }),
        _ => None,
    }
}

fn string_attribute<'a>(
    attributes: &'a BTreeMap<Arc<str>, PreprocessAttributeValue>,
    name: &str,
) -> Option<&'a str> {
    match attributes.get(name) {
        Some(PreprocessAttributeValue::String(value)) => Some(value.as_ref()),
        _ => None,
    }
}

fn assert_migrate_fixture(fixture_name: &str) {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases_by_name(&repo_root, "migrate").expect("discover migrate");
    let case = cases
        .into_iter()
        .find(|case| case.name == fixture_name)
        .unwrap_or_else(|| panic!("{fixture_name} fixture exists"));
    let config = load_test_config::<FixtureConfigJson>(&case)
        .unwrap_or_else(|err| panic!("{} load _config.js: {err}", case.name))
        .unwrap_or_default();

    let input = case
        .read_required_text("input.svelte")
        .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name))
        .replace("\r\n", "\n")
        .trim_end()
        .to_string();
    let expected = case
        .read_required_text("output.svelte")
        .unwrap_or_else(|err| panic!("{} missing output.svelte: {err}", case.name))
        .replace("\r\n", "\n");

    let actual = migrate(
        &input,
        MigrateOptions {
            filename: (!config.skip_filename).then(|| camino::Utf8PathBuf::from("output.svelte")),
            use_ts: config.use_ts,
        },
    )
    .expect("migrate fixture should succeed");

    assert_eq!(actual.code.trim(), expected.trim());
}

#[test]
#[ignore = "debug helper; run explicitly"]
fn debug_single_migrate_fixture() {
    let fixture_name = std::env::var("SVELTE_FIXTURE").expect("set SVELTE_FIXTURE=<fixture-name>");
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases_by_name(&repo_root, "migrate").expect("discover migrate");
    let case = cases
        .into_iter()
        .find(|case| case.name == fixture_name)
        .unwrap_or_else(|| panic!("fixture not found: {fixture_name}"));
    let config = load_test_config::<FixtureConfigJson>(&case)
        .unwrap_or_else(|err| panic!("{} load _config.js: {err}", case.name))
        .unwrap_or_default();

    let input = case
        .read_required_text("input.svelte")
        .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name))
        .replace("\r\n", "\n")
        .trim_end()
        .to_string();
    let expected = case
        .read_required_text("output.svelte")
        .unwrap_or_else(|err| panic!("{} missing output.svelte: {err}", case.name))
        .replace("\r\n", "\n");

    println!("=== FIXTURE {} ===", case.name);
    println!("=== INPUT ===\n{}", normalize_newlines(&input));
    println!("=== EXPECTED ===\n{}", normalize_newlines(expected.trim()));

    match migrate(
        &input,
        MigrateOptions {
            filename: (!config.skip_filename).then(|| camino::Utf8PathBuf::from("output.svelte")),
            use_ts: config.use_ts,
        },
    ) {
        Ok(result) => {
            println!("=== ACTUAL ===\n{}", normalize_newlines(result.code.trim()));
        }
        Err(error) => {
            println!("=== ERROR ===\n{error}");
        }
    }
}

#[test]
fn css_suite_ported() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "css").expect("discover css suite");

    let mut failures = Vec::new();

    for case in cases {
        let input = normalize_source(
            case.read_required_text("input.svelte")
                .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name)),
        );
        let expected_css = case
            .read_required_text("expected.css")
            .unwrap_or_else(|err| panic!("{} missing expected.css: {err}", case.name));

        let config_js = case
            .read_optional_text("_config.js")
            .unwrap_or_else(|err| panic!("{} read _config.js: {err}", case.name));
        let dev_override = config_js
            .as_deref()
            .and_then(|cfg| parse_bool_field(cfg, "dev"))
            .unwrap_or(false);
        let css_hash_override = config_js
            .as_deref()
            .and_then(|cfg| {
                if cfg.contains("cssHash(") {
                    extract_css_hash_from_expected_css(&expected_css)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| Arc::from("svelte-xyz"));

        let client_options = CompileOptions {
            filename: None,
            generate: GenerateTarget::Client,
            fragments: FragmentStrategy::Html,
            dev: dev_override,
            error_mode: ErrorMode::Error,
            css_hash: Some(css_hash_override.clone()),
            ..CompileOptions::default()
        };
        let server_options = CompileOptions {
            filename: None,
            generate: GenerateTarget::Server,
            fragments: FragmentStrategy::Html,
            dev: dev_override,
            error_mode: ErrorMode::Error,
            css_hash: Some(css_hash_override),
            ..CompileOptions::default()
        };

        let client_result = compile(&input, client_options);
        let server_result = compile(&input, server_options);

        match (client_result, server_result) {
            (Ok(client), Ok(server)) => {
                let client_css = client.css.map(|artifact| artifact.code).unwrap_or_default();
                let server_css = server.css.map(|artifact| artifact.code).unwrap_or_default();

                if client_css.trim() != server_css.trim() {
                    failures.push(format!("{}: css mismatch between client/server", case.name));
                }

                if normalize_newlines(client_css.trim()) != normalize_newlines(expected_css.trim())
                {
                    failures.push(format!("{}: css mismatch against expected.css", case.name));
                }

                if case.has_file("expected.html") && client.js.code.trim().is_empty() {
                    failures.push(format!(
                        "{}: expected.html present but client js output is empty",
                        case.name
                    ));
                }
            }
            (Err(client_err), Err(server_err)) => {
                failures.push(format!(
                    "{}: compile client/server failed: client={client_err}; server={server_err}",
                    case.name
                ));
            }
            (Err(client_err), Ok(_)) => failures.push(format!(
                "{}: compile client failed: {client_err}",
                case.name
            )),
            (Ok(_), Err(server_err)) => failures.push(format!(
                "{}: compile server failed: {server_err}",
                case.name
            )),
        }
    }

    assert_no_failures("css", failures);
}

#[test]
#[ignore = "debug helper; run explicitly"]
fn debug_single_css_fixture() {
    let fixture_name = std::env::var("SVELTE_FIXTURE").expect("set SVELTE_FIXTURE=<fixture-name>");
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "css").expect("discover css suite");
    let case = cases
        .into_iter()
        .find(|case| case.name == fixture_name)
        .unwrap_or_else(|| panic!("fixture not found: {fixture_name}"));

    let input = normalize_source(
        case.read_required_text("input.svelte")
            .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name)),
    );
    let expected_css = case
        .read_required_text("expected.css")
        .unwrap_or_else(|err| panic!("{} missing expected.css: {err}", case.name));

    let config_js = case
        .read_optional_text("_config.js")
        .unwrap_or_else(|err| panic!("{} read _config.js: {err}", case.name));
    let dev_override = config_js
        .as_deref()
        .and_then(|cfg| parse_bool_field(cfg, "dev"))
        .unwrap_or(false);
    let css_hash_override = config_js
        .as_deref()
        .and_then(|cfg| {
            if cfg.contains("cssHash(") {
                extract_css_hash_from_expected_css(&expected_css)
            } else {
                None
            }
        })
        .unwrap_or_else(|| Arc::from("svelte-xyz"));

    let options = CompileOptions {
        filename: None,
        generate: GenerateTarget::Client,
        fragments: FragmentStrategy::Html,
        dev: dev_override,
        error_mode: ErrorMode::Error,
        css_hash: Some(css_hash_override),
        ..CompileOptions::default()
    };

    let actual = compile(&input, options)
        .expect("compile failed")
        .css
        .map(|artifact| artifact.code)
        .unwrap_or_default();

    println!("=== INPUT ===\n{}", normalize_newlines(&input));
    println!(
        "=== EXPECTED CSS ===\n{}",
        normalize_newlines(expected_css.trim())
    );
    println!("=== ACTUAL CSS ===\n{}", normalize_newlines(actual.trim()));
}

fn run_expected_error_compile(
    case: &FixtureCase,
    input_file: &str,
    async_mode: bool,
    expected: &ExpectedCompilerError,
    failures: &mut Vec<String>,
) {
    let source = case
        .read_required_text(input_file)
        .unwrap_or_else(|err| panic!("{} missing {input_file}: {err}", case.name));

    let options = CompileOptions {
        filename: None,
        generate: GenerateTarget::Client,
        fragments: FragmentStrategy::Html,
        error_mode: ErrorMode::Error,
        experimental: svelte_compiler::ExperimentalOptions {
            r#async: async_mode,
        },
        ..CompileOptions::default()
    };

    match compile(&source, options) {
        Ok(_) => failures.push(format!("{}: expected compile() to fail", case.name)),
        Err(err) => {
            if err.code.as_ref() != expected.code {
                failures.push(format!(
                    "{}: error code mismatch (actual={}, expected={})",
                    case.name, err.code, expected.code
                ));
            }
            if strip_doc_link(&err.message) != expected.message {
                failures.push(format!(
                    "{}: error message mismatch (actual={:?}, expected={:?})",
                    case.name,
                    strip_doc_link(&err.message),
                    expected.message
                ));
            }
            if let Some((expected_start, expected_end)) = expected.position {
                match err.position {
                    Some(actual) => {
                        if actual.start != expected_start || actual.end != expected_end {
                            failures.push(format!(
                                "{}: error position mismatch (actual=[{}, {}], expected=[{}, {}])",
                                case.name, actual.start, actual.end, expected_start, expected_end
                            ));
                        }
                    }
                    None => failures.push(format!(
                        "{}: missing error position, expected [{}, {}]",
                        case.name, expected_start, expected_end
                    )),
                }
            }
        }
    }
}

fn run_expected_error_compile_module(
    case: &FixtureCase,
    input_file: &str,
    expected: &ExpectedCompilerError,
    failures: &mut Vec<String>,
) {
    let source = case
        .read_required_text(input_file)
        .unwrap_or_else(|err| panic!("{} missing {input_file}: {err}", case.name));

    let options = CompileOptions {
        filename: None,
        generate: GenerateTarget::Client,
        fragments: FragmentStrategy::Html,
        error_mode: ErrorMode::Error,
        ..CompileOptions::default()
    };

    match compile_module(&source, options) {
        Ok(_) => failures.push(format!("{}: expected compile_module() to fail", case.name)),
        Err(err) => {
            if err.code.as_ref() != expected.code {
                failures.push(format!(
                    "{}: module error code mismatch (actual={}, expected={})",
                    case.name, err.code, expected.code
                ));
            }
            if strip_doc_link(&err.message) != expected.message {
                failures.push(format!(
                    "{}: module error message mismatch (actual={:?}, expected={:?})",
                    case.name,
                    strip_doc_link(&err.message),
                    expected.message
                ));
            }
            if let Some((expected_start, expected_end)) = expected.position {
                match err.position {
                    Some(actual) => {
                        if actual.start != expected_start || actual.end != expected_end {
                            failures.push(format!(
                                "{}: module error position mismatch (actual=[{}, {}], expected=[{}, {}])",
                                case.name, actual.start, actual.end, expected_start, expected_end
                            ));
                        }
                    }
                    None => failures.push(format!(
                        "{}: missing module error position, expected [{}, {}]",
                        case.name, expected_start, expected_end
                    )),
                }
            }
        }
    }
}

fn assert_no_failures(suite_name: &str, failures: Vec<String>) {
    if failures.is_empty() {
        return;
    }

    let mut message = format!("{} fixture parity failures: {}", suite_name, failures.len());

    let max_lines = failures.len();
    for failure in failures.into_iter().take(max_lines) {
        message.push('\n');
        message.push_str(" - ");
        message.push_str(&failure);
    }

    panic!("{message}");
}

fn normalize_source(source: String) -> String {
    source.replace('\r', "").trim_end().to_string()
}

fn normalize_newlines(source: &str) -> String {
    source.replace("\r\n", "\n")
}

/// Normalize print output for comparison. OXC codegen strips blank lines and
/// uses single quotes — cosmetic differences we don't treat as failures.
fn normalize_print_output(source: &str) -> String {
    source
        .replace("\r\n", "\n")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.replace('\'', "\""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_doc_link(message: &str) -> String {
    let mut lines: Vec<&str> = message.lines().collect();
    if let Some(last_line) = lines.last() {
        if last_line.starts_with("https://svelte.dev/e/") {
            lines.pop();
        }
    }
    lines.join("\n")
}

fn extract_css_hash_from_expected_css(expected_css: &str) -> Option<Arc<str>> {
    let mut chars = expected_css.char_indices();
    while let Some((index, ch)) = chars.next() {
        if ch != '.' {
            continue;
        }
        let mut end = index + 1;
        while end < expected_css.len() {
            let next = expected_css.as_bytes()[end] as char;
            if next.is_ascii_alphanumeric() || next == '-' || next == '_' {
                end += 1;
            } else {
                break;
            }
        }
        if end > index + 1 {
            return expected_css.get(index + 1..end).map(Arc::from);
        }
    }
    None
}

#[derive(Debug, Clone)]
struct ExpectedCompilerError {
    code: String,
    message: String,
    position: Option<(usize, usize)>,
    async_mode: bool,
}

fn parse_compiler_error_config(source: &str) -> Result<ExpectedCompilerError, String> {
    let code = parse_js_string_field(source, "code")
        .ok_or_else(|| "missing error.code in _config.js".to_string())?;
    let message = parse_js_string_field(source, "message")
        .ok_or_else(|| "missing error.message in _config.js".to_string())?;
    let position = parse_js_position_field(source, "position");
    let async_mode = parse_bool_field(source, "async").unwrap_or(false);

    Ok(ExpectedCompilerError {
        code,
        message,
        position,
        async_mode,
    })
}

fn parse_bool_field(source: &str, field_name: &str) -> Option<bool> {
    let needle = format!("{field_name}:");
    let start = source.find(&needle)? + needle.len();
    let remainder = source.get(start..)?.trim_start();
    if remainder.starts_with("true") {
        return Some(true);
    }
    if remainder.starts_with("false") {
        return Some(false);
    }
    None
}

fn parse_js_position_field(source: &str, field_name: &str) -> Option<(usize, usize)> {
    let needle = format!("{field_name}:");
    let start = source.find(&needle)? + needle.len();
    let remainder = source.get(start..)?.trim_start();

    if !remainder.starts_with('[') {
        return None;
    }

    let after_open = &remainder[1..];
    let inside = after_open.split_once(']')?.0;
    let mut parts = inside.split(',');
    let first = parts.next()?.trim().parse::<usize>().ok()?;
    let second = parts.next()?.trim().parse::<usize>().ok()?;

    Some((first, second))
}

fn parse_js_string_field(source: &str, field_name: &str) -> Option<String> {
    let needle = format!("{field_name}:");
    let field_offset = source.find(&needle)? + needle.len();
    let value_start = source.get(field_offset..)?.trim_start();

    let mut chars = value_start.char_indices();
    let (_, quote_char) = chars.next()?;
    if quote_char != '\'' && quote_char != '"' && quote_char != '`' {
        return None;
    }

    let mut escaped = false;
    let mut output = String::new();
    let mut unicode_pending = 0;
    let mut unicode_digits = String::new();

    for (_, c) in chars {
        if unicode_pending > 0 {
            unicode_digits.push(c);
            unicode_pending -= 1;
            if unicode_pending == 0 {
                if let Ok(value) = u32::from_str_radix(&unicode_digits, 16) {
                    if let Some(decoded) = char::from_u32(value) {
                        output.push(decoded);
                    }
                }
                unicode_digits.clear();
            }
            continue;
        }

        if escaped {
            match c {
                'n' => output.push('\n'),
                'r' => output.push('\r'),
                't' => output.push('\t'),
                'u' => unicode_pending = 4,
                other => output.push(other),
            }
            escaped = false;
            continue;
        }

        if c == '\\' {
            escaped = true;
            continue;
        }

        if c == quote_char {
            return Some(output);
        }

        output.push(c);
    }

    None
}

#[test]
fn print_suite_ported() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "print").expect("discover print suite");

    let mut failures = Vec::new();

    for case in cases {
        if should_skip_case(&case, &mut failures) {
            continue;
        }

        let input = case
            .read_required_text("input.svelte")
            .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name));
        let expected = case
            .read_optional_text("output.svelte")
            .unwrap_or_else(|err| panic!("{} read output.svelte: {err}", case.name))
            .unwrap_or_default();

        match parse(
            &input,
            ParseOptions {
                mode: ParseMode::Modern,
                loose: false,
                ..Default::default()
            },
        ) {
            Ok(ast) => match print(&ast, PrintOptions::default()) {
                Ok(output) => {
                    let output_code = if output.code.ends_with('\n') {
                        output.code.to_string()
                    } else {
                        format!("{}\n", output.code)
                    };

                    if normalize_print_output(output_code.trim())
                        != normalize_print_output(expected.trim())
                    {
                        failures.push(format!(
                            "{}: print output mismatch against output.svelte",
                            case.name
                        ));
                    }
                }
                Err(err) => failures.push(format!("{}: print failed: {err}", case.name)),
            },
            Err(err) => failures.push(format!("{}: parse failed: {err}", case.name)),
        }
    }

    assert_no_failures("print", failures);
}

#[test]
fn validator_suite_ported() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "validator").expect("discover validator suite");

    let mut failures = Vec::new();

    for case in cases {
        if should_skip_case(&case, &mut failures) {
            continue;
        }

        let expected_warnings =
            read_optional_json::<Vec<FixtureWarningJson>>(&case, "warnings.json")
                .unwrap_or_else(|err| panic!("{} read warnings.json: {err}", case.name))
                .unwrap_or_default();
        let expected_error = read_optional_json::<Vec<FixtureErrorJson>>(&case, "errors.json")
            .unwrap_or_else(|err| panic!("{} read errors.json: {err}", case.name))
            .unwrap_or_default()
            .into_iter()
            .next();
        let options_json = read_optional_json::<FixtureCompileOptionsJson>(&case, "options.json")
            .unwrap_or_else(|err| panic!("{} read options.json: {err}", case.name));

        let config = load_test_config::<FixtureConfigJson>(&case)
            .unwrap_or_else(|err| panic!("{} load _config.js: {err}", case.name));
        let compile_options =
            compile_options_from_config(config.as_ref(), options_json.as_ref(), true);

        let module = case.has_file("input.svelte.js");
        let input_file = if module {
            "input.svelte.js"
        } else {
            "input.svelte"
        };
        let input = normalize_source(
            case.read_required_text(input_file)
                .unwrap_or_else(|err| panic!("{} missing {input_file}: {err}", case.name)),
        );

        let result = if module {
            compile_module(
                &input,
                CompileOptions {
                    filename: Some(case.path.join(input_file)),
                    ..compile_options
                },
            )
        } else {
            compile(
                &input,
                CompileOptions {
                    filename: Some(case.path.join(input_file)),
                    ..compile_options
                },
            )
        };

        match result {
            Ok(output) => {
                if let Some(expected_error) = expected_error.as_ref() {
                    failures.push(format!(
                        "{}: expected error but compile succeeded: {}",
                        case.name, expected_error.message
                    ));
                    continue;
                }

                let actual_warning_entries = output
                    .warnings
                    .iter()
                    .map(normalize_warning)
                    .collect::<Vec<_>>();

                if actual_warning_entries.len() != expected_warnings.len() {
                    failures.push(format!(
                        "{}: warning count mismatch (actual={}, expected={})\n  actual: {:#?}\n  expected: {:#?}",
                        case.name,
                        actual_warning_entries.len(),
                        expected_warnings.len(),
                        actual_warning_entries,
                        expected_warnings,
                    ));
                    continue;
                }

                for (index, actual) in actual_warning_entries.iter().enumerate() {
                    let Some(expected) = expected_warnings.get(index) else {
                        break;
                    };
                    if actual != expected {
                        failures.push(format!(
                            "{}: warning mismatch at index {}\n  actual: {:#?}\n  expected: {:#?}",
                            case.name, index, actual, expected
                        ));
                        break;
                    }
                }
            }
            Err(error) => {
                let Some(expected) = expected_error.as_ref() else {
                    failures.push(format!("{}: unexpected compile error: {error}", case.name));
                    continue;
                };

                if error.code.as_ref() != expected.code {
                    failures.push(format!(
                        "{}: error code mismatch (actual={}, expected={})",
                        case.name, error.code, expected.code
                    ));
                }

                if strip_doc_link(&error.message) != expected.message {
                    failures.push(format!(
                        "{}: error message mismatch (actual={:?}, expected={:?})",
                        case.name,
                        strip_doc_link(&error.message),
                        expected.message
                    ));
                }

                let actual_start = error.start.as_ref().map(|loc| (loc.line, loc.column));
                let expected_start = location_tuple(expected.start.as_ref());
                if expected_start.is_some() && actual_start != expected_start {
                    failures.push(format!(
                        "{}: error start mismatch (actual={:?}, expected={:?})",
                        case.name, actual_start, expected_start
                    ));
                }

                let actual_end = error.end.as_ref().map(|loc| (loc.line, loc.column));
                let expected_end = location_tuple(expected.end.as_ref());
                if expected_end.is_some() && actual_end != expected_end {
                    failures.push(format!(
                        "{}: error end mismatch (actual={:?}, expected={:?})",
                        case.name, actual_end, expected_end
                    ));
                }
            }
        }
    }

    assert_no_failures("validator", failures);
}

#[test]
fn validator_suite_ported_smoke() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "validator").expect("discover validator suite");

    let mut failures = Vec::new();

    for case in cases {
        if should_skip_case(&case, &mut failures) {
            continue;
        }

        let expected_error = read_optional_json::<Vec<FixtureErrorJson>>(&case, "errors.json")
            .unwrap_or_else(|err| panic!("{} read errors.json: {err}", case.name))
            .unwrap_or_default()
            .into_iter()
            .next();
        let options_json = read_optional_json::<FixtureCompileOptionsJson>(&case, "options.json")
            .unwrap_or_else(|err| panic!("{} read options.json: {err}", case.name));
        let config = load_test_config::<FixtureConfigJson>(&case)
            .unwrap_or_else(|err| panic!("{} load _config.js: {err}", case.name));
        let compile_options =
            compile_options_from_config(config.as_ref(), options_json.as_ref(), true);

        let module = case.has_file("input.svelte.js");
        let input_file = if module {
            "input.svelte.js"
        } else {
            "input.svelte"
        };
        let input = normalize_source(
            case.read_required_text(input_file)
                .unwrap_or_else(|err| panic!("{} missing {input_file}: {err}", case.name)),
        );

        let result = if module {
            compile_module(
                &input,
                CompileOptions {
                    filename: Some(case.path.join(input_file)),
                    ..compile_options
                },
            )
        } else {
            compile(
                &input,
                CompileOptions {
                    filename: Some(case.path.join(input_file)),
                    ..compile_options
                },
            )
        };

        match result {
            Ok(_) => {
                if let Some(expected) = expected_error.as_ref() {
                    failures.push(format!(
                        "{}: expected compile error but succeeded: {}",
                        case.name, expected.message
                    ));
                }
            }
            Err(error) => {
                let Some(expected) = expected_error.as_ref() else {
                    failures.push(format!("{}: unexpected compile error: {error}", case.name));
                    continue;
                };

                if error.code.as_ref() != expected.code {
                    failures.push(format!(
                        "{}: error code mismatch (actual={}, expected={})",
                        case.name, error.code, expected.code
                    ));
                }
            }
        }
    }

    assert_no_failures("validator-smoke", failures);
}

#[test]
#[ignore = "debug helper; run explicitly"]
fn debug_single_validator_fixture() {
    let fixture_name = std::env::var("SVELTE_FIXTURE").expect("set SVELTE_FIXTURE=<fixture-name>");
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let mut cases =
        discover_suite_cases(&repo_root, "validator").expect("discover validator suite");
    cases.retain(|case| case.name == fixture_name);
    let case = cases
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("fixture not found: {fixture_name}"));

    let expected_warnings = read_optional_json::<Vec<FixtureWarningJson>>(&case, "warnings.json")
        .unwrap_or_else(|err| panic!("{} read warnings.json: {err}", case.name))
        .unwrap_or_default();
    let options_json = read_optional_json::<FixtureCompileOptionsJson>(&case, "options.json")
        .unwrap_or_else(|err| panic!("{} read options.json: {err}", case.name));
    let config = load_test_config::<FixtureConfigJson>(&case)
        .unwrap_or_else(|err| panic!("{} load _config.js: {err}", case.name));
    let compile_options = compile_options_from_config(config.as_ref(), options_json.as_ref(), true);

    let module = case.has_file("input.svelte.js");
    let input_file = if module {
        "input.svelte.js"
    } else {
        "input.svelte"
    };
    let input = normalize_source(
        case.read_required_text(input_file)
            .unwrap_or_else(|err| panic!("{} missing {input_file}: {err}", case.name)),
    );

    let output = if module {
        compile_module(&input, compile_options).expect("compile_module failed")
    } else {
        compile(&input, compile_options).expect("compile failed")
    };

    let actual = output
        .warnings
        .iter()
        .map(normalize_warning)
        .collect::<Vec<_>>();

    println!(
        "=== EXPECTED WARNINGS ({}) ===\n{}",
        case.name,
        serde_json::to_string_pretty(&expected_warnings).unwrap()
    );
    println!(
        "=== ACTUAL WARNINGS ({}) ===\n{}",
        case.name,
        serde_json::to_string_pretty(&actual).unwrap()
    );
}

#[test]
fn snapshot_suite_ported() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "snapshot").expect("discover snapshot suite");

    let mut failures = Vec::new();

    for case in cases {
        if should_skip_case(&case, &mut failures) {
            continue;
        }

        let config = load_test_config::<FixtureConfigJson>(&case)
            .unwrap_or_else(|err| panic!("{} load _config.js: {err}", case.name));
        let options = compile_options_from_config(config.as_ref(), None, false);

        let fixture_files = recursive_fixture_files(&case)
            .unwrap_or_else(|err| panic!("{} enumerate fixture files: {err}", case.name));

        let mut had_compilation_target = false;

        for relative in fixture_files {
            if relative.starts_with('_') {
                continue;
            }

            if relative.ends_with(".svelte")
                && !relative.ends_with(".server.svelte")
                && !relative.ends_with(".client.svelte")
            {
                had_compilation_target = true;
                let source = normalize_source(
                    case.read_required_text(&relative)
                        .unwrap_or_else(|err| panic!("{} read {}: {err}", case.name, relative)),
                );

                let client = CompileOptions {
                    generate: GenerateTarget::Client,
                    ..options.clone()
                };
                let server = CompileOptions {
                    generate: GenerateTarget::Server,
                    ..options.clone()
                };

                if let Err(error) = compile(&source, client) {
                    failures.push(format!(
                        "{}: client compile failed for {}: {}",
                        case.name, relative, error
                    ));
                }
                if let Err(error) = compile(&source, server) {
                    failures.push(format!(
                        "{}: server compile failed for {}: {}",
                        case.name, relative, error
                    ));
                }
            }

            if relative.ends_with(".svelte.js") {
                had_compilation_target = true;
                let source = normalize_source(
                    case.read_required_text(&relative)
                        .unwrap_or_else(|err| panic!("{} read {}: {err}", case.name, relative)),
                );

                let client = CompileOptions {
                    generate: GenerateTarget::Client,
                    ..options.clone()
                };
                let server = CompileOptions {
                    generate: GenerateTarget::Server,
                    ..options.clone()
                };

                if let Err(error) = compile_module(&source, client) {
                    failures.push(format!(
                        "{}: client compile_module failed for {}: {}",
                        case.name, relative, error
                    ));
                }
                if let Err(error) = compile_module(&source, server) {
                    failures.push(format!(
                        "{}: server compile_module failed for {}: {}",
                        case.name, relative, error
                    ));
                }
            }
        }

        if !had_compilation_target {
            failures.push(format!(
                "{}: no .svelte or .svelte.js files discovered in snapshot fixture",
                case.name
            ));
        }
    }

    assert_no_failures("snapshot", failures);
}

#[test]
fn snapshot_js_suite_ported() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let mut cases = discover_suite_cases(&repo_root, "snapshot").expect("discover snapshot suite");

    if let Ok(fixture_name) = std::env::var("SVELTE_FIXTURE") {
        cases.retain(|case| case.name == fixture_name);
    }

    let mut failures = Vec::new();

    for case in cases {
        if should_skip_case(&case, &mut failures) {
            continue;
        }

        let config = load_test_config::<FixtureConfigJson>(&case)
            .unwrap_or_else(|err| panic!("{} load _config.js: {err}", case.name));
        let options = compile_options_from_config(config.as_ref(), None, false);
        let fixture_files = recursive_fixture_files(&case)
            .unwrap_or_else(|err| panic!("{} enumerate fixture files: {err}", case.name));

        for relative in fixture_files {
            if relative.starts_with('_') {
                continue;
            }

            if relative.ends_with(".svelte")
                && !relative.ends_with(".server.svelte")
                && !relative.ends_with(".client.svelte")
            {
                let source = normalize_source(
                    case.read_required_text(&relative)
                        .unwrap_or_else(|err| panic!("{} read {}: {err}", case.name, relative)),
                );

                let client_expected_path = format!("_expected/client/{relative}.js");
                let server_expected_path = format!("_expected/server/{relative}.js");
                assert_snapshot_js_output(
                    &case,
                    &relative,
                    "client",
                    &client_expected_path,
                    compile(
                        &source,
                        CompileOptions {
                            filename: Some(case.path.join(&relative)),
                            generate: GenerateTarget::Client,
                            ..options.clone()
                        },
                    ),
                    &mut failures,
                );
                assert_snapshot_js_output(
                    &case,
                    &relative,
                    "server",
                    &server_expected_path,
                    compile(
                        &source,
                        CompileOptions {
                            filename: Some(case.path.join(&relative)),
                            generate: GenerateTarget::Server,
                            ..options.clone()
                        },
                    ),
                    &mut failures,
                );
            }

            if relative.ends_with(".svelte.js") {
                let source = normalize_source(
                    case.read_required_text(&relative)
                        .unwrap_or_else(|err| panic!("{} read {}: {err}", case.name, relative)),
                );

                let client_expected_path = format!("_expected/client/{relative}");
                let server_expected_path = format!("_expected/server/{relative}");
                assert_snapshot_js_output(
                    &case,
                    &relative,
                    "client",
                    &client_expected_path,
                    compile_module(
                        &source,
                        CompileOptions {
                            filename: Some(case.path.join(&relative)),
                            generate: GenerateTarget::Client,
                            ..options.clone()
                        },
                    ),
                    &mut failures,
                );
                assert_snapshot_js_output(
                    &case,
                    &relative,
                    "server",
                    &server_expected_path,
                    compile_module(
                        &source,
                        CompileOptions {
                            filename: Some(case.path.join(&relative)),
                            generate: GenerateTarget::Server,
                            ..options.clone()
                        },
                    ),
                    &mut failures,
                );
            }
        }
    }

    assert_no_failures("snapshot-js", failures);
}

#[test]
fn js_test_suite_inventory_is_accounted_for() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let tests_root = repo_root.join("packages").join("svelte").join("tests");

    let mut actual_dirs = std::fs::read_dir(&tests_root)
        .unwrap_or_else(|err| panic!("read tests dir {}: {err}", tests_root))
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_dir() {
                return None;
            }
            Some(entry.file_name().to_string_lossy().to_string())
        })
        .collect::<Vec<_>>();
    actual_dirs.sort();

    let mut accounted = COMPILER_MAPPED_JS_SUITES
        .iter()
        .chain(EXPLICITLY_UNPORTED_JS_SUITES.iter())
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    accounted.sort();

    let unaccounted = actual_dirs
        .iter()
        .filter(|name| !accounted.iter().any(|known| known == *name))
        .cloned()
        .collect::<Vec<_>>();

    assert!(
        unaccounted.is_empty(),
        "unaccounted JS test suites in packages/svelte/tests: {}",
        unaccounted.join(", ")
    );
}

#[test]
fn js_unported_suites_parse_smoke() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let mut failures = Vec::new();

    for suite in EXPLICITLY_UNPORTED_JS_SUITES {
        let cases = match discover_suite_cases_by_name(&repo_root, suite) {
            Ok(cases) => cases,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("discover {suite} suite: {error}"),
        };

        for case in cases {
            let fixture_files = recursive_fixture_files(&case)
                .unwrap_or_else(|err| panic!("{} enumerate fixture files: {err}", case.name));

            for relative in fixture_files {
                if relative.starts_with('_') || !relative.ends_with(".svelte") {
                    continue;
                }

                let source = normalize_source(
                    case.read_required_text(&relative)
                        .unwrap_or_else(|err| panic!("{} read {}: {err}", case.name, relative)),
                );

                if let Err(error) = parse(
                    &source,
                    ParseOptions {
                        mode: ParseMode::Modern,
                        loose: false,
                        ..Default::default()
                    },
                ) {
                    failures.push(format!(
                        "{} (suite={}): parse failed for {}: {}",
                        case.name, suite, relative, error
                    ));
                }
            }
        }
    }

    assert_no_failures("js-unported-parse-smoke", failures);
}

#[test]
fn js_unported_suites_compile_smoke() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let mut failures = Vec::new();

    for suite in UNPORTED_JS_COMPILE_SMOKE_SUITES {
        let cases = match discover_suite_cases_by_name(&repo_root, suite) {
            Ok(cases) => cases,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("discover {suite} suite: {error}"),
        };

        for case in cases {
            if should_skip_case(&case, &mut failures) {
                continue;
            }

            let config = match load_test_config::<FixtureConfigJson>(&case) {
                Ok(config) => config,
                Err(error) => {
                    failures.push(format!(
                        "{} (suite={}): load _config.js failed: {}",
                        case.name, suite, error
                    ));
                    continue;
                }
            };
            let options = compile_options_from_config(config.as_ref(), None, false);
            let fixture_files = match recursive_fixture_files(&case) {
                Ok(files) => files,
                Err(error) => {
                    failures.push(format!(
                        "{} (suite={}): enumerate fixture files failed: {}",
                        case.name, suite, error
                    ));
                    continue;
                }
            };

            for relative in fixture_files {
                if relative.starts_with('_') {
                    continue;
                }

                if relative.ends_with(".svelte.js") {
                    let source = match case.read_required_text(&relative) {
                        Ok(text) => normalize_source(text),
                        Err(error) => {
                            failures.push(format!(
                                "{} (suite={}): read {} failed: {}",
                                case.name, suite, relative, error
                            ));
                            continue;
                        }
                    };

                    let client_result = compile_module(
                        &source,
                        CompileOptions {
                            filename: Some(case.path.join(&relative)),
                            generate: GenerateTarget::Client,
                            ..options.clone()
                        },
                    );
                    if let Err(error) = client_result {
                        failures.push(format!(
                            "{} (suite={}): client compile_module failed for {}: {}",
                            case.name, suite, relative, error
                        ));
                    }

                    let server_result = compile_module(
                        &source,
                        CompileOptions {
                            filename: Some(case.path.join(&relative)),
                            generate: GenerateTarget::Server,
                            ..options.clone()
                        },
                    );
                    if let Err(error) = server_result {
                        failures.push(format!(
                            "{} (suite={}): server compile_module failed for {}: {}",
                            case.name, suite, relative, error
                        ));
                    }

                    continue;
                }

                if !relative.ends_with(".svelte") {
                    continue;
                }

                let source = match case.read_required_text(&relative) {
                    Ok(text) => normalize_source(text),
                    Err(error) => {
                        failures.push(format!(
                            "{} (suite={}): read {} failed: {}",
                            case.name, suite, relative, error
                        ));
                        continue;
                    }
                };

                let compile_client = !relative.ends_with(".server.svelte");
                let compile_server = !relative.ends_with(".client.svelte");

                if compile_client {
                    let client_result = compile(
                        &source,
                        CompileOptions {
                            filename: Some(case.path.join(&relative)),
                            generate: GenerateTarget::Client,
                            ..options.clone()
                        },
                    );
                    if let Err(error) = client_result {
                        failures.push(format!(
                            "{} (suite={}): client compile failed for {}: {}",
                            case.name, suite, relative, error
                        ));
                    }
                }

                if compile_server {
                    let server_result = compile(
                        &source,
                        CompileOptions {
                            filename: Some(case.path.join(&relative)),
                            generate: GenerateTarget::Server,
                            ..options.clone()
                        },
                    );
                    if let Err(error) = server_result {
                        failures.push(format!(
                            "{} (suite={}): server compile failed for {}: {}",
                            case.name, suite, relative, error
                        ));
                    }
                }
            }
        }
    }

    assert_no_failures("js-unported-compile-smoke", failures);
}

#[test]
#[ignore = "debug helper; run explicitly"]
fn debug_single_snapshot_js_fixture() {
    let fixture_name = std::env::var("SVELTE_FIXTURE").expect("set SVELTE_FIXTURE=<fixture-name>");
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let mut cases = discover_suite_cases(&repo_root, "snapshot").expect("discover snapshot suite");
    cases.retain(|case| case.name == fixture_name);
    let case = cases
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("fixture not found: {fixture_name}"));

    let config = load_test_config::<FixtureConfigJson>(&case)
        .unwrap_or_else(|err| panic!("{} load _config.js: {err}", case.name));
    let options = compile_options_from_config(config.as_ref(), None, false);

    let source = normalize_source(
        case.read_required_text("index.svelte")
            .unwrap_or_else(|err| panic!("{} read index.svelte: {err}", case.name)),
    );

    let client = compile(
        &source,
        CompileOptions {
            filename: Some(case.path.join("index.svelte")),
            generate: GenerateTarget::Client,
            ..options.clone()
        },
    )
    .expect("client compile failed");
    let server = compile(
        &source,
        CompileOptions {
            filename: Some(case.path.join("index.svelte")),
            generate: GenerateTarget::Server,
            ..options
        },
    )
    .expect("server compile failed");

    let expected_client = case
        .read_required_text("_expected/client/index.svelte.js")
        .expect("missing expected client js");
    let expected_server = case
        .read_required_text("_expected/server/index.svelte.js")
        .expect("missing expected server js");

    println!("=== INPUT ===\n{}", normalize_newlines(&source));
    println!(
        "=== EXPECTED CLIENT ===\n{}",
        normalize_newlines(expected_client.trim())
    );
    println!(
        "=== ACTUAL CLIENT ===\n{}",
        normalize_newlines(client.js.code.trim())
    );
    println!(
        "=== NORMALIZED EXPECTED CLIENT ===\n{}",
        normalize_snapshot_js_output(&expected_client)
    );
    println!(
        "=== NORMALIZED ACTUAL CLIENT ===\n{}",
        normalize_snapshot_js_output(client.js.code.as_ref())
    );
    println!(
        "=== EXPECTED SERVER ===\n{}",
        normalize_newlines(expected_server.trim())
    );
    println!(
        "=== ACTUAL SERVER ===\n{}",
        normalize_newlines(server.js.code.trim())
    );
}

#[test]
#[ignore = "debug helper; run explicitly"]
fn debug_single_js_unported_compile_fixture() {
    let fixture_name = std::env::var("SVELTE_FIXTURE").expect("set SVELTE_FIXTURE=<fixture-name>");
    let requested_suite = std::env::var("SVELTE_SUITE").ok();
    let repo_root = detect_repo_root().expect("failed to detect repo root");

    let mut matched = None;
    for suite in UNPORTED_JS_COMPILE_SMOKE_SUITES {
        if requested_suite
            .as_deref()
            .is_some_and(|requested| requested != *suite)
        {
            continue;
        }
        let mut cases = discover_suite_cases_by_name(&repo_root, suite)
            .unwrap_or_else(|err| panic!("{suite}: {err}"));
        cases.retain(|case| case.name == fixture_name);
        if let Some(case) = cases.into_iter().next() {
            matched = Some((suite, case));
            break;
        }
    }

    let Some((suite, case)) = matched else {
        panic!("fixture not found in js-unported compile smoke suites: {fixture_name}");
    };

    let config = load_test_config::<FixtureConfigJson>(&case)
        .unwrap_or_else(|err| panic!("{} load _config.js: {err}", case.name));
    let options = compile_options_from_config(config.as_ref(), None, false);
    let fixture_files = recursive_fixture_files(&case)
        .unwrap_or_else(|err| panic!("{} enumerate fixture files: {err}", case.name));

    println!("=== FIXTURE {} (suite={suite}) ===", case.name);
    println!("=== OPTIONS ===\n{options:#?}");

    for relative in fixture_files {
        if relative.starts_with('_') {
            continue;
        }

        if relative.ends_with(".svelte.js") {
            let source = normalize_source(
                case.read_required_text(&relative)
                    .unwrap_or_else(|err| panic!("{} read {}: {err}", case.name, relative)),
            );

            println!("=== FILE {relative} ===\n{}", normalize_newlines(&source));

            for generate in [GenerateTarget::Client, GenerateTarget::Server] {
                let label = match generate {
                    GenerateTarget::Client => "client",
                    GenerateTarget::Server => "server",
                    GenerateTarget::None => "none",
                };

                match compile_module(
                    &source,
                    CompileOptions {
                        filename: Some(case.path.join(&relative)),
                        generate,
                        ..options.clone()
                    },
                ) {
                    Ok(result) => {
                        println!(
                            "=== {label} module ok; warnings={} js_len={} ===",
                            result.warnings.len(),
                            result.js.code.len()
                        );
                    }
                    Err(error) => {
                        println!("=== {label} module error ===\n{error}");
                    }
                }
            }
        } else if relative.ends_with(".svelte") {
            let source = normalize_source(
                case.read_required_text(&relative)
                    .unwrap_or_else(|err| panic!("{} read {}: {err}", case.name, relative)),
            );

            println!("=== FILE {relative} ===\n{}", normalize_newlines(&source));

            for generate in [GenerateTarget::Client, GenerateTarget::Server] {
                let label = match generate {
                    GenerateTarget::Client => "client",
                    GenerateTarget::Server => "server",
                    GenerateTarget::None => "none",
                };

                match compile(
                    &source,
                    CompileOptions {
                        filename: Some(case.path.join(&relative)),
                        generate,
                        ..options.clone()
                    },
                ) {
                    Ok(result) => {
                        println!(
                            "=== {label} ok; warnings={} js_len={} ===",
                            result.warnings.len(),
                            result.js.code.len()
                        );
                    }
                    Err(error) => {
                        println!("=== {label} error ===\n{error}");
                    }
                }
            }
        }
    }
}

#[test]
fn sourcemaps_suite_ported() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "sourcemaps").expect("discover sourcemaps suite");

    let mut failures = Vec::new();

    for case in cases {
        if should_skip_case(&case, &mut failures) {
            continue;
        }

        let config = load_test_config::<SourcemapFixtureConfigJson>(&case)
            .unwrap_or_else(|err| panic!("{} load _config.js: {err}", case.name));
        let config = config.unwrap_or_default();
        if config.skip {
            continue;
        }
        match execute_sourcemap_fixture(&case, &config) {
            Ok(artifacts) => {
                if let Some(expected_sources) = config.js_map_sources.as_ref() {
                    if let Some(map) = artifacts.client.js.map.as_ref() {
                        assert_sorted_sources(
                            &case.name,
                            "client",
                            map,
                            expected_sources,
                            &mut failures,
                        );
                    } else {
                        failures.push(format!(
                            "{}: expected client source map but got none",
                            case.name
                        ));
                    }
                } else if config.client.is_some() && artifacts.client.js.map.is_none() {
                    failures.push(format!(
                        "{}: expected client source map but got none",
                        case.name
                    ));
                }

                if let Some(Some(entries)) = config.client.as_ref()
                    && let Some(map) = artifacts.client.js.map.as_ref()
                    && let Err(error) = compare_sourcemap_entries(
                        "client",
                        &artifacts.input,
                        artifacts
                            .preprocessed
                            .as_ref()
                            .map(|value| value.code.as_ref()),
                        artifacts.client.js.code.as_ref(),
                        map,
                        entries,
                    )
                {
                    failures.push(format!("{}: {}", case.name, error));
                }

                if config.client.is_some() || config.server.is_some() {
                    if let Some(map) = artifacts.server.js.map.as_ref() {
                        let expected = config
                            .server
                            .as_ref()
                            .and_then(|value| value.as_ref())
                            .or_else(|| config.client.as_ref().and_then(|value| value.as_ref()))
                            .cloned()
                            .unwrap_or_default();
                        if let Err(error) = compare_sourcemap_entries(
                            "server",
                            &artifacts.input,
                            artifacts
                                .preprocessed
                                .as_ref()
                                .map(|value| value.code.as_ref()),
                            artifacts.server.js.code.as_ref(),
                            map,
                            &expected,
                        ) {
                            failures.push(format!("{}: {}", case.name, error));
                        }
                    } else {
                        failures.push(format!(
                            "{}: expected server source map but got none",
                            case.name
                        ));
                    }
                }

                if let Some(css_expectation) = config.css.as_ref() {
                    match css_expectation {
                        Some(entries) => match artifacts
                            .client
                            .css
                            .as_ref()
                            .and_then(|css| css.map.as_ref())
                        {
                            Some(map) => {
                                if let Some(expected_sources) = config.css_map_sources.as_ref() {
                                    assert_sorted_sources(
                                        &case.name,
                                        "css",
                                        map,
                                        expected_sources,
                                        &mut failures,
                                    );
                                }
                                let css_code = artifacts
                                    .client
                                    .css
                                    .as_ref()
                                    .map(|css| css.code.as_ref())
                                    .unwrap_or_default();
                                if let Err(error) = compare_sourcemap_entries(
                                    "css",
                                    &artifacts.input,
                                    artifacts
                                        .preprocessed
                                        .as_ref()
                                        .map(|value| value.code.as_ref()),
                                    css_code,
                                    map,
                                    entries,
                                ) {
                                    failures.push(format!("{}: {}", case.name, error));
                                }
                            }
                            None => failures.push(format!(
                                "{}: expected css source map but got none",
                                case.name
                            )),
                        },
                        None => {
                            if artifacts
                                .client
                                .css
                                .as_ref()
                                .and_then(|css| css.map.as_ref())
                                .is_some()
                            {
                                failures.push(format!("{}: expected no css source map", case.name));
                            }
                        }
                    }
                }

                if let Some(preprocessed_expectation) = config.preprocessed.as_ref() {
                    match preprocessed_expectation {
                        Some(entries) => match artifacts
                            .preprocessed
                            .as_ref()
                            .and_then(|value| value.map.as_ref())
                        {
                            Some(map) => {
                                if let Err(error) = compare_sourcemap_entries(
                                    "preprocessed",
                                    &artifacts.input,
                                    None,
                                    artifacts
                                        .preprocessed
                                        .as_ref()
                                        .map(|value| value.code.as_ref())
                                        .unwrap_or_default(),
                                    map,
                                    entries,
                                ) {
                                    failures.push(format!("{}: {}", case.name, error));
                                }
                            }
                            None => failures.push(format!(
                                "{}: expected preprocessed source map but got none",
                                case.name
                            )),
                        },
                        None => {
                            if artifacts
                                .preprocessed
                                .as_ref()
                                .and_then(|value| value.map.as_ref())
                                .is_some()
                            {
                                failures.push(format!(
                                    "{}: expected no preprocessed source map",
                                    case.name
                                ));
                            }
                        }
                    }
                }

                if let Err(error) =
                    run_sourcemap_fixture_post_checks(&case.name, &config, &artifacts)
                {
                    failures.push(format!("{}: {}", case.name, error));
                }
            }
            Err(error) => failures.push(format!("{}: {}", case.name, error)),
        }
    }

    assert_no_failures("sourcemaps", failures);
}

struct SourcemapFixtureArtifacts {
    input: String,
    preprocessed: Option<PreprocessResult>,
    client: svelte_compiler::CompileResult,
    server: svelte_compiler::CompileResult,
}

fn execute_sourcemap_fixture(
    case: &FixtureCase,
    config: &SourcemapFixtureConfigJson,
) -> Result<SourcemapFixtureArtifacts, String> {
    let repo_root = detect_repo_root().map_err(|err| format!("detect repo root: {err}"))?;
    let input = normalize_source(
        case.read_required_text("input.svelte")
            .map_err(|err| format!("missing input.svelte: {err}"))?,
    );
    let input_filename = case
        .path
        .join("input.svelte")
        .strip_prefix(&repo_root)
        .map(|path| path.to_path_buf())
        .map_err(|_| "strip repo root for input filename failed".to_string())?;
    let preprocess_filename = config
        .options
        .as_ref()
        .and_then(|options| options.filename.as_deref())
        .map(camino::Utf8PathBuf::from)
        .unwrap_or_else(|| input_filename.clone());
    let preprocessed =
        build_sourcemap_preprocessed_result(&case.name, &input, &preprocess_filename, config)?;
    let compile_source = preprocessed
        .as_ref()
        .map(|result| result.code.as_ref())
        .unwrap_or(input.as_str());
    let input_map = preprocessed.as_ref().and_then(|result| result.map.clone());

    let mut options = compile_options_from_config(None, config.compile_options.as_ref(), false);
    if options.css_hash.is_none() {
        let hash_input = options
            .filename
            .as_deref()
            .unwrap_or(input_filename.as_path())
            .as_str()
            .replace('\\', "/");
        options.css_hash = Some(Arc::from(format!("svelte-{}", svelte_hash(&hash_input))));
    }
    let client_result = compile(
        compile_source,
        CompileOptions {
            filename: options
                .filename
                .clone()
                .or_else(|| Some(input_filename.clone())),
            generate: GenerateTarget::Client,
            sourcemap: input_map.clone(),
            output_filename: sourcemap_output_filename(
                case,
                config,
                GenerateTarget::Client,
                &repo_root,
            ),
            css_output_filename: sourcemap_css_output_filename(
                case,
                config,
                GenerateTarget::Client,
                &repo_root,
            ),
            ..options.clone()
        },
    )
    .map_err(|err| format!("client compile failed: {err}"))?;

    let server_result = compile(
        compile_source,
        CompileOptions {
            filename: options.filename.clone().or_else(|| Some(input_filename)),
            generate: GenerateTarget::Server,
            sourcemap: input_map,
            output_filename: sourcemap_output_filename(
                case,
                config,
                GenerateTarget::Server,
                &repo_root,
            ),
            css_output_filename: sourcemap_css_output_filename(
                case,
                config,
                GenerateTarget::Server,
                &repo_root,
            ),
            ..options
        },
    )
    .map_err(|err| format!("server compile failed: {err}"))?;

    Ok(SourcemapFixtureArtifacts {
        input,
        preprocessed,
        client: client_result,
        server: server_result,
    })
}

fn sourcemap_output_filename(
    case: &FixtureCase,
    config: &SourcemapFixtureConfigJson,
    target: GenerateTarget,
    repo_root: &camino::Utf8Path,
) -> Option<camino::Utf8PathBuf> {
    match config
        .compile_options
        .as_ref()
        .and_then(|options| options.output_filename.as_ref())
    {
        Some(Some(path)) => Some(camino::Utf8PathBuf::from(path.as_str())),
        Some(None) => None,
        None => Some(
            case.path
                .join(format!(
                    "_output/{}/input.svelte.js",
                    match target {
                        GenerateTarget::Client => "client",
                        GenerateTarget::Server => "server",
                        GenerateTarget::None => "none",
                    }
                ))
                .strip_prefix(repo_root)
                .expect("output path under repo root")
                .to_path_buf(),
        ),
    }
}

fn sourcemap_css_output_filename(
    case: &FixtureCase,
    config: &SourcemapFixtureConfigJson,
    target: GenerateTarget,
    repo_root: &camino::Utf8Path,
) -> Option<camino::Utf8PathBuf> {
    match config
        .compile_options
        .as_ref()
        .and_then(|options| options.css_output_filename.as_ref())
    {
        Some(Some(path)) => Some(camino::Utf8PathBuf::from(path.as_str())),
        Some(None) => None,
        None => Some(
            case.path
                .join(format!(
                    "_output/{}/input.svelte.css",
                    match target {
                        GenerateTarget::Client => "client",
                        GenerateTarget::Server => "server",
                        GenerateTarget::None => "none",
                    }
                ))
                .strip_prefix(repo_root)
                .expect("output path under repo root")
                .to_path_buf(),
        ),
    }
}

fn build_sourcemap_preprocessed_result(
    fixture_name: &str,
    input: &str,
    filename: &camino::Utf8Path,
    config: &SourcemapFixtureConfigJson,
) -> Result<Option<PreprocessResult>, String> {
    let Some(spec) = sourcemap_preprocess_spec(fixture_name, input, filename, config)? else {
        return Ok(None);
    };

    let map = build_test_sourcemap(
        Some(filename.as_str()),
        &spec.code,
        &spec.sources,
        &spec.entries,
    )?;

    Ok(Some(PreprocessResult {
        code: Arc::from(spec.code),
        dependencies: Box::new([]),
        map: Some(map),
    }))
}

struct SourcemapPreprocessSpec {
    code: String,
    sources: Vec<(String, String)>,
    entries: Vec<TestMapEntryOwned>,
}

#[derive(Clone)]
struct TestMapEntryOwned {
    original: String,
    generated: Option<String>,
    original_occurrence: usize,
    generated_occurrence: usize,
    name: Option<String>,
    source_code: Option<String>,
}

fn sourcemap_preprocess_spec(
    fixture_name: &str,
    input: &str,
    filename: &camino::Utf8Path,
    config: &SourcemapFixtureConfigJson,
) -> Result<Option<SourcemapPreprocessSpec>, String> {
    let build_from_output = |code: String, extra_sources: Vec<(String, String)>| {
        let mut sources = vec![(
            filename.file_name().unwrap_or("input.svelte").to_string(),
            input.to_string(),
        )];
        sources.extend(extra_sources);
        let entries = collect_preprocess_entries(config, &code, &sources);
        Some(SourcemapPreprocessSpec {
            code,
            sources,
            entries,
        })
    };

    Ok(match fixture_name {
        "attached-sourcemap" => {
            let code = input
                .replace("replace_me_script", "done_replace_script_2")
                .replace(".replace_me_style", ".done_replace_style_2");
            build_from_output(code, Vec::new())
        }
        "css-injected-map" => {
            let code = input
                .replace("--replace-me-once", "\n --done-replace-once")
                .replace("--replace-me-twice", "\n  --done-replace-twice");
            build_from_output(code, Vec::new())
        }
        "decoded-sourcemap" => build_from_output(input.replace("replace me", "success"), Vec::new()),
        "preprocessed-markup" => build_from_output(input.replace("baritone", "bar"), Vec::new()),
        "preprocessed-multiple" => build_from_output(
            input
                .replace("baritone", "bar")
                .replace("--bazitone", "      --baz")
                .replacen("bar", "      bar", 1),
            Vec::new(),
        ),
        "preprocessed-no-map" => Some(SourcemapPreprocessSpec {
            code: input.replace("font-weight: bold;", "font-weight: bold; "),
            sources: vec![(
                filename
                    .file_name()
                    .unwrap_or("input.svelte")
                    .to_string(),
                input.to_string(),
            )],
            entries: collect_preprocess_entries(
                config,
                &input.replace("font-weight: bold;", "font-weight: bold; "),
                &[(
                    filename
                        .file_name()
                        .unwrap_or("input.svelte")
                        .to_string(),
                    input.to_string(),
                )],
            ),
        }),
        "preprocessed-script" => build_from_output(input.replace("baritone", "bar"), Vec::new()),
        "preprocessed-styles" => build_from_output(input.replace("baritone", "bar"), Vec::new()),
        "source-map-generator" => build_from_output(input.replace("baritone", "bar"), Vec::new()),
        "sourcemap-concat" => build_from_output(
            input.replacen(
                "export let name;",
                "console.log(\"Injected first line\");\n\texport let name;",
                1,
            ),
            Vec::new(),
        ),
        "sourcemap-names" => build_from_output(
            input
                .replace("baritone", "bar")
                .replace("--bazitone", "--baz")
                .replace("old_name_1", "new_name_1")
                .replace("old_name_2", "new_name_2"),
            Vec::new(),
        ),
        "external" => {
            let common = ":global(html) { height: 100%; }\n".to_string();
            let styles = ".awesome { color: orange; }\n".to_string();
            build_from_output(
                input.replace(
                    "<style lang=\"scss\" src=\"./styles.scss\"></style>",
                    &format!("<style lang=\"scss\">{common}{styles}</style>"),
                ),
                vec![
                    ("common.scss".to_string(), common),
                    ("styles.scss".to_string(), styles),
                ],
            )
        }
        "sourcemap-basename" => {
            let external_relative_filename = "external_code.css".to_string();
            let external_code = "\nspan {\n\t--external_code-var: 1px;\n}\n".to_string();
            let marker = "/* Filename from preprocess: src/input.svelte */".to_string();
            build_from_output(
                input.replace(
                    "<style src=\"external_code.css\"></style>",
                    &format!("<style>{marker}{external_code}</style>"),
                ),
                vec![(external_relative_filename, format!("{marker}{external_code}"))],
            )
        }
        "sourcemap-basename-without-outputname" => {
            let component_basename = "input.svelte".to_string();
            let css_basename = "input.css".to_string();
            let input_css = " h1 {color: blue;}".to_string();
            let marker =
                "/* Filename from preprocess: src/some/deep/path/input.svelte */".to_string();
            build_from_output(
                input.replace("h1 {color: red;}", &format!("{marker}{input_css}")),
                vec![
                    (component_basename, "h1 {color: red;}".to_string()),
                    (css_basename, format!("{marker}{input_css}")),
                ],
            )
        }
        "sourcemap-offsets" => {
            let external = "span { --external-var: 1px; }".to_string();
            build_from_output(
                input.replace(
                    "div { --component-var: 2px; }",
                    &(external.clone() + "div { --component-var: 2px; }"),
                ),
                vec![("external.css".to_string(), external)],
            )
        }
        "sourcemap-sources" => {
            let foo = "var answer = 42; // foo.js\n".to_string();
            let bar = "console.log(answer); // bar.js\n".to_string();
            let foo2 = "var answer2 = 84; // foo2.js\n".to_string();
            let bar2 = "console.log(answer2); // bar2.js\n".to_string();
            build_from_output(
                input.replacen(
                    "export let name;",
                    &format!("export let name;\n{foo}{bar}{foo2}{bar2}"),
                    1,
                ),
                vec![
                    ("foo.js".to_string(), foo),
                    ("bar.js".to_string(), bar),
                    ("foo2.js".to_string(), foo2),
                    ("bar2.js".to_string(), bar2),
                ],
            )
        }
        "typescript" => build_from_output(
            input
                .replace(": number", "")
                .replace(
                    "\n\tinterface ITimeoutDestroyer {\n\t\t(): void; // send timeout to the void!\n\t}\n",
                    "\n",
                )
                .replace(": ITimeoutDestroyer", ""),
            Vec::new(),
        ),
        _ => None,
    })
}

fn collect_preprocess_entries(
    config: &SourcemapFixtureConfigJson,
    output: &str,
    sources: &[(String, String)],
) -> Vec<TestMapEntryOwned> {
    let mut out = Vec::new();
    let mut push_entry = |entry: &SourcemapEntryJson| {
        let original = sourcemap_entry_str(entry).to_string();
        let generated = match sourcemap_entry_generated(entry) {
            Some(Some(value)) if output.contains(value) => Some(value.to_string()),
            Some(Some(value)) => normalize_preprocess_generated(value, output),
            Some(None) => None,
            _ if output.contains(&original) => Some(original.clone()),
            _ => None,
        };
        if generated.is_none() {
            return;
        }
        let code = sourcemap_entry_code(entry);
        if let Some(code) = code
            && !sources.iter().any(|(_, source)| source == code)
        {
            return;
        }
        let name = generated
            .as_ref()
            .filter(|generated| {
                generated.as_str() != original
                    && !original.chars().any(char::is_whitespace)
                    && !original.contains(':')
                    && !original.contains('(')
            })
            .map(|_| original.clone());
        out.push(TestMapEntryOwned {
            original,
            generated,
            original_occurrence: sourcemap_entry_idx_original(entry),
            generated_occurrence: sourcemap_entry_idx_generated(entry),
            name,
            source_code: code.map(ToOwned::to_owned),
        });
    };

    for entries in [
        config.client.as_ref().and_then(|value| value.as_ref()),
        config.server.as_ref().and_then(|value| value.as_ref()),
        config.css.as_ref().and_then(|value| value.as_ref()),
        config
            .preprocessed
            .as_ref()
            .and_then(|value| value.as_ref()),
    ]
    .into_iter()
    .flatten()
    {
        for entry in entries {
            push_entry(entry);
        }
    }

    out
}

fn normalize_preprocess_generated(value: &str, output: &str) -> Option<String> {
    if let Some(scope_index) = value.find(".svelte-") {
        let unscoped = &value[..scope_index];
        if output.contains(unscoped) {
            return Some(unscoped.to_string());
        }
    }

    None
}

fn assert_sorted_sources(
    case_name: &str,
    label: &str,
    map: &SourceMap,
    expected_sources: &[String],
    failures: &mut Vec<String>,
) {
    let mut actual = map
        .sources
        .iter()
        .map(|value| value.as_ref().to_string())
        .collect::<Vec<_>>();
    let mut expected = expected_sources.to_vec();
    actual.sort();
    expected.sort();
    if actual != expected {
        failures.push(format!(
            "{}: {} source list mismatch: actual={:?} expected={:?}",
            case_name, label, actual, expected
        ));
    }
}

fn compare_sourcemap_entries(
    info: &str,
    input: &str,
    _preprocessed: Option<&str>,
    output: &str,
    map: &SourceMap,
    entries: &[SourcemapEntryJson],
) -> Result<(), String> {
    let decoded = decode_test_mappings(map.mappings.as_ref());

    for entry in entries {
        let original_source = sourcemap_entry_code(entry).unwrap_or(input);
        let original = sourcemap_entry_str(entry);
        let original_pos = find_nth_offset_and_line_col(
            original_source,
            original,
            sourcemap_entry_idx_original(entry),
        )
        .ok_or_else(|| format!("could not find '{original}' in original {info} source"))?;

        let generated_value = match sourcemap_entry_generated(entry) {
            Some(None) => {
                if let Some((_, generated_line, generated_column)) = find_nth_offset_and_line_col(
                    output,
                    original,
                    sourcemap_entry_idx_generated(entry),
                ) {
                    let segment = decoded.get(generated_line).and_then(|line| {
                        line.iter()
                            .find(|segment| segment.generated_column == generated_column)
                    });
                    if segment.is_some() {
                        return Err(format!(
                            "found segment for '{original}' in {info} sourcemap ({generated_line}:{generated_column}) but should not"
                        ));
                    }
                }
                continue;
            }
            Some(Some(value)) => value,
            None => original,
        };

        let (generated_offset, generated_line, generated_column) = find_nth_offset_and_line_col(
            output,
            generated_value,
            sourcemap_entry_idx_generated(entry),
        )
        .ok_or_else(|| format!("could not find '{generated_value}' in generated {info} output"))?;

        let segments = decoded
            .get(generated_line)
            .ok_or_else(|| format!("missing decoded mappings for line {}", generated_line))?;
        let segment = segments
            .iter()
            .find(|segment| segment.generated_column == generated_column)
            .ok_or_else(|| {
                format!(
                    "could not find segment for '{}' in {} sourcemap ({}:{})",
                    original, info, generated_line, generated_column
                )
            })?;

        if segment.original_line != original_pos.1 || segment.original_column != original_pos.2 {
            return Err(format!(
                "mapped position mismatch for '{}' in {} sourcemap: actual=({}:{}) expected=({}:{})",
                original,
                info,
                segment.original_line,
                segment.original_column,
                original_pos.1,
                original_pos.2
            ));
        }

        let generated_end_column = generated_column + generated_value.len();
        let end_segment = segments
            .iter()
            .find(|segment| segment.generated_column == generated_end_column);
        if end_segment.is_none() {
            let next = output
                .as_bytes()
                .get(generated_offset + generated_value.len())
                .copied();
            let last = segments.last().copied();
            if last.is_some_and(|last| last.generated_column <= generated_end_column)
                && next.is_some_and(|ch| ch == b'\n' || ch == b'\r')
            {
                continue;
            }
            return Err(format!(
                "could not find end segment for '{}' in {} sourcemap ({}:{})",
                original, info, generated_line, generated_end_column
            ));
        }
        let end_segment = end_segment.expect("checked above");
        if end_segment.original_line != original_pos.1
            || end_segment.original_column != original_pos.2 + original.len()
        {
            return Err(format!(
                "mapped end position mismatch for '{}' in {} sourcemap",
                original, info
            ));
        }
    }

    Ok(())
}

fn run_sourcemap_fixture_post_checks(
    fixture_name: &str,
    _config: &SourcemapFixtureConfigJson,
    artifacts: &SourcemapFixtureArtifacts,
) -> Result<(), String> {
    match fixture_name {
        "attached-sourcemap" => {
            if artifacts.preprocessed.as_ref().is_some_and(|result| {
                result
                    .code
                    .contains("sourceMappingURL=data:application/json;base64,")
            }) {
                return Err(
                    "magic-comment attachments were not removed from preprocessed output"
                        .to_string(),
                );
            }
            if artifacts.client.css.as_ref().is_some_and(|css| {
                css.code
                    .contains("sourceMappingURL=data:application/json;base64,")
            }) {
                return Err(
                    "magic-comment attachments were not removed from css output".to_string()
                );
            }
        }
        "sourcemap-basename" => {
            let preprocessed = artifacts
                .preprocessed
                .as_ref()
                .ok_or_else(|| "missing preprocessed result".to_string())?;
            if !preprocessed
                .code
                .contains("/* Filename from preprocess: src/input.svelte */")
            {
                return Err("preprocessed code did not preserve filename marker".to_string());
            }
            let map = preprocessed
                .map
                .as_ref()
                .ok_or_else(|| "missing preprocessed map".to_string())?;
            let mut actual = map
                .sources
                .iter()
                .map(|value| value.as_ref().to_string())
                .collect::<Vec<_>>();
            actual.sort();
            let mut expected = vec!["external_code.css".to_string(), "input.svelte".to_string()];
            expected.sort();
            if actual != expected {
                return Err(format!(
                    "preprocessed source list mismatch: actual={:?} expected={:?}",
                    actual, expected
                ));
            }
        }
        "sourcemap-names" => {
            let map = artifacts
                .preprocessed
                .as_ref()
                .and_then(|result| result.map.as_ref())
                .ok_or_else(|| "missing preprocessed map".to_string())?;
            let mut actual = map
                .names
                .iter()
                .map(|value| value.as_ref().to_string())
                .collect::<Vec<_>>();
            actual.sort();
            let mut expected = vec![
                "baritone".to_string(),
                "--bazitone".to_string(),
                "old_name_1".to_string(),
                "old_name_2".to_string(),
            ];
            expected.sort();
            if actual != expected {
                return Err(format!(
                    "preprocessed names mismatch: actual={:?} expected={:?}",
                    actual, expected
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn sourcemap_entry_str(entry: &SourcemapEntryJson) -> &str {
    match entry {
        SourcemapEntryJson::String(value) => value.as_str(),
        SourcemapEntryJson::Object(value) => value.str.as_str(),
    }
}

fn sourcemap_entry_generated(entry: &SourcemapEntryJson) -> Option<Option<&str>> {
    match entry {
        SourcemapEntryJson::String(_) => None,
        SourcemapEntryJson::Object(value) => match &value.str_generated {
            GeneratedStringField::Missing => None,
            GeneratedStringField::Null => Some(None),
            GeneratedStringField::Value(value) => Some(Some(value.as_str())),
        },
    }
}

fn sourcemap_entry_code(entry: &SourcemapEntryJson) -> Option<&str> {
    match entry {
        SourcemapEntryJson::String(_) => None,
        SourcemapEntryJson::Object(value) => value.code.as_deref(),
    }
}

fn sourcemap_entry_idx_original(entry: &SourcemapEntryJson) -> usize {
    match entry {
        SourcemapEntryJson::String(_) => 0,
        SourcemapEntryJson::Object(value) => value.idx_original.unwrap_or(0),
    }
}

fn sourcemap_entry_idx_generated(entry: &SourcemapEntryJson) -> usize {
    match entry {
        SourcemapEntryJson::String(_) => 0,
        SourcemapEntryJson::Object(value) => value.idx_generated.unwrap_or(0),
    }
}

fn build_test_sourcemap(
    output_filename: Option<&str>,
    output: &str,
    sources: &[(String, String)],
    entries: &[TestMapEntryOwned],
) -> Result<SourceMap, String> {
    let mut names = Vec::<Arc<str>>::new();
    let mut name_lookup = BTreeMap::<String, usize>::new();
    let mut lines = vec![Vec::<TestDecodedSegment>::new(); output.split('\n').count().max(1)];

    for entry in entries {
        let Some(generated) = entry.generated.as_deref() else {
            continue;
        };

        let (source_index, source_code) = sources
            .iter()
            .enumerate()
            .find(|(_, (_, code))| {
                entry
                    .source_code
                    .as_ref()
                    .is_none_or(|expected| code == expected)
                    && find_nth_offset(code, &entry.original, entry.original_occurrence).is_some()
            })
            .map(|(index, (_, code))| (index, code.as_str()))
            .ok_or_else(|| {
                format!(
                    "unable to locate original '{}' in source set",
                    entry.original
                )
            })?;
        let original_offset =
            find_nth_offset(source_code, &entry.original, entry.original_occurrence)
                .ok_or_else(|| format!("missing original occurrence for '{}'", entry.original))?;
        let generated_offset = find_nth_offset(output, generated, entry.generated_occurrence)
            .ok_or_else(|| format!("missing generated occurrence for '{}'", generated))?;
        let (_, original_line, original_column) =
            find_nth_offset_and_line_col(source_code, &entry.original, entry.original_occurrence)
                .ok_or_else(|| format!("missing original line/column for '{}'", entry.original))?;
        let (_, generated_line, generated_column) =
            find_nth_offset_and_line_col(output, generated, entry.generated_occurrence)
                .ok_or_else(|| format!("missing generated line/column for '{}'", generated))?;
        let name_index = entry.name.as_ref().map(|name| {
            if let Some(index) = name_lookup.get(name) {
                *index
            } else {
                let index = names.len();
                names.push(Arc::from(name.as_str()));
                name_lookup.insert(name.clone(), index);
                index
            }
        });

        lines[generated_line].push(TestDecodedSegment {
            generated_column,
            source_index,
            original_line,
            original_column,
            name_index,
        });

        let original_end = line_col_at(source_code, original_offset + entry.original.len());
        let generated_end = line_col_at(output, generated_offset + generated.len());
        lines[generated_end.0].push(TestDecodedSegment {
            generated_column: generated_end.1,
            source_index,
            original_line: original_end.0,
            original_column: original_end.1,
            name_index,
        });
    }

    for line in &mut lines {
        line.sort_by_key(|segment| segment.generated_column);
        line.dedup_by_key(|segment| segment.generated_column);
    }

    Ok(SourceMap {
        version: 3,
        file: output_filename
            .and_then(|filename| camino::Utf8Path::new(filename).file_name())
            .map(Arc::from),
        source_root: None,
        sources: sources
            .iter()
            .map(|(filename, _)| Arc::from(filename.as_str()))
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        sources_content: None,
        names: names.into_boxed_slice(),
        mappings: Arc::from(encode_test_mappings(&lines)),
    })
}

fn encode_test_mappings(lines: &[Vec<TestDecodedSegment>]) -> String {
    let mut encoded = String::new();
    let mut previous_source_index = 0i64;
    let mut previous_original_line = 0i64;
    let mut previous_original_column = 0i64;
    let mut previous_name_index = 0i64;

    for (line_index, line) in lines.iter().enumerate() {
        if line_index > 0 {
            encoded.push(';');
        }
        let mut previous_generated_column = 0i64;
        for (entry_index, segment) in line.iter().enumerate() {
            if entry_index > 0 {
                encoded.push(',');
            }
            encode_test_vlq(
                segment.generated_column as i64 - previous_generated_column,
                &mut encoded,
            );
            previous_generated_column = segment.generated_column as i64;
            encode_test_vlq(
                segment.source_index as i64 - previous_source_index,
                &mut encoded,
            );
            previous_source_index = segment.source_index as i64;
            encode_test_vlq(
                segment.original_line as i64 - previous_original_line,
                &mut encoded,
            );
            previous_original_line = segment.original_line as i64;
            encode_test_vlq(
                segment.original_column as i64 - previous_original_column,
                &mut encoded,
            );
            previous_original_column = segment.original_column as i64;
            if let Some(name_index) = segment.name_index {
                encode_test_vlq(name_index as i64 - previous_name_index, &mut encoded);
                previous_name_index = name_index as i64;
            }
        }
    }

    encoded
}

fn encode_test_vlq(value: i64, out: &mut String) {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut value = if value < 0 {
        ((-value as u64) << 1) | 1
    } else {
        (value as u64) << 1
    };

    loop {
        let mut digit = (value & 31) as usize;
        value >>= 5;
        if value != 0 {
            digit |= 32;
        }
        out.push(CHARS[digit] as char);
        if value == 0 {
            break;
        }
    }
}

fn find_nth_offset(haystack: &str, needle: &str, occurrence: usize) -> Option<usize> {
    let mut start = 0usize;
    let mut found = 0usize;
    while let Some(relative) = haystack.get(start..)?.find(needle) {
        let absolute = start + relative;
        if found == occurrence {
            return Some(absolute);
        }
        found += 1;
        start = absolute + needle.len();
    }
    None
}

fn find_nth_offset_and_line_col(
    haystack: &str,
    needle: &str,
    occurrence: usize,
) -> Option<(usize, usize, usize)> {
    let offset = find_nth_offset(haystack, needle, occurrence)?;
    let (line, column) = line_col_at(haystack, offset);
    Some((offset, line, column))
}

fn svelte_hash(input: &str) -> String {
    let normalized = input.replace('\r', "");
    let mut hash = 5381u32;
    for ch in normalized.chars().rev() {
        hash = hash.wrapping_shl(5).wrapping_sub(hash) ^ (ch as u32);
    }
    radix36(hash)
}

fn radix36(mut value: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(DIGITS[(value % 36) as usize] as char);
        value /= 36;
    }
    out.iter().rev().collect()
}

fn assert_snapshot_js_output(
    case: &FixtureCase,
    relative_input: &str,
    target: &str,
    expected_path: &str,
    result: Result<svelte_compiler::CompileResult, svelte_compiler::CompileError>,
    failures: &mut Vec<String>,
) {
    let expected = match case
        .read_optional_text(expected_path)
        .unwrap_or_else(|err| panic!("{} read {}: {err}", case.name, expected_path))
    {
        Some(expected) => expected,
        None => {
            // Missing expectations must fail explicitly; silent passthrough hides implementation gaps.
            failures.push(format!(
                "{}: missing expected snapshot {} for {} ({})",
                case.name, expected_path, target, relative_input
            ));
            return;
        }
    };

    match result {
        Ok(output) => {
            let actual = normalize_snapshot_js_output(output.js.code.as_ref());
            let expected = normalize_snapshot_js_output(&expected);
            if actual != expected {
                if std::env::var("SVELTE_FIXTURE").is_ok() {
                    eprintln!("=== {}: {} EXPECTED ===\n{expected}\n=== ACTUAL ===\n{actual}\n", case.name, target);
                }
                failures.push(format!(
                    "{}: {} js mismatch for {} (expected {})",
                    case.name, target, relative_input, expected_path
                ));
            }
        }
        Err(error) => failures.push(format!(
            "{}: {} compile failed for {}: {}",
            case.name, target, relative_input, error
        )),
    }
}

fn normalize_snapshot_js_output(source: &str) -> String {
    let normalized = normalize_newlines(source.trim_end());
    let mut lines = normalized.lines().map(str::to_string).collect::<Vec<_>>();

    for line in lines.iter_mut() {
        if let Some((prefix, _)) = line.split_once(" generated by Svelte v")
            && line.ends_with(" */")
        {
            *line = format!("{prefix} generated by Svelte VERSION */");
        }
    }

    lines.join("\n")
}

#[test]
fn sourcemap_basic_fixture_decodes_expected_client_segment() {
    assert_named_sourcemap_segment(
        "basic",
        GenerateTarget::Client,
        "foo.bar.baz",
        "foo.bar.baz",
    );
}

#[test]
fn sourcemap_script_fixture_decodes_expected_client_segment() {
    assert_named_sourcemap_segment("script", GenerateTarget::Client, "42", "42");
}

#[test]
#[ignore = "debug helper; run explicitly"]
fn debug_single_sourcemap_fixture() {
    let fixture_name = std::env::var("SVELTE_FIXTURE").expect("set SVELTE_FIXTURE=<fixture-name>");
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases =
        discover_suite_cases_by_name(&repo_root, "sourcemaps").expect("discover sourcemaps");
    let case = cases
        .into_iter()
        .find(|case| case.name == fixture_name)
        .unwrap_or_else(|| panic!("fixture not found: {fixture_name}"));
    let config = load_test_config::<SourcemapFixtureConfigJson>(&case)
        .unwrap_or_else(|err| panic!("{} load _config.js: {err}", case.name))
        .unwrap_or_default();
    let artifacts = execute_sourcemap_fixture(&case, &config).expect("execute fixture");
    println!("=== INPUT ===\n{}", artifacts.input);
    if let Some(preprocessed) = artifacts.preprocessed.as_ref() {
        println!("=== PREPROCESSED ===\n{}", preprocessed.code);
        if let Some(map) = preprocessed.map.as_ref() {
            println!(
                "=== PREPROCESSED MAP ===\n{}",
                serde_json::to_string_pretty(map).unwrap()
            );
        }
    }
    println!("=== CLIENT JS ===\n{}", artifacts.client.js.code);
    if let Some(map) = artifacts.client.js.map.as_ref() {
        println!(
            "=== CLIENT MAP ===\n{}",
            serde_json::to_string_pretty(map).unwrap()
        );
    }
    if let Some(css) = artifacts.client.css.as_ref() {
        println!("=== CLIENT CSS ===\n{}", css.code);
        if let Some(map) = css.map.as_ref() {
            println!(
                "=== CLIENT CSS MAP ===\n{}",
                serde_json::to_string_pretty(map).unwrap()
            );
        }
    }
    println!("=== SERVER JS ===\n{}", artifacts.server.js.code);
    if let Some(map) = artifacts.server.js.map.as_ref() {
        println!(
            "=== SERVER MAP ===\n{}",
            serde_json::to_string_pretty(map).unwrap()
        );
    }
}

#[test]
#[ignore = "debug helper; run explicitly"]
fn debug_single_sourcemap_fixture_parse() {
    let fixture_name = std::env::var("SVELTE_FIXTURE").expect("set SVELTE_FIXTURE=<fixture-name>");
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases =
        discover_suite_cases_by_name(&repo_root, "sourcemaps").expect("discover sourcemaps");
    let case = cases
        .into_iter()
        .find(|case| case.name == fixture_name)
        .unwrap_or_else(|| panic!("fixture not found: {fixture_name}"));
    let config = load_test_config::<SourcemapFixtureConfigJson>(&case)
        .unwrap_or_else(|err| panic!("{} load _config.js: {err}", case.name))
        .unwrap_or_default();
    let artifacts = execute_sourcemap_fixture(&case, &config).expect("execute fixture");
    let source = artifacts
        .preprocessed
        .as_ref()
        .map(|value| value.code.as_ref())
        .unwrap_or(artifacts.input.as_str());
    let ast = parse(
        source,
        ParseOptions {
            mode: ParseMode::Modern,
            loose: false,
            ..Default::default()
        },
    )
    .expect("parse sourcemap fixture");
    println!(
        "=== AST ROOT ===\n{}",
        serde_json::to_string_pretty(&ast.root).expect("serialize ast root")
    );
}

fn assert_named_sourcemap_segment(
    fixture_name: &str,
    target: GenerateTarget,
    original: &str,
    generated: &str,
) {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases =
        discover_suite_cases_by_name(&repo_root, "sourcemaps").expect("discover sourcemaps");
    let case = cases
        .into_iter()
        .find(|case| case.name == fixture_name)
        .unwrap_or_else(|| panic!("{fixture_name} fixture exists"));

    let source = normalize_source(
        case.read_required_text("input.svelte")
            .unwrap_or_else(|err| panic!("{} missing input.svelte: {err}", case.name)),
    );
    let output = compile(
        &source,
        CompileOptions {
            filename: Some(case.path.join("input.svelte")),
            generate: target,
            output_filename: Some(case.path.join(format!(
                "_output/{}/input.svelte.js",
                match target {
                    GenerateTarget::Client => "client",
                    GenerateTarget::Server => "server",
                    GenerateTarget::None => "none",
                }
            ))),
            css_output_filename: Some(case.path.join(format!(
                "_output/{}/input.svelte.css",
                match target {
                    GenerateTarget::Client => "client",
                    GenerateTarget::Server => "server",
                    GenerateTarget::None => "none",
                }
            ))),
            ..CompileOptions::default()
        },
    )
    .unwrap_or_else(|err| panic!("{} compile failed: {err}", case.name));

    let map = output.js.map.expect("js map");
    let decoded = decode_test_mappings(map.mappings.as_ref());

    let original_pos = find_nth_line_col(&source, original, 0).expect("original token");
    let generated_pos =
        find_nth_line_col(output.js.code.as_ref(), generated, 0).expect("generated token");

    let segment = decoded
        .get(generated_pos.0)
        .and_then(|line| {
            line.iter()
                .find(|segment| segment.generated_column == generated_pos.1)
        })
        .unwrap_or_else(|| panic!("missing mapping segment for {}", generated));

    assert_eq!(
        map.sources
            .get(segment.source_index)
            .map(|value| value.as_ref()),
        Some("../../input.svelte")
    );
    assert_eq!(
        segment.original_line, original_pos.0,
        "mapped line mismatch"
    );
    assert_eq!(
        segment.original_column, original_pos.1,
        "mapped column mismatch"
    );
}

#[derive(Debug, Clone, Copy)]
struct TestDecodedSegment {
    generated_column: usize,
    source_index: usize,
    original_line: usize,
    original_column: usize,
    name_index: Option<usize>,
}

fn decode_test_mappings(mappings: &str) -> Vec<Vec<TestDecodedSegment>> {
    let mut out = Vec::new();
    let mut source_index = 0i64;
    let mut original_line = 0i64;
    let mut original_column = 0i64;
    let mut name_index = 0i64;

    for line in mappings.split(';') {
        let mut decoded_line = Vec::new();
        let mut generated_column = 0i64;

        for entry in line.split(',').filter(|entry| !entry.is_empty()) {
            let mut cursor = 0usize;
            generated_column += decode_test_vlq(entry, &mut cursor);
            source_index += decode_test_vlq(entry, &mut cursor);
            original_line += decode_test_vlq(entry, &mut cursor);
            original_column += decode_test_vlq(entry, &mut cursor);
            let decoded_name_index = if cursor < entry.len() {
                name_index += decode_test_vlq(entry, &mut cursor);
                Some(name_index as usize)
            } else {
                None
            };
            decoded_line.push(TestDecodedSegment {
                generated_column: generated_column as usize,
                source_index: source_index as usize,
                original_line: original_line as usize,
                original_column: original_column as usize,
                name_index: decoded_name_index,
            });
        }

        out.push(decoded_line);
    }

    out
}

fn decode_test_vlq(input: &str, cursor: &mut usize) -> i64 {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut shift = 0u32;
    let mut value = 0u64;

    while let Some(&byte) = bytes.get(*cursor) {
        *cursor += 1;
        let digit = CHARS
            .iter()
            .position(|candidate| *candidate == byte)
            .expect("invalid base64-vlq digit") as u64;
        let continuation = (digit & 0b10_0000) != 0;
        value |= (digit & 0b1_1111) << shift;
        shift += 5;
        if !continuation {
            break;
        }
    }

    let signed = value as i64;
    let negative = (signed & 1) == 1;
    let shifted = signed >> 1;
    if negative { -shifted } else { shifted }
}

fn find_nth_line_col(haystack: &str, needle: &str, occurrence: usize) -> Option<(usize, usize)> {
    let mut start = 0usize;
    let mut found = 0usize;
    while let Some(relative) = haystack.get(start..)?.find(needle) {
        let absolute = start + relative;
        if found == occurrence {
            return Some(line_col_at(haystack, absolute));
        }
        found += 1;
        start = absolute + needle.len();
    }
    None
}

fn line_col_at(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut column = 0usize;
    for (index, ch) in text.char_indices() {
        if index >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 0;
        } else {
            column += ch.len_utf8();
        }
    }
    (line, column)
}

#[test]
fn css_parse_suite_ported() {
    let mut failures = Vec::new();

    let cases = [
        "div { color: red; }",
        "@media (min-width: 800px) { div { color: red; } }",
        "@import 'foo.css';",
        "div { color: red; } span { color: blue; }",
        "\u{feff}div { color: red; }",
        "div { color: red; span { color: blue; } }",
        "",
        "   \n\t  ",
        "/* comment */ div { color: red; }",
        "div > span + p ~ a { color: red; }",
        "div:hover::before { color: red; }",
        "@keyframes fade { from { opacity: 0; } to { opacity: 1; } }",
        ".foo#bar { color: red; }",
        "[data-foo=\"bar\"] { color: red; }",
        "div { background: url('./example.png?\\\''); }",
    ];

    for (index, source) in cases.iter().enumerate() {
        if let Err(error) = parse_css(source) {
            failures.push(format!("case {index}: parse_css failed: {error}"));
        }
    }

    assert_no_failures("css-parse", failures);
}

#[test]
fn debug_a11y_no_static_element_interactions_case() {
    let repo_root = detect_repo_root().expect("failed to detect repo root");
    let cases = discover_suite_cases(&repo_root, "validator").expect("discover validator cases");
    let case = cases
        .into_iter()
        .find(|case| case.name == "a11y-no-static-element-interactions")
        .expect("missing a11y-no-static-element-interactions fixture");

    let input = normalize_source(
        case.read_required_text("input.svelte")
            .expect("read input.svelte"),
    );
    let output = compile(
        &input,
        CompileOptions {
            filename: Some(case.path.join("input.svelte")),
            generate: GenerateTarget::None,
            ..CompileOptions::default()
        },
    )
    .expect("compile");

    for warning in output.warnings {
        eprintln!(
            "{} @ {:?}: {}",
            warning.code,
            warning.position,
            strip_doc_link(&warning.message)
        );
    }
}

// Cases that cannot pass due to fundamental tree-sitter limitations
// (not incomplete implementation). These have skip:true in the JS suite too.
const TREE_SITTER_LIMITATION_SKIPS: &[&str] = &[
    // Implicit <li> closing across Svelte block boundaries requires
    // parser-level understanding of HTML content model + block structure.
    // tree-sitter's external scanner tag stack can't handle this interaction.
    "implicitly-closed-li-block",
];

fn should_skip_case(case: &FixtureCase, failures: &mut Vec<String>) -> bool {
    if TREE_SITTER_LIMITATION_SKIPS.contains(&case.name.as_str()) {
        return true;
    }
    // We do not honor fixture-level `skip` toggles in this harness.
    // Every case must execute so incomplete implementation fails loudly.
    match load_test_config::<FixtureConfigJson>(case) {
        Ok(_) => false,
        Err(err) => {
            failures.push(format!("{}: unable to load _config.js: {err}", case.name));
            true
        }
    }
}

fn read_optional_json<T: DeserializeOwned>(
    case: &FixtureCase,
    relative: &str,
) -> std::io::Result<Option<T>> {
    let Some(source) = case.read_optional_text(relative)? else {
        return Ok(None);
    };
    serde_json::from_str::<T>(&source)
        .map(Some)
        .map_err(|error| std::io::Error::other(format!("invalid JSON in {}: {}", relative, error)))
}

fn compile_options_from_config(
    config: Option<&FixtureConfigJson>,
    options_json: Option<&FixtureCompileOptionsJson>,
    force_generate_none: bool,
) -> CompileOptions {
    let config_compile_options = config.and_then(|config| config.compile_options.as_ref());

    let generate = if force_generate_none {
        options_json
            .and_then(|options| options.generate.as_ref())
            .and_then(generate_target_from_option)
            .unwrap_or(GenerateTarget::None)
    } else {
        options_json
            .and_then(|options| options.generate.as_ref())
            .and_then(generate_target_from_option)
            .or_else(|| {
                config_compile_options
                    .and_then(|options| options.generate.as_ref())
                    .and_then(generate_target_from_option)
            })
            .unwrap_or(GenerateTarget::Client)
    };

    let dev = options_json
        .and_then(|options| options.dev)
        .or_else(|| config_compile_options.and_then(|options| options.dev))
        .unwrap_or(false);
    let hmr = options_json
        .and_then(|options| options.hmr)
        .or_else(|| config_compile_options.and_then(|options| options.hmr))
        .unwrap_or(false);
    let custom_element = options_json
        .and_then(|options| options.custom_element)
        .or_else(|| config_compile_options.and_then(|options| options.custom_element))
        .unwrap_or(false);
    let fragments = options_json
        .and_then(|options| options.fragments)
        .or_else(|| config_compile_options.and_then(|options| options.fragments))
        .map(fragment_strategy_from_option)
        .unwrap_or(FragmentStrategy::Html);
    let warning_filter_ignore_codes =
        warning_filter_ignore_codes_from_options(config_compile_options, options_json);
    let runes = options_json
        .and_then(|options| options.runes)
        .or_else(|| config_compile_options.and_then(|options| options.runes));
    let error_mode = options_json
        .and_then(|options| options.error_mode)
        .or_else(|| config_compile_options.and_then(|options| options.error_mode))
        .map(error_mode_from_option)
        .unwrap_or(ErrorMode::Error);
    let async_mode = options_json
        .and_then(|options| options.experimental.as_ref())
        .map(|experimental| experimental.r#async)
        .or_else(|| {
            config_compile_options
                .and_then(|options| options.experimental.as_ref())
                .map(|experimental| experimental.r#async)
        })
        .unwrap_or(false);

    let filename = options_json
        .and_then(|options| options.filename.as_deref())
        .map(camino::Utf8PathBuf::from);

    let css_hash = config_compile_options
        .and_then(|options| options.css_hash.as_ref())
        .and_then(FixtureConfigString::as_str)
        .map(Arc::from);

    CompileOptions {
        filename,
        generate,
        fragments,
        dev,
        hmr,
        custom_element,
        warning_filter_ignore_codes,
        runes,
        error_mode,
        sourcemap: None,
        output_filename: None,
        css_output_filename: None,
        css_hash,
        experimental: svelte_compiler::ExperimentalOptions {
            r#async: async_mode,
        },
        ..CompileOptions::default()
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureConfigJson {
    #[serde(default, alias = "skip_filename")]
    skip_filename: bool,
    #[serde(default, alias = "use_ts")]
    use_ts: bool,
    #[serde(default)]
    compile_options: Option<FixtureCompileOptionsJson>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureCompileOptionsJson {
    #[serde(default)]
    generate: Option<GenerateOption>,
    #[serde(default)]
    dev: Option<bool>,
    #[serde(default)]
    hmr: Option<bool>,
    #[serde(default)]
    custom_element: Option<bool>,
    #[serde(default)]
    warning_filter: Option<WarningFilterOption>,
    #[serde(default)]
    runes: Option<bool>,
    #[serde(default)]
    error_mode: Option<ErrorModeOption>,
    #[serde(default)]
    fragments: Option<FragmentOption>,
    #[serde(default)]
    experimental: Option<ExperimentalCompileOption>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    output_filename: Option<Option<String>>,
    #[serde(default)]
    css_output_filename: Option<Option<String>>,
    #[serde(default)]
    css_hash: Option<FixtureConfigString>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourcemapFixtureConfigJson {
    #[serde(default)]
    skip: bool,
    #[serde(default)]
    options: Option<SourcemapFixtureOptionsJson>,
    #[serde(default)]
    compile_options: Option<FixtureCompileOptionsJson>,
    #[serde(default)]
    js_map_sources: Option<Vec<String>>,
    #[serde(default)]
    css_map_sources: Option<Vec<String>>,
    #[serde(default)]
    client: Option<Option<Vec<SourcemapEntryJson>>>,
    #[serde(default)]
    server: Option<Option<Vec<SourcemapEntryJson>>>,
    #[serde(default)]
    css: Option<Option<Vec<SourcemapEntryJson>>>,
    #[serde(default)]
    preprocessed: Option<Option<Vec<SourcemapEntryJson>>>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourcemapFixtureOptionsJson {
    #[serde(default)]
    filename: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum SourcemapEntryJson {
    String(String),
    Object(SourcemapEntryObjectJson),
}

#[derive(Debug, Clone)]
struct SourcemapEntryObjectJson {
    idx_original: Option<usize>,
    idx_generated: Option<usize>,
    str: String,
    str_generated: GeneratedStringField,
    code: Option<String>,
}

#[derive(Debug, Clone, Default)]
enum GeneratedStringField {
    #[default]
    Missing,
    Null,
    Value(String),
}

impl<'de> Deserialize<'de> for SourcemapEntryObjectJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("expected sourcemap entry object"))?;

        let idx_original = object
            .get("idxOriginal")
            .map(|value| {
                value
                    .as_u64()
                    .map(|value| value as usize)
                    .ok_or_else(|| serde::de::Error::custom("idxOriginal must be a number"))
            })
            .transpose()?;
        let idx_generated = object
            .get("idxGenerated")
            .map(|value| {
                value
                    .as_u64()
                    .map(|value| value as usize)
                    .ok_or_else(|| serde::de::Error::custom("idxGenerated must be a number"))
            })
            .transpose()?;
        let str = object
            .get("str")
            .and_then(|value| value.as_str())
            .ok_or_else(|| serde::de::Error::custom("missing string sourcemap entry str"))?
            .to_string();
        let str_generated = match object.get("strGenerated") {
            None => GeneratedStringField::Missing,
            Some(serde_json::Value::Null) => GeneratedStringField::Null,
            Some(serde_json::Value::String(value)) => GeneratedStringField::Value(value.clone()),
            Some(_) => {
                return Err(serde::de::Error::custom(
                    "strGenerated must be a string or null",
                ));
            }
        };
        let code = match object.get("code") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(value)) => Some(value.clone()),
            Some(_) => return Err(serde::de::Error::custom("code must be a string")),
        };

        Ok(Self {
            idx_original,
            idx_generated,
            str,
            str_generated,
            code,
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FixtureConfigString {
    String(String),
    Other(serde::de::IgnoredAny),
}

impl FixtureConfigString {
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            Self::Other(_) => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WarningFilterOption {
    #[serde(default)]
    source: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExperimentalCompileOption {
    #[serde(default)]
    r#async: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ErrorModeOption {
    Error,
    Warn,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum FragmentOption {
    Html,
    Tree,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum GenerateOption {
    Bool(bool),
    String(String),
}

fn warning_filter_ignore_codes_from_options(
    config_compile_options: Option<&FixtureCompileOptionsJson>,
    options_json: Option<&FixtureCompileOptionsJson>,
) -> Box<[Arc<str>]> {
    let source = options_json
        .and_then(|options| options.warning_filter.as_ref())
        .and_then(|warning_filter| warning_filter.source.as_deref())
        .or_else(|| {
            config_compile_options
                .and_then(|options| options.warning_filter.as_ref())
                .and_then(|warning_filter| warning_filter.source.as_deref())
        });

    let Some(source) = source else {
        return Box::default();
    };

    let Some(open_index) = source.find('[') else {
        return Box::default();
    };
    let Some(close_rel) = source.get(open_index + 1..).and_then(|tail| tail.find(']')) else {
        return Box::default();
    };
    let close_index = open_index + 1 + close_rel;

    source
        .get(open_index + 1..close_index)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter_map(strip_js_string_literal)
        .map(Arc::from)
        .collect()
}

fn strip_js_string_literal(raw: &str) -> Option<&str> {
    if raw.len() < 2 {
        return None;
    }
    let mut chars = raw.chars();
    let first = chars.next()?;
    let last = raw.chars().next_back()?;
    if (first == '\'' || first == '"') && last == first {
        return raw.get(1..raw.len() - 1);
    }
    None
}

fn fragment_strategy_from_option(value: FragmentOption) -> FragmentStrategy {
    match value {
        FragmentOption::Html => FragmentStrategy::Html,
        FragmentOption::Tree => FragmentStrategy::Tree,
    }
}

fn error_mode_from_option(value: ErrorModeOption) -> ErrorMode {
    match value {
        ErrorModeOption::Error => ErrorMode::Error,
        ErrorModeOption::Warn => ErrorMode::Warn,
    }
}

fn generate_target_from_option(value: &GenerateOption) -> Option<GenerateTarget> {
    match value {
        GenerateOption::Bool(true) => Some(GenerateTarget::Client),
        GenerateOption::Bool(false) => Some(GenerateTarget::None),
        GenerateOption::String(value) => match value.as_str() {
            "client" => Some(GenerateTarget::Client),
            "server" => Some(GenerateTarget::Server),
            _ => None,
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FixtureLocationJson {
    line: usize,
    column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FixtureWarningJson {
    code: String,
    message: String,
    start: Option<FixtureLocationJson>,
    end: Option<FixtureLocationJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FixtureErrorJson {
    code: String,
    message: String,
    start: Option<FixtureLocationJson>,
    end: Option<FixtureLocationJson>,
}

fn normalize_warning(warning: &svelte_compiler::Warning) -> FixtureWarningJson {
    let start = warning.start.as_ref().map(|loc| FixtureLocationJson {
        line: loc.line,
        column: loc.column,
    });
    let end = warning.end.as_ref().map(|loc| FixtureLocationJson {
        line: loc.line,
        column: loc.column,
    });

    FixtureWarningJson {
        code: warning.code.to_string(),
        message: strip_doc_link(&warning.message),
        start,
        end,
    }
}

fn location_tuple(location: Option<&FixtureLocationJson>) -> Option<(usize, usize)> {
    location.map(|location| (location.line, location.column))
}

fn recursive_fixture_files(case: &FixtureCase) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    collect_files_recursive(&case.path, &case.path, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_files_recursive(
    root: &camino::Utf8Path,
    current: &camino::Utf8Path,
    out: &mut Vec<String>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = match camino::Utf8PathBuf::from_path_buf(entry.path()) {
            Ok(path) => path,
            Err(_) => continue,
        };
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            collect_files_recursive(root, &path, out)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.as_str().replace('\\', "/"));
        }
    }

    Ok(())
}

#[test]
fn sourcemap_entry_deserializes_null_generated_value() {
    let entry: SourcemapEntryJson =
        serde_json::from_str(r#"{ "str": "ITimeoutDestroyer", "strGenerated": null }"#)
            .expect("deserialize sourcemap entry");
    let SourcemapEntryJson::Object(value) = entry else {
        panic!("expected object entry");
    };
    match value.str_generated {
        GeneratedStringField::Null => {}
        other => panic!("unexpected generated field: {other:?}"),
    }
}
