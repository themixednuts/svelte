use crate::escape_html_with_mode;

pub fn render_prerender_redirect_html(location: &str) -> String {
    let script_location = serde_json::to_string(location)
        .expect("string locations always serialize")
        .replace('<', "\\u003C")
        .replace('>', "\\u003E");
    let refresh_location = escape_html_with_mode(&format!("0;url={location}"), true)
        .replace('<', "&lt;")
        .replace('>', "&gt;");

    format!(
        "<script>location.href={script_location};</script><meta http-equiv=\"refresh\" content=\"{refresh_location}\">"
    )
}

pub fn service_worker_prerender_paths(base: &str, prerendered_paths: &[String]) -> Vec<String> {
    prerendered_paths
        .iter()
        .map(|path| {
            let stripped = path.strip_prefix(base).unwrap_or(path);
            format!("base + {:?}", stripped)
        })
        .collect()
}

pub fn serialize_missing_ids_jsonl(ids: &[&str]) -> String {
    ids.iter()
        .map(|id| serde_json::to_string(id).expect("missing id values always serialize") + ",")
        .collect()
}
