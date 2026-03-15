pub const SVELTE_KIT_ASSETS: &str = "/_svelte_kit_assets";
pub const GENERATED_COMMENT: &str = "// this file is generated — do not edit it\n";
pub const MUTATIVE_METHODS: &[&str] = &["POST", "PUT", "PATCH", "DELETE"];
pub const PAGE_METHODS_PUBLIC: &[&str] = &["GET", "POST", "HEAD"];
pub const ENDPOINT_METHODS_PUBLIC: &[&str] =
    &["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"];

pub const fn endpoint_methods() -> &'static [&'static str] {
    ENDPOINT_METHODS_PUBLIC
}
