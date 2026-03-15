pub fn fallback_page_filename(fallback: &str) -> String {
    fallback.to_string()
}

pub fn public_asset_output_path(_app_dir: &str, asset: &str) -> String {
    asset.trim_start_matches('/').to_string()
}

pub fn should_prerender_linked_server_route(prerender: Option<bool>) -> bool {
    prerender == Some(true)
}
