use std::sync::Arc;

use svelte_compiler::{CompileOptions, GenerateTarget, compile};

const HYDRATION_EACH_ELSE: &str =
    include_str!("../../../packages/svelte/tests/hydration/samples/each-else/main.svelte");
const RUNTIME_LEGACY_ACTION: &str =
    include_str!("../../../packages/svelte/tests/runtime-legacy/samples/action/main.svelte");
const RUNTIME_LEGACY_ATTRIBUTE_AFTER_PROPERTY: &str = include_str!(
    "../../../packages/svelte/tests/runtime-legacy/samples/attribute-after-property/main.svelte"
);
const RUNTIME_RUNES_PROPS_BOUND_COUNTER: &str =
    include_str!("../../../packages/svelte/tests/runtime-runes/samples/props-bound/Counter.svelte");
const RUNTIME_LEGACY_EVENT_HANDLER_MULTIPLE: &str = include_str!(
    "../../../packages/svelte/tests/runtime-legacy/samples/event-handler-multiple/main.svelte"
);
const RUNTIME_LEGACY_COMPONENT_SLOT_LET_SCOPE_4: &str = include_str!(
    "../../../packages/svelte/tests/runtime-legacy/samples/component-slot-let-scope-4/main.svelte"
);

fn compile_both_targets(source: &str, fixture_label: &str) -> Result<(), String> {
    for generate in [GenerateTarget::Client, GenerateTarget::Server] {
        let options = CompileOptions {
            generate,
            filename: Some(
                camino::Utf8PathBuf::from(format!("{fixture_label}.svelte"))
                    .try_into()
                    .expect("utf8 path"),
            ),
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
    compile_both_targets(HYDRATION_EACH_ELSE, "hydration/each-else")
        .expect("inline each-else should compile");
}

#[test]
fn quoted_action_directive_fixture_compiles() {
    compile_both_targets(RUNTIME_LEGACY_ACTION, "runtime-legacy/action")
        .expect("quoted action directive should compile");
}

#[test]
fn object_literal_spread_attribute_fixture_compiles() {
    compile_both_targets(
        RUNTIME_LEGACY_ATTRIBUTE_AFTER_PROPERTY,
        "runtime-legacy/attribute-after-property",
    )
    .expect("object literal spread in attributes should compile");
}

#[test]
fn bindable_inside_props_destructure_compiles() {
    compile_both_targets(
        RUNTIME_RUNES_PROPS_BOUND_COUNTER,
        "runtime-runes/props-bound/Counter",
    )
    .expect("$bindable in $props destructuring should compile");
}

#[test]
fn duplicate_event_handlers_are_allowed() {
    compile_both_targets(
        RUNTIME_LEGACY_EVENT_HANDLER_MULTIPLE,
        "runtime-legacy/event-handler-multiple",
    )
    .expect("multiple on: handlers should not trigger attribute_duplicate");
}

#[test]
fn slot_attribute_scope_component_chain_compiles() {
    compile_both_targets(
        RUNTIME_LEGACY_COMPONENT_SLOT_LET_SCOPE_4,
        "runtime-legacy/component-slot-let-scope-4",
    )
    .expect("slot placement across component chain should compile");
}
