use svelte_kit::VERSION;

#[test]
fn version_matches_cargo_package_version() {
    assert_eq!(
        VERSION,
        env!("CARGO_PKG_VERSION"),
        "VERSION export does not equal Cargo package version"
    );
}
