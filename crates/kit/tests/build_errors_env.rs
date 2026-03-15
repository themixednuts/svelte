use svelte_kit::{
    Error, ViteGuardError, browser_import_guard_error, service_worker_import_guard_error,
};

#[test]
fn private_env_imports_are_rejected_in_browser_code() {
    for normalized in ["$env/dynamic/private", "$env/static/private"] {
        let error =
            browser_import_guard_error(normalized, &[normalized, "src/routes/+page.svelte"]);
        let message = error.to_string();
        assert!(message.contains(&format!(
            "Cannot import {normalized} into code that runs in the browser"
        )));
        assert!(message.contains("If you're only using the import as a type"));
        assert!(matches!(
            error,
            Error::ViteGuard(ViteGuardError::BrowserImport { normalized: value, .. })
            if value == normalized
        ));
    }
}

#[test]
fn service_worker_env_restrictions_match_upstream_wording() {
    assert_eq!(
        service_worker_import_guard_error("$env/dynamic/private").to_string(),
        "Cannot import $env/dynamic/private into service-worker code. Only the modules $service-worker and $env/static/public are available in service workers."
    );
    assert_eq!(
        service_worker_import_guard_error("$env/dynamic/public").to_string(),
        "Cannot import $env/dynamic/public into service-worker code. Only the modules $service-worker and $env/static/public are available in service workers."
    );
}
