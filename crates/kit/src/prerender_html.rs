use crate::escape_html_with_mode;

pub fn render_http_equiv_meta_tag(name: &str, content: &str) -> String {
    format!(
        "<meta http-equiv=\"{}\" content=\"{}\">",
        name,
        escape_html_with_mode(content, true)
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    )
}

pub fn relative_service_worker_path(page_path: &str) -> String {
    let trimmed = page_path.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return "./service-worker.js".to_string();
    }

    let depth = trimmed.split('/').count().saturating_sub(1);
    format!(
        "{}{service}",
        "../".repeat(depth),
        service = "service-worker.js"
    )
}

pub fn render_service_worker_registration(page_path: &str) -> String {
    format!(
        "navigator.serviceWorker.register('{}')",
        relative_service_worker_path(page_path)
    )
}
