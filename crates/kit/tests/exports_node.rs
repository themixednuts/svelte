use http::{Method, Request};
use svelte_kit::{
    Error, ExportsNodeError, create_readable_stream, get_node_request, set_node_response,
};

#[test]
fn get_node_request_ignores_get_bodies() {
    let request = Request::builder()
        .method(Method::GET)
        .uri("https://example.com/test")
        .header("content-type", "text/plain")
        .body(Some(b"ignored".to_vec()))
        .expect("http request should build");

    let request = get_node_request(request, None).expect("request should convert");
    assert_eq!(request.request.method.as_str(), "GET");
    assert!(request.body.is_none());
}

#[test]
fn get_node_request_enforces_body_limit() {
    let request = Request::builder()
        .method(Method::POST)
        .uri("https://example.com/test")
        .header("content-type", "application/json")
        .body(Some(br#"{"x":1}"#.to_vec()))
        .expect("http request should build");

    let error = get_node_request(request, Some(3)).expect_err("body limit should fail");
    assert!(error.to_string().contains("exceeds limit"));
    assert!(matches!(
        error,
        Error::ExportsNode(ExportsNodeError::BodyLimitExceeded { length, limit })
        if length == br#"{"x":1}"#.len() && limit == 3
    ));
}

#[test]
fn set_node_response_round_trips_server_response() {
    let mut response = svelte_kit::ServerResponse::new(201);
    response.set_header("content-type", "text/plain");
    response.body = Some("created".to_string());

    let http = set_node_response(&response).expect("response should convert");
    assert_eq!(http.status().as_u16(), 201);
    assert_eq!(http.headers()["content-type"], "text/plain");
    assert_eq!(http.body().as_deref(), Some("created"));
}

#[test]
fn create_readable_stream_opens_file_contents() {
    let root = std::env::temp_dir().join("svelte-kit-node-stream.txt");
    std::fs::write(&root, "stream me").expect("temp file should be written");

    let mut file = create_readable_stream(&root).expect("stream should open");
    let mut contents = String::new();
    use std::io::Read;
    file.read_to_string(&mut contents)
        .expect("file should be readable");
    assert_eq!(contents, "stream me");

    std::fs::remove_file(root).expect("temp file should be removed");
}
