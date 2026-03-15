use serde_json::json;
use svelte_kit::statically_analyze_page_options;

#[test]
fn analyzes_literal_page_options_exports() {
    let options = statically_analyze_page_options(
        r#"
        export const ssr = false;
        export const csr = true;
        export const prerender = "auto";
        export const trailingSlash = 'always';
        "#,
    )
    .expect("literal exports should analyze");

    assert_eq!(options.get("ssr"), Some(&json!(false)));
    assert_eq!(options.get("csr"), Some(&json!(true)));
    assert_eq!(options.get("prerender"), Some(&json!("auto")));
    assert_eq!(options.get("trailingSlash"), Some(&json!("always")));
}

#[test]
fn analyzes_single_line_literal_page_options_exports() {
    let options = statically_analyze_page_options(
        r#"export const ssr = false, csr = true, prerender = "auto", trailingSlash = 'always';"#,
    )
    .expect("single-line literal exports should analyze");

    assert_eq!(options.get("ssr"), Some(&json!(false)));
    assert_eq!(options.get("csr"), Some(&json!(true)));
    assert_eq!(options.get("prerender"), Some(&json!("auto")));
    assert_eq!(options.get("trailingSlash"), Some(&json!("always")));
}

#[test]
fn analyzes_load_exports_without_treating_them_as_dynamic() {
    let options = statically_analyze_page_options(
        r#"
        export async function load() {
            return {};
        }
        "#,
    )
    .expect("load export should analyze");

    assert_eq!(options.get("load"), Some(&serde_json::Value::Null));
}

#[test]
fn returns_none_for_dynamic_page_option_exports() {
    let options = statically_analyze_page_options(
        r#"
        export const prerender = true;
        export const ssr = process.env.SSR;
        "#,
    );

    assert!(options.is_none());
}

#[test]
fn analyzes_object_page_option_exports() {
    let options = statically_analyze_page_options(
        r#"
        export const ssr = false;
        export const config = {
            runtime: "edge"
        };
        "#,
    );

    let options = options.expect("object exports should analyze");
    assert_eq!(options.get("ssr"), Some(&json!(false)));
    assert_eq!(options.get("config"), Some(&json!({ "runtime": "edge" })));
}

#[test]
fn returns_none_for_arrow_function_page_option_exports() {
    let options = statically_analyze_page_options(
        r#"
        export const prerender = true;
        export const entries = () => {
            return [{ slug: "foo" }];
        };
        "#,
    );

    assert!(options.is_none());
}

#[test]
fn analyzes_page_options_exported_via_specifiers() {
    let options = statically_analyze_page_options(
        r#"
        let ssr = false;
        export { ssr };
        "#,
    )
    .expect("export specifier should analyze");

    assert_eq!(options.get("ssr"), Some(&json!(false)));
}

#[test]
fn preserves_exported_let_page_options() {
    let options = statically_analyze_page_options(
        r#"
        export let ssr = true;
        export const prerender = true;
        "#,
    )
    .expect("exported let page options should analyze");

    assert_eq!(options.get("ssr"), Some(&json!(true)));
    assert_eq!(options.get("prerender"), Some(&json!(true)));
}

#[test]
fn returns_none_for_export_all_alias_page_options() {
    let options = statically_analyze_page_options("export * as ssr from './foo';");

    assert!(options.is_none());
}

#[test]
fn ignores_private_and_non_page_export_aliases() {
    let private = statically_analyze_page_options("export * as bar from './foo';")
        .expect("non-page export-all alias should be ignored");
    let named = statically_analyze_page_options("export { foo as bar };")
        .expect("non-page export alias should be ignored");

    assert!(private.is_empty());
    assert!(named.is_empty());
}

#[test]
fn returns_none_for_object_page_options_without_semicolons() {
    let options = statically_analyze_page_options(
        r#"
        export const ssr = false
        export const config = {
            runtime: 'edge'
        }
        "#,
    );

    let options = options.expect("object exports without semicolons should analyze");
    assert_eq!(options.get("ssr"), Some(&json!(false)));
    assert_eq!(options.get("config"), Some(&json!({ "runtime": "edge" })));
}

