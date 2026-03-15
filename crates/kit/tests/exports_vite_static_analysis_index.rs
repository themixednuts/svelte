use serde_json::{Value, json};
use svelte_kit::statically_analyze_vite_page_options;

fn analyze(input: &str) -> Option<Value> {
    let options = statically_analyze_vite_page_options(input)?;
    Some(json!(options))
}

#[test]
fn page_option_literals_are_analyzed() {
    let cases = [
        r#"
            export const ssr = false;
            export const csr = true;
            export const prerender = 'auto';
            export const trailingSlash = 'always';
        "#,
        r#"
            export const ssr = false, csr = true, prerender = 'auto', trailingSlash = 'always';
        "#,
    ];

    for input in cases {
        assert_eq!(
            analyze(input),
            Some(json!({
                "ssr": false,
                "csr": true,
                "prerender": "auto",
                "trailingSlash": "always"
            }))
        );
    }
}

#[test]
fn dynamic_page_options_return_none() {
    let cases = [
        r#"
            export const ssr = process.env.SSR;
            export const prerender = true;
        "#,
        r#"
            export const ssr = false;
            export const config = {
                runtime: 'edge'
            };
        "#,
        r#"
            export const prerender = true;
            export const entries = () => {
                return [{ slug: 'foo' }];
            };
        "#,
        "export * as ssr from './foo';",
    ];

    for input in cases {
        assert_eq!(analyze(input), None, "input: {input}");
    }
}

#[test]
fn non_page_exports_are_ignored_and_export_all_fails() {
    let ignored = [
        "export let _foo = 'bar';",
        "export * as bar from './foo';",
        "export const foo = 'bar';",
    ];

    for input in ignored {
        assert_eq!(analyze(input), Some(json!({})), "input: {input}");
    }

    let failing = [
        "export * from './foo';",
        "export\n*\nfrom\n'./foo';",
        "export    *      from \"./foo\";",
        "export   \n  *\n   from 'abc';  ",
    ];

    for input in failing {
        assert_eq!(analyze(input), None, "input: {input}");
    }
}

#[test]
fn exported_specifiers_and_load_are_handled_like_upstream() {
    let specifier_cases = [
        (
            r#"
                let ssr = false;
                export { ssr };
            "#,
            Some(json!({ "ssr": false })),
        ),
        (
            r#"
                export let foo = false;
                export { foo as ssr };
            "#,
            Some(json!({ "ssr": false })),
        ),
        (
            r#"
                import { ssr } from './foo';
                export { ssr };
            "#,
            None,
        ),
        (
            r#"
                import ssr from './foo';
                export { ssr };
            "#,
            None,
        ),
        (
            r#"
                import * as ssr from './foo';
                export { ssr };
            "#,
            None,
        ),
    ];

    for (input, expected) in specifier_cases {
        assert_eq!(analyze(input), expected, "input: {input}");
    }

    assert_eq!(
        analyze("export async function load () { return {} }"),
        Some(json!({ "load": null }))
    );
    assert_eq!(
        analyze("export const load = () => { return {} }"),
        Some(json!({ "load": null }))
    );
    assert_eq!(
        analyze(
            r#"
                export const load = () => { return {} };
                export const ssr = process.env.SSR;
            "#
        ),
        None
    );
}
