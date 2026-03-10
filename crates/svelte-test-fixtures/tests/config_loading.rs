use svelte_test_fixtures::{detect_repo_root, discover_suite_cases_by_name, load_test_config};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MinimalConfig {
    #[serde(default)]
    compile_options: Option<MinimalCompileOptions>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MinimalCompileOptions {
    #[serde(default)]
    runes: Option<bool>,
    #[serde(default)]
    css_hash: Option<String>,
}

#[test]
fn await_fixture_config_with_getter_loads() {
    let repo_root = detect_repo_root().expect("detect repo root");
    let cases = discover_suite_cases_by_name(&repo_root, "runtime-legacy")
        .expect("discover runtime-legacy fixture cases");
    let case = cases
        .into_iter()
        .find(|case| case.name == "await-then-catch")
        .expect("await-then-catch fixture exists");

    let config =
        load_test_config::<serde::de::IgnoredAny>(&case).expect("load _config.js should succeed");
    assert!(
        config.is_some(),
        "_config.js should produce a config object"
    );
}

#[test]
fn runtime_runes_ambiguous_config_allows_undefined_runes() {
    let repo_root = detect_repo_root().expect("detect repo root");
    let cases = discover_suite_cases_by_name(&repo_root, "runtime-runes")
        .expect("discover runtime-runes fixture cases");
    let case = cases
        .into_iter()
        .find(|case| case.name == "legacy-runes-ambiguous")
        .expect("legacy-runes-ambiguous fixture exists");

    let config = load_test_config::<MinimalConfig>(&case)
        .expect("load _config.js should succeed")
        .expect("_config.js should produce a config object");

    assert_eq!(
        config.compile_options.and_then(|options| options.runes),
        None
    );
}

#[test]
fn runtime_legacy_target_dom_config_evaluates_zero_arg_css_hash() {
    let repo_root = detect_repo_root().expect("detect repo root");
    let cases = discover_suite_cases_by_name(&repo_root, "runtime-legacy")
        .expect("discover runtime-legacy fixture cases");
    let case = cases
        .into_iter()
        .find(|case| case.name == "target-dom")
        .expect("target-dom fixture exists");

    let config = load_test_config::<MinimalConfig>(&case)
        .expect("load _config.js should succeed")
        .expect("_config.js should produce a config object");

    assert_eq!(
        config.compile_options.and_then(|options| options.css_hash),
        Some(String::from("svelte-xyz"))
    );
}