#[test]
fn ignores_export_all_aliases_for_non_page_options() {
    let options = statically_analyze_page_options(r#"export * as bar from "./foo";"#)
        .expect("non-page export-all alias should be ignored");

    assert!(options.is_empty());
}

#[test]
fn ignores_non_page_option_exports() {
    let private = statically_analyze_page_options("export let _foo = 'bar';")
        .expect("private export should be ignored");
    let plain = statically_analyze_page_options("export const foo = 'bar';")
        .expect("plain export should be ignored");

    assert!(private.is_empty());
    assert!(plain.is_empty());
}

#[test]
fn returns_none_for_export_all_declarations() {
    let options = statically_analyze_page_options(
        r#"
        export
        *
        from
        "./foo";
        "#,
    );

    assert!(options.is_none());
}

#[test]
fn analyzes_export_aliases_for_page_options() {
    let options = statically_analyze_page_options(
        r#"
        export let foo = false;
        export { foo as ssr };
        "#,
    )
    .expect("export alias should analyze");

    assert_eq!(options.get("ssr"), Some(&json!(false)));
}

#[test]
fn preserves_page_options_through_switch_shadowing() {
    let options = statically_analyze_page_options(
        r#"
        export let ssr = true;
        export const prerender = true;
        switch (ssr) {
            case true:
                let ssr = true;
                ssr = false;
                break;
        }
        "#,
    )
    .expect("switch shadowing should not invalidate page options");

    assert_eq!(options.get("ssr"), Some(&json!(true)));
    assert_eq!(options.get("prerender"), Some(&json!(true)));
}

#[test]
fn returns_none_for_imported_export_specifiers() {
    let options = statically_analyze_page_options(
        r#"
        import { ssr } from "./foo";
        export { ssr };
        "#,
    );

    assert!(options.is_none());
}

#[test]
fn returns_none_for_import_default_export_specifiers() {
    let options = statically_analyze_page_options(
        r#"
        import ssr from "./foo";
        export { ssr };
        "#,
    );

    assert!(options.is_none());
}

#[test]
fn returns_none_for_import_namespace_export_specifiers() {
    let options = statically_analyze_page_options(
        r#"
        import * as ssr from "./foo";
        export { ssr };
        "#,
    );

    assert!(options.is_none());
}

#[test]
fn returns_none_for_destructured_export_specifiers() {
    let options = statically_analyze_page_options(
        r#"
        let { ssr } = { ssr: false };
        export { ssr };
        "#,
    );

    assert!(options.is_none());
}

#[test]
fn analyzes_load_arrow_function_exports() {
    let options = statically_analyze_page_options(
        r#"
        export const load = () => {
            return {};
        };
        "#,
    )
    .expect("load arrow function should analyze");

    assert_eq!(options.get("load"), Some(&serde_json::Value::Null));
}

#[test]
fn returns_none_when_dynamic_export_coexists_with_load() {
    let options = statically_analyze_page_options(
        r#"
        export const load = () => {
            return {};
        };
        export const ssr = process.env.SSR;
        "#,
    );

    assert!(options.is_none());
}

#[test]
fn preserves_page_options_through_nested_shadowing() {
    let options = statically_analyze_page_options(
        r#"
        export let ssr = true;
        export const prerender = true;
        function foo() {
            let ssr = true;
            {
                ssr = false;
            }
        }
        "#,
    )
    .expect("nested shadowing should not invalidate page options");

    assert_eq!(options.get("ssr"), Some(&json!(true)));
    assert_eq!(options.get("prerender"), Some(&json!(true)));
}

#[test]
fn preserves_page_options_when_used_as_assignment_values() {
    let options = statically_analyze_page_options(
        r#"
        export let ssr = true;
        export const prerender = true;
        let csr;
        csr = ssr;
        "#,
    )
    .expect("using exports in assignments should not invalidate page options");

    assert_eq!(options.get("ssr"), Some(&json!(true)));
    assert_eq!(options.get("prerender"), Some(&json!(true)));
}
