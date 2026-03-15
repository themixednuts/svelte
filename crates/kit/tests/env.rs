use serde_json::{Map, Value};
use svelte_kit::{EnvKind, create_dynamic_module, create_static_module, is_valid_identifier};

fn env(values: &[(&str, &str)]) -> Map<String, Value> {
    values
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
        .collect()
}

#[test]
fn creates_static_env_modules_with_identifier_filtering() {
    let module = create_static_module(
        "$env/static/private",
        &env(&[
            ("SECRET_KEY", "shh"),
            ("PUBLIC_OK", "yes"),
            ("not-valid", "no"),
            ("default", "reserved"),
        ]),
    );

    assert!(module.starts_with("// this file is generated"));
    assert!(module.contains("/** @type {import(\"$env/static/private\").SECRET_KEY} */"));
    assert!(module.contains("export const SECRET_KEY = \"shh\";"));
    assert!(module.contains("export const PUBLIC_OK = \"yes\";"));
    assert!(!module.contains("not-valid"));
    assert!(!module.contains("default"));
}

#[test]
fn creates_dynamic_env_modules_with_dev_values() {
    let module = create_dynamic_module(
        EnvKind::Public,
        Some(&env(&[("PUBLIC_FOO", "bar"), ("PUBLIC_BAR", "baz")])),
        "/runtime",
    );

    assert_eq!(
        module,
        "export const env = {\n\"PUBLIC_BAR\": \"baz\",\n\"PUBLIC_FOO\": \"bar\"\n}"
    );
}

#[test]
fn creates_dynamic_env_modules_that_reexport_runtime_values() {
    let public_module = create_dynamic_module(EnvKind::Public, None, "/runtime");
    let private_module = create_dynamic_module(EnvKind::Private, None, "/runtime");

    assert_eq!(
        public_module,
        "export { public_env as env } from '/runtime/shared-server.js';"
    );
    assert_eq!(
        private_module,
        "export { private_env as env } from '/runtime/shared-server.js';"
    );
}

#[test]
fn validates_js_identifiers_like_upstream() {
    assert!(is_valid_identifier("PUBLIC_FOO"));
    assert!(is_valid_identifier("_private"));
    assert!(is_valid_identifier("$dollar"));
    assert!(!is_valid_identifier(""));
    assert!(!is_valid_identifier("1BAD"));
    assert!(!is_valid_identifier("bad-key"));
}
