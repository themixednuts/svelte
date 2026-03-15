use svelte_kit::{BINARY_FORM_CONTENT_TYPE, is_form_content_type, negotiate};

#[test]
fn negotiates_valid_accept_headers() {
    assert_eq!(negotiate("text/html", &["text/html"]), Some("text/html"));
}

#[test]
fn negotiates_accept_headers_with_optional_whitespace() {
    let accept = "application/some-thing-else, \tapplication/json \t; q=0.9  ,text/plain;q=0.1";
    assert_eq!(
        negotiate(accept, &["application/json", "text/plain"]),
        Some("application/json")
    );
}

#[test]
fn ignores_invalid_accept_header_parts() {
    assert_eq!(negotiate("text/html,*", &["text/html"]), Some("text/html"));
}

#[test]
fn detects_form_content_types_case_insensitively() {
    assert!(is_form_content_type(Some(
        "Multipart/Form-Data; boundary=example"
    )));
    assert!(is_form_content_type(Some(BINARY_FORM_CONTENT_TYPE)));
    assert!(!is_form_content_type(Some("application/json")));
}
