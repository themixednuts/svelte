use std::{fs, sync::Arc};

use svelte_compiler::{CompileOptions, GenerateTarget, compile};
use svelte_test_fixtures::detect_repo_root;

fn load_fixture(relative_path: &str) -> String {
    let repo_root = detect_repo_root().expect("detect repo root");
    let fixture_path = repo_root
        .join("packages")
        .join("svelte")
        .join("tests")
        .join(relative_path);
    fs::read_to_string(&fixture_path)
        .unwrap_or_else(|error| panic!("failed to read fixture {}: {error}", fixture_path))
}

fn compile_both_targets(source: &str, fixture_label: &str) -> Result<(), String> {
    for generate in [GenerateTarget::Client, GenerateTarget::Server] {
        let options = CompileOptions {
            generate,
            filename: Some(camino::Utf8PathBuf::from(format!("{fixture_label}.svelte"))),
            ..CompileOptions::default()
        };
        if let Err(error) = compile(source, options) {
            let target = match generate {
                GenerateTarget::Client => Arc::<str>::from("client"),
                GenerateTarget::Server => Arc::<str>::from("server"),
                _ => Arc::<str>::from("other"),
            };
            return Err(format!(
                "{target} compile failed: {} ({}) position={:?}",
                error.code, error, error.position
            ));
        }
    }

    Ok(())
}

#[test]
fn inline_each_else_fixture_compiles() {
    let source = load_fixture("hydration/samples/each-else/main.svelte");
    compile_both_targets(&source, "hydration/each-else").expect("inline each-else should compile");
}

#[test]
fn quoted_action_directive_fixture_compiles() {
    let source = load_fixture("runtime-legacy/samples/action/main.svelte");
    compile_both_targets(&source, "runtime-legacy/action")
        .expect("quoted action directive should compile");
}

#[test]
fn object_literal_spread_attribute_fixture_compiles() {
    let source = load_fixture("runtime-legacy/samples/attribute-after-property/main.svelte");
    compile_both_targets(&source, "runtime-legacy/attribute-after-property")
        .expect("object literal spread in attributes should compile");
}

#[test]
fn bindable_inside_props_destructure_compiles() {
    let source = load_fixture("runtime-runes/samples/props-bound/Counter.svelte");
    compile_both_targets(&source, "runtime-runes/props-bound/Counter")
        .expect("$bindable in $props destructuring should compile");
}

#[test]
fn duplicate_event_handlers_are_allowed() {
    let source = load_fixture("runtime-legacy/samples/event-handler-multiple/main.svelte");
    compile_both_targets(&source, "runtime-legacy/event-handler-multiple")
        .expect("multiple on: handlers should not trigger attribute_duplicate");
}

#[test]
fn slot_attribute_scope_component_chain_compiles() {
    let source = load_fixture("runtime-legacy/samples/component-slot-let-scope-4/main.svelte");
    compile_both_targets(&source, "runtime-legacy/component-slot-let-scope-4")
        .expect("slot placement across component chain should compile");
}
