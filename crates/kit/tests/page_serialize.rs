use std::collections::BTreeMap;

use http::{Method, StatusCode};
use svelte_kit::{FetchedResponse, serialize_data};

fn fetched(body: &str) -> FetchedResponse {
    FetchedResponse {
        url: "foo".to_string(),
        method: Method::GET,
        request_body: None,
        request_headers: None,
        response_body: body.to_string(),
        response_status: StatusCode::OK,
        response_status_text: String::new(),
        response_headers: BTreeMap::new(),
        is_b64: false,
    }
}

#[test]
fn serializes_fetched_data_like_upstream() {
    let response_body = "</script><script>alert(\"xss\")";
    assert_eq!(
        serialize_data(&fetched(response_body), |_, _| false, false),
        "<script type=\"application/json\" data-sveltekit-fetched data-url=\"foo\">\
{\"status\":200,\"statusText\":\"\",\"headers\":{},\"body\":\"\\u003C/script>\\u003Cscript>alert(\\\"xss\\\")\"}\
</script>"
    );
}

#[test]
fn escapes_html_comments_like_upstream() {
    let response_body = "<!--</script>...-->alert(\"xss\")";
    assert_eq!(
        serialize_data(&fetched(response_body), |_, _| false, false),
        "<script type=\"application/json\" data-sveltekit-fetched data-url=\"foo\">\
{\"status\":200,\"statusText\":\"\",\"headers\":{},\"body\":\"\\u003C!--\\u003C/script>...-->alert(\\\"xss\\\")\"}\
</script>"
    );
}

#[test]
fn escapes_attribute_values_like_upstream() {
    let response_body = "";
    let mut fetched = fetched(response_body);
    fetched.url = "an \"attr\" & a".to_string();

    assert_eq!(
        serialize_data(&fetched, |_, _| false, false),
        "<script type=\"application/json\" data-sveltekit-fetched data-url=\"an &quot;attr&quot; &amp; a\">\
{\"status\":200,\"statusText\":\"\",\"headers\":{},\"body\":\"\"}\
</script>"
    );
}

#[test]
fn computes_ttl_from_cache_control_and_age_headers() {
    let mut fetched = fetched("");
    fetched.url = "an \"attr\" & a".to_string();
    fetched
        .response_headers
        .insert("cache-control".to_string(), "max-age=10".to_string());
    fetched
        .response_headers
        .insert("age".to_string(), "1".to_string());

    assert_eq!(
        serialize_data(&fetched, |_, _| false, false),
        "<script type=\"application/json\" data-sveltekit-fetched data-url=\"an &quot;attr&quot; &amp; a\" data-ttl=\"9\">\
{\"status\":200,\"statusText\":\"\",\"headers\":{},\"body\":\"\"}\
</script>"
    );
}

#[test]
fn skips_ttl_when_vary_star_is_present() {
    let mut fetched = fetched("");
    fetched.url = "an \"attr\" & a".to_string();
    fetched
        .response_headers
        .insert("cache-control".to_string(), "max-age=10".to_string());
    fetched
        .response_headers
        .insert("vary".to_string(), "*".to_string());

    assert_eq!(
        serialize_data(&fetched, |_, _| false, false),
        "<script type=\"application/json\" data-sveltekit-fetched data-url=\"an &quot;attr&quot; &amp; a\">\
{\"status\":200,\"statusText\":\"\",\"headers\":{},\"body\":\"\"}\
</script>"
    );
}

#[test]
fn serializes_base64_fetched_payloads() {
    let mut fetched = fetched("AAEC");
    fetched.is_b64 = true;

    assert_eq!(
        serialize_data(&fetched, |_, _| false, false),
        "<script type=\"application/json\" data-sveltekit-fetched data-url=\"foo\" data-b64>\
{\"status\":200,\"statusText\":\"\",\"headers\":{},\"body\":\"AAEC\"}\
</script>"
    );
}
