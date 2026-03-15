const DATA_SUFFIX: &str = "/__data.json";
const HTML_DATA_SUFFIX: &str = ".html__data.json";
const ROUTE_SUFFIX: &str = "/__route.js";

pub fn has_data_suffix(pathname: &str) -> bool {
    pathname.ends_with(DATA_SUFFIX) || pathname.ends_with(HTML_DATA_SUFFIX)
}

pub fn add_data_suffix(pathname: &str) -> String {
    if pathname.ends_with(".html") {
        return pathname.replace(".html", HTML_DATA_SUFFIX);
    }

    format!("{}{}", pathname.trim_end_matches('/'), DATA_SUFFIX)
}

pub fn strip_data_suffix(pathname: &str) -> String {
    if pathname.ends_with(HTML_DATA_SUFFIX) {
        return format!(
            "{}.html",
            &pathname[..pathname.len() - HTML_DATA_SUFFIX.len()]
        );
    }

    pathname[..pathname.len() - DATA_SUFFIX.len()].to_string()
}

pub fn has_resolution_suffix(pathname: &str) -> bool {
    pathname.ends_with(ROUTE_SUFFIX)
}

pub fn add_resolution_suffix(pathname: &str) -> String {
    format!("{}{}", pathname.trim_end_matches('/'), ROUTE_SUFFIX)
}

pub fn strip_resolution_suffix(pathname: &str) -> String {
    pathname[..pathname.len() - ROUTE_SUFFIX.len()].to_string()
}
