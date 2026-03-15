use svelte_kit::{
    Csp, Error, RuntimeCspConfig, RuntimeCspDirectives, RuntimeCspError, RuntimeCspMode,
    RuntimeCspOptions,
};

fn directives(
    entries: impl IntoIterator<Item = (&'static str, Vec<&'static str>)>,
) -> RuntimeCspDirectives {
    RuntimeCspDirectives::new(entries)
}

#[test]
fn generates_blank_csp_header() {
    let csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Hash,
            directives: directives([]),
            report_only: directives([]),
        },
        RuntimeCspOptions {
            prerender: false,
            dev: false,
        },
    )
    .expect("blank csp");

    assert_eq!(csp.csp_provider.get_header(), "");
    assert_eq!(csp.report_only_provider.get_header(), "");
}

#[test]
fn generates_csp_header_with_directive() {
    let csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Hash,
            directives: directives([("default-src", vec!["self"])]),
            report_only: directives([("default-src", vec!["self"]), ("report-uri", vec!["/"])]),
        },
        RuntimeCspOptions {
            prerender: false,
            dev: false,
        },
    )
    .expect("directive csp");

    assert_eq!(csp.csp_provider.get_header(), "default-src 'self'");
    assert_eq!(
        csp.report_only_provider.get_header(),
        "default-src 'self'; report-uri /"
    );
}

#[test]
fn generates_nonce_script_headers() {
    let mut csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Nonce,
            directives: directives([("default-src", vec!["self"])]),
            report_only: directives([("default-src", vec!["self"]), ("report-uri", vec!["/"])]),
        },
        RuntimeCspOptions {
            prerender: false,
            dev: false,
        },
    )
    .expect("nonce csp");

    csp.add_script("");

    assert!(
        csp.csp_provider
            .get_header()
            .starts_with("default-src 'self'; script-src 'self' 'nonce-")
    );
    assert!(
        csp.report_only_provider
            .get_header()
            .starts_with("default-src 'self'; report-uri /; script-src 'self' 'nonce-")
    );
}

#[test]
fn skips_nonce_when_unsafe_inline_is_present() {
    let mut csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Nonce,
            directives: directives([
                ("default-src", vec!["unsafe-inline"]),
                ("script-src", vec!["unsafe-inline"]),
                ("script-src-elem", vec!["unsafe-inline"]),
                ("style-src", vec!["unsafe-inline"]),
                ("style-src-attr", vec!["unsafe-inline"]),
                ("style-src-elem", vec!["unsafe-inline"]),
            ]),
            report_only: directives([
                ("default-src", vec!["unsafe-inline"]),
                ("script-src", vec!["unsafe-inline"]),
                ("script-src-elem", vec!["unsafe-inline"]),
                ("style-src", vec!["unsafe-inline"]),
                ("style-src-attr", vec!["unsafe-inline"]),
                ("style-src-elem", vec!["unsafe-inline"]),
                ("report-uri", vec!["/"]),
            ]),
        },
        RuntimeCspOptions {
            prerender: false,
            dev: false,
        },
    )
    .expect("unsafe-inline csp");

    csp.add_script("");
    csp.add_style("");

    assert_eq!(
        csp.csp_provider.get_header(),
        "default-src 'unsafe-inline'; script-src 'unsafe-inline'; script-src-elem 'unsafe-inline'; style-src 'unsafe-inline'; style-src-attr 'unsafe-inline'; style-src-elem 'unsafe-inline'"
    );
    assert_eq!(
        csp.report_only_provider.get_header(),
        "default-src 'unsafe-inline'; script-src 'unsafe-inline'; script-src-elem 'unsafe-inline'; style-src 'unsafe-inline'; style-src-attr 'unsafe-inline'; style-src-elem 'unsafe-inline'; report-uri /"
    );
}

#[test]
fn adds_unsafe_inline_styles_in_dev() {
    let mut csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Hash,
            directives: directives([
                ("default-src", vec!["self"]),
                (
                    "style-src-attr",
                    vec![
                        "self",
                        "sha256-9OlNO0DNEeaVzHL4RZwCLsBHA8WBQ8toBp/4F5XV2nc=",
                    ],
                ),
                (
                    "style-src-elem",
                    vec![
                        "self",
                        "sha256-9OlNO0DNEeaVzHL4RZwCLsBHA8WBQ8toBp/4F5XV2nc=",
                    ],
                ),
            ]),
            report_only: directives([
                ("default-src", vec!["self"]),
                ("style-src-attr", vec!["self"]),
                ("style-src-elem", vec!["self"]),
                ("report-uri", vec!["/"]),
            ]),
        },
        RuntimeCspOptions {
            prerender: false,
            dev: true,
        },
    )
    .expect("dev csp");

    csp.add_style("");

    assert_eq!(
        csp.csp_provider.get_header(),
        "default-src 'self'; style-src-attr 'self' 'unsafe-inline'; style-src-elem 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'"
    );
    assert_eq!(
        csp.report_only_provider.get_header(),
        "default-src 'self'; style-src-attr 'self' 'unsafe-inline'; style-src-elem 'self' 'unsafe-inline'; report-uri /; style-src 'self' 'unsafe-inline'"
    );
}

