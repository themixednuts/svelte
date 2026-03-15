use svelte_kit::browser_import_guard_error;

#[test]
fn server_only_modules_are_rejected_in_browser_code() {
    for normalized in ["$lib/test.server.js", "$lib/server/something/private.js"] {
        let message =
            browser_import_guard_error(normalized, &[normalized, "src/routes/+page.svelte"])
                .to_string();
        assert!(message.contains(&format!(
            "Cannot import {normalized} into code that runs in the browser"
        )));
        assert!(message.contains("If you're only using the import as a type"));
    }
}
