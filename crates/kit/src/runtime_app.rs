pub fn resolve_app_module(specifier: &str) -> Option<&'static str> {
    match specifier {
        "$app/environment" => Some("runtime/app/environment/index.js"),
        "$app/forms" => Some("runtime/app/forms.js"),
        "$app/navigation" => Some("runtime/app/navigation.js"),
        "$app/paths" => Some("runtime/app/paths/index.js"),
        "$app/server" => Some("runtime/app/server/index.js"),
        "$app/state" => Some("runtime/app/state/index.js"),
        "$app/stores" => Some("runtime/app/stores.js"),
        _ => None,
    }
}