#[test]
fn preserves_empty_comment_hash_dedup_for_style_src_elem() {
    let mut csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Hash,
            directives: directives([(
                "style-src-elem",
                vec![
                    "self",
                    "sha256-9OlNO0DNEeaVzHL4RZwCLsBHA8WBQ8toBp/4F5XV2nc=",
                ],
            )]),
            report_only: directives([("report-uri", vec!["/"])]),
        },
        RuntimeCspOptions {
            prerender: false,
            dev: false,
        },
    )
    .expect("style elem csp");

    csp.add_style("/* empty */");

    assert_eq!(
        csp.csp_provider.get_header(),
        "style-src-elem 'self' 'sha256-9OlNO0DNEeaVzHL4RZwCLsBHA8WBQ8toBp/4F5XV2nc='"
    );
}

#[test]
fn skips_meta_unsupported_directives() {
    let csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Hash,
            directives: directives([
                ("default-src", vec!["self"]),
                ("frame-ancestors", vec!["self"]),
                ("report-uri", vec!["/csp-violation-report-endpoint/"]),
                ("sandbox", vec!["allow-modals"]),
            ]),
            report_only: directives([]),
        },
        RuntimeCspOptions {
            prerender: false,
            dev: false,
        },
    )
    .expect("meta csp");

    assert_eq!(
        csp.csp_provider.get_header(),
        "default-src 'self'; frame-ancestors 'self'; report-uri /csp-violation-report-endpoint/; sandbox allow-modals"
    );
    assert_eq!(
        csp.csp_provider.get_meta().as_deref(),
        Some("<meta http-equiv=\"content-security-policy\" content=\"default-src 'self'\">")
    );
}

#[test]
fn auto_mode_switches_between_nonces_and_hashes() {
    let mut nonce_csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Auto,
            directives: directives([
                ("script-src-elem", vec!["self"]),
                ("style-src-attr", vec!["self"]),
                ("style-src-elem", vec!["self"]),
            ]),
            report_only: directives([]),
        },
        RuntimeCspOptions {
            prerender: false,
            dev: false,
        },
    )
    .expect("auto nonce csp");
    nonce_csp.add_script("");
    nonce_csp.add_style("");
    let nonce_header = nonce_csp.csp_provider.get_header();
    assert!(nonce_header.contains("script-src-elem 'self' 'nonce-"));
    assert!(nonce_header.contains("style-src-attr 'self' 'nonce-"));
    assert!(nonce_header.contains(
        "style-src-elem 'self' 'sha256-9OlNO0DNEeaVzHL4RZwCLsBHA8WBQ8toBp/4F5XV2nc=' 'nonce-"
    ));

    let mut hash_csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Auto,
            directives: directives([
                ("script-src-elem", vec!["self"]),
                ("style-src-attr", vec!["self"]),
                ("style-src-elem", vec!["self"]),
            ]),
            report_only: directives([]),
        },
        RuntimeCspOptions {
            prerender: true,
            dev: false,
        },
    )
    .expect("auto hash csp");
    hash_csp.add_script("");
    hash_csp.add_style("");
    let hash_header = hash_csp.csp_provider.get_header();
    assert!(
        hash_header.contains(
            "script-src-elem 'self' 'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU='"
        )
    );
    assert!(
        hash_header.contains(
            "style-src-attr 'self' 'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU='"
        )
    );
}

#[test]
fn validates_report_only_requirements() {
    let error = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Hash,
            directives: directives([]),
            report_only: directives([("script-src", vec!["self"])]),
        },
        RuntimeCspOptions {
            prerender: false,
            dev: false,
        },
    )
    .expect_err("report-only directives without sink should fail");

    assert!(matches!(
        error,
        Error::RuntimeCsp(RuntimeCspError::MissingReportOnlySink)
    ));
    assert_eq!(
        error.to_string(),
        "`content-security-policy-report-only` must be specified with either the `report-to` or `report-uri` directives, or both"
    );
}

#[test]
fn adds_and_deduplicates_script_hashes() {
    let mut csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Hash,
            directives: directives([("script-src", vec!["self"])]),
            report_only: directives([]),
        },
        RuntimeCspOptions {
            prerender: true,
            dev: false,
        },
    )
    .expect("hash csp");

    csp.add_script_hashes(&["sha256-abc123"]);
    csp.add_script_hashes(&["sha256-abc123", "sha256-def456"]);

    let header = csp.csp_provider.get_header();
    assert_eq!(header.matches("'sha256-abc123'").count(), 1);
    assert!(header.contains("'sha256-def456'"));
    assert!(csp.script_needs_hash());
    assert!(!csp.script_needs_nonce());
}

#[test]
fn strict_dynamic_still_requires_script_nonce_but_not_style_nonce() {
    let mut script_csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Nonce,
            directives: directives([("script-src", vec!["strict-dynamic", "unsafe-inline"])]),
            report_only: directives([]),
        },
        RuntimeCspOptions {
            prerender: false,
            dev: false,
        },
    )
    .expect("strict dynamic csp");
    script_csp.add_script("");
    let script_header = script_csp.csp_provider.get_header();
    assert!(script_header.contains("'nonce-"));
    assert!(script_header.contains("'strict-dynamic'"));
    assert!(script_header.contains("'unsafe-inline'"));

    let mut style_csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Nonce,
            directives: directives([("style-src", vec!["strict-dynamic", "unsafe-inline"])]),
            report_only: directives([]),
        },
        RuntimeCspOptions {
            prerender: false,
            dev: false,
        },
    )
    .expect("strict dynamic style csp");
    style_csp.add_style("");
    let style_header = style_csp.csp_provider.get_header();
    assert!(!style_header.contains("'nonce-"));
    assert!(style_header.contains("'strict-dynamic'"));
    assert!(style_header.contains("'unsafe-inline'"));
}
