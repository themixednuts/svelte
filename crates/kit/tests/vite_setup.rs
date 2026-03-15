use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use svelte_kit::{
    ViteAliasFind, ViteOptimizeRemoteFunctionsPlan, build_vite_setup_plan, validate_config,
};

fn repo_root() -> Utf8PathBuf {
    let manifest_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .find(|candidate| candidate.join("kit").join("packages").join("kit").is_dir())
        .expect("workspace root")
        .to_path_buf()
}

fn temp_dir(label: &str) -> Utf8PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    let dir = repo_root()
        .join("tmp")
        .join(format!("svelte-kit-vite-setup-{label}-{unique}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_file(path: &Utf8PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write file");
}

#[test]
fn builds_vite_setup_plan_for_dev() {
    let cwd = temp_dir("dev");
    write_file(&cwd.join("src").join("hooks.client.ts"), "export {};");
    let config = validate_config(
        &serde_json::json!({
            "extensions": [".svelte"],
            "kit": {
                "outDir": ".svelte-kit",
                "experimental": {
                    "remoteFunctions": true,
                    "forkPreloads": true,
                    "tracing": { "server": true }
                },
                "alias": {
                    "@pkg": "src/pkg"
                },
                "version": {
                    "name": "v1"
                }
            }
        }),
        &cwd,
    )
    .expect("config should validate");

    let plan = build_vite_setup_plan(
        &cwd,
        &config.kit,
        "runtime",
        "development",
        false,
        false,
        false,
        None,
        None,
    )
    .expect("setup plan should build");

    assert_eq!(plan.root, ".");
    assert!(plan.resolve_aliases.iter().any(|alias| {
        matches!(&alias.find, ViteAliasFind::Literal(value) if value == "__SERVER__")
            && alias.replacement.contains("generated")
            && alias.replacement.replace('\\', "/").ends_with("/server")
    }));
    assert!(plan.resolve_aliases.iter().any(|alias| {
        matches!(&alias.find, ViteAliasFind::Literal(value) if value == "$app")
            && alias.replacement == "runtime/app"
    }));
    assert!(plan.server_allow.iter().any(|entry| entry.ends_with("src")));
    assert!(plan.server_allow.iter().any(|entry| entry == ".svelte-kit"));
    assert!(plan.server_allow.iter().any(|entry| entry == "src"));
    assert_eq!(
        plan.optimize_entries,
        vec![
            "src/routes/**/+*.{svelte,js,ts}",
            "!src/routes/**/+*server.*"
        ]
    );
    assert!(plan.optimize_exclude.contains(&"@sveltejs/kit".to_string()));
    assert_eq!(plan.define["__SVELTEKIT_APP_DIR__"], "\"_app\"");
    assert_eq!(plan.define["__SVELTEKIT_APP_VERSION_POLL_INTERVAL__"], "0");
    assert_eq!(plan.external_in_dev, vec!["cookie", "set-cookie-parser"]);
    assert!(matches!(
        plan.remote_functions_optimizer,
        Some(ViteOptimizeRemoteFunctionsPlan::Esbuild { .. })
    ));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn builds_vite_setup_plan_for_secondary_build() {
    let cwd = temp_dir("build");
    let config = validate_config(
        &serde_json::json!({
            "kit": {
                "outDir": ".svelte-kit",
                "experimental": {
                    "remoteFunctions": true
                },
                "version": {
                    "name": "v1",
                    "pollInterval": 5000
                }
            }
        }),
        &cwd,
    )
    .expect("config should validate");

    let plan = build_vite_setup_plan(
        &cwd,
        &config.kit,
        "runtime",
        "production",
        true,
        true,
        true,
        Some(false),
        Some(true),
    )
    .expect("setup plan should build");

    assert!(!plan.build_ssr);
    assert_eq!(plan.define["__SVELTEKIT_HAS_SERVER_LOAD__"], "false");
    assert_eq!(plan.define["__SVELTEKIT_HAS_UNIVERSAL_LOAD__"], "true");
    assert_eq!(
        plan.define["__SVELTEKIT_APP_VERSION_POLL_INTERVAL__"],
        "5000"
    );
    assert!(matches!(
        plan.remote_functions_optimizer,
        Some(ViteOptimizeRemoteFunctionsPlan::Rolldown { .. })
    ));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
