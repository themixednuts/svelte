pub fn prerender_output_filename(base: &str, path: &str, is_html: bool) -> String {
    let file = path
        .strip_prefix(base)
        .unwrap_or(path)
        .trim_start_matches('/');
    let file = if file.is_empty() { "index.html" } else { file };

    if is_html && !file.ends_with(".html") {
        return if file.ends_with('/') {
            format!("{file}index.html")
        } else {
            format!("{file}.html")
        };
    }

    file.to_string()
}

pub fn prepend_base_path(base: &str, path: &str) -> String {
    if path.is_empty()
        || !path.starts_with('/')
        || path.starts_with("//")
        || (path.len() > 1 && path.as_bytes()[1] == b':')
    {
        return path.to_string();
    }

    if base.is_empty() || path == base || path.starts_with(&format!("{base}/")) {
        path.to_string()
    } else {
        format!("{base}{path}")
    }
}
