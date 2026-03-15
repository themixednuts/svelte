use http::{Method, StatusCode};
use svelte_kit::{Error, RuntimeHttpError, ServerRequest, ServerResponse};
use url::Url;

#[test]
fn server_request_builder_round_trips_through_http_types() {
    let request = ServerRequest::builder()
        .method(Method::POST)
        .url(Url::parse("https://example.com/base/path?q=1").expect("url"))
        .header("Content-Type", "application/json")
        .expect("header")
        .header("X-Test", "value")
        .expect("header")
        .build()
        .expect("request");

    assert_eq!(request.header("content-type"), Some("application/json"));

    let http_request = request.to_http_request().expect("http request");
    assert_eq!(http_request.method(), Method::POST);
    assert_eq!(
        http_request.uri().to_string(),
        "https://example.com/base/path?q=1"
    );
    assert_eq!(http_request.headers()["content-type"], "application/json");

    let round_trip = ServerRequest::try_from(http_request).expect("round trip");
    assert_eq!(round_trip, request);
}

#[test]
fn server_response_builder_round_trips_through_http_types() {
    let response = ServerResponse::builder(201)
        .header("Content-Type", "text/plain; charset=utf-8")
        .expect("header")
        .append_header("Set-Cookie", "a=1; Path=/")
        .expect("header")
        .append_header("Set-Cookie", "b=2; Path=/")
        .expect("header")
        .body("created")
        .build()
        .expect("response");

    let http_response = response.to_http_response().expect("http response");
    assert_eq!(http_response.status(), StatusCode::CREATED);
    assert_eq!(
        http_response.headers()["content-type"],
        "text/plain; charset=utf-8"
    );
    assert_eq!(http_response.body().as_deref(), Some("created"));

    let round_trip = ServerResponse::try_from(http_response).expect("round trip");
    assert_eq!(round_trip, response);
}

#[test]
fn invalid_header_names_are_rejected_by_builders() {
    let request_error = ServerRequest::builder()
        .url(Url::parse("https://example.com/").expect("url"))
        .header("bad header", "value")
        .expect_err("invalid request header");
    assert!(matches!(
        request_error,
        Error::RuntimeHttp(RuntimeHttpError::InvalidHeaderName { .. })
    ));
    assert!(request_error.to_string().contains("invalid header name"));

    let response_error = ServerResponse::builder(200)
        .header("bad header", "value")
        .expect_err("invalid response header");
    assert!(matches!(
        response_error,
        Error::RuntimeHttp(RuntimeHttpError::InvalidHeaderName { .. })
    ));
    assert!(response_error.to_string().contains("invalid header name"));
}

#[test]
fn server_response_accessors_return_normalized_headers() {
    let response = ServerResponse::builder(200)
        .header("Content-Type", "text/plain")
        .expect("header")
        .append_header("Set-Cookie", "a=1; Path=/")
        .expect("header")
        .append_header("Set-Cookie", "b=2; Path=/")
        .expect("header")
        .build()
        .expect("response");

    assert_eq!(response.header("content-type"), Some("text/plain"));
    assert!(response.has_header("set-cookie"));
    assert_eq!(
        response.header_values("set-cookie"),
        Some(vec!["a=1; Path=/", "b=2; Path=/"])
    );
}
