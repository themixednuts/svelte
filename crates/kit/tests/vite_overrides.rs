use serde_json::json;
use svelte_kit::{find_overridden_vite_config, warn_overridden_vite_config};

#[test]
fn finds_only_enforced_vite_config_overrides() {
    let config = json!({
        "root": "custom-root",
        "build": {
            "cssCodeSplit": false,
            "outDir": "dist",
            "rollupOptions": {
                "output": {
                    "format": "cjs"
                }
            }
        },
        "resolve": {
            "alias": {
                "$app": "/custom/app",
                "@pkg": "/custom/pkg"
            }
        },
        "server": {
            "port": 5173
        }
    });

    let resolved = json!({
        "root": ".",
        "build": {
            "cssCodeSplit": true,
            "outDir": ".svelte-kit/output",
            "rollupOptions": {
                "output": {
                    "format": "esm"
                }
            }
        },
        "resolve": {
            "alias": {
                "$app": "/runtime/app",
                "@pkg": "/custom/pkg"
            }
        },
        "server": {
            "port": 4173
        }
    });

    assert_eq!(
        find_overridden_vite_config(&config, &resolved),
        vec![
            "build.cssCodeSplit".to_string(),
            "build.outDir".to_string(),
            "build.rollupOptions.output.format".to_string(),
            "resolve.alias.$app".to_string(),
            "root".to_string(),
        ]
    );
}

#[test]
fn formats_vite_override_warning_message() {
    let warning =
        warn_overridden_vite_config(&json!({ "root": "custom-root" }), &json!({ "root": "." }))
            .expect("warning should exist");

    assert!(warning.contains("The following Vite config options will be overridden by SvelteKit:"));
    assert!(warning.contains("\n  - root"));
}

#[test]
fn returns_no_warning_when_enforced_values_match() {
    assert_eq!(
        warn_overridden_vite_config(
            &json!({ "root": ".", "build": { "outDir": ".svelte-kit/output" } }),
            &json!({ "root": ".", "build": { "outDir": ".svelte-kit/output" } }),
        ),
        None
    );
}
