use camino::Utf8PathBuf;
use serde_json::{Map, json};
use svelte_kit::{
    EnvKind, create_dynamic_module, data_json_response, decode_pathname, fallback_page_filename,
    prerender_output_filename, redirect_data_response, relative_service_worker_path,
    render_http_equiv_meta_tag, render_prerender_redirect_html, render_service_worker_registration,
    render_shell_page_response, resolve, serialize_missing_ids_jsonl,
    service_worker_prerender_paths, validate_config,
};

#[test]
fn renders_absolute_relative_and_encoded_redirects_like_upstream_prerender() {
    assert_eq!(
        render_prerender_redirect_html("https://example.com/redirected"),
        "<script>location.href=\"https://example.com/redirected\";</script><meta http-equiv=\"refresh\" content=\"0;url=https://example.com/redirected\">"
    );
    assert_eq!(
        render_prerender_redirect_html("/env"),
        "<script>location.href=\"/env\";</script><meta http-equiv=\"refresh\" content=\"0;url=/env\">"
    );
    assert_eq!(
        render_prerender_redirect_html(
            "https://example.com/redirected?returnTo=%2Ffoo%3Fbar%3Dbaz"
        ),
        "<script>location.href=\"https://example.com/redirected?returnTo=%2Ffoo%3Fbar%3Dbaz\";</script><meta http-equiv=\"refresh\" content=\"0;url=https://example.com/redirected?returnTo=%2Ffoo%3Fbar%3Dbaz\">"
    );
}

#[test]
fn escapes_malicious_redirect_content() {
    assert_eq!(
        render_prerender_redirect_html("https://example.com/</script>alert(\"pwned\")"),
        "<script>location.href=\"https://example.com/\\u003C/script\\u003Ealert(\\\"pwned\\\")\";</script><meta http-equiv=\"refresh\" content=\"0;url=https://example.com/&lt;/script&gt;alert(&quot;pwned&quot;)\">"
    );
}

#[test]
fn renders_redirect_and_data_json_payloads() {
    let redirect = redirect_data_response("https://example.com/redirected");
    assert_eq!(redirect.status.as_u16(), 200);
    let redirect_body: serde_json::Value =
        serde_json::from_str(redirect.body.as_deref().expect("redirect body"))
            .expect("valid redirect json");
    assert_eq!(
        redirect_body,
        json!({
            "type": "redirect",
            "location": "https://example.com/redirected"
        })
    );

    let data = data_json_response(
        json!({
            "type": "data",
            "nodes": [serde_json::Value::Null, {
                "type": "data",
                "data": [{"message": 1}, "hello"],
                "uses": {}
            }]
        }),
        200,
    );
    assert!(data.body.expect("data body").contains("\"nodes\""));
}

#[test]
fn renders_shell_page_for_spa_prerender_mode() {
    let shell = render_shell_page_response(200, true);
    assert_eq!(shell.status, 200);
    assert!(!shell.ssr);
    assert!(shell.csr);
}

#[test]
fn renders_http_equiv_meta_tags_for_prerender_headers() {
    assert_eq!(
        render_http_equiv_meta_tag("cache-control", "max-age=300"),
        "<meta http-equiv=\"cache-control\" content=\"max-age=300\">"
    );
}

#[test]
fn decodes_paths_when_writing_output_files() {
    let decoded = decode_pathname("/encoding/path%20with%20spaces");
    assert_eq!(
        prerender_output_filename("", &decoded, true),
        "encoding/path with spaces.html"
    );
}

#[test]
fn resolves_relative_links_during_prerender() {
    assert_eq!(
        resolve("/resolve-relative/lv1/lv2/", "/resolve-relative/lv1"),
        "/resolve-relative/lv1"
    );
}

#[test]
fn preserves_configured_prerender_origin() {
    let cwd = Utf8PathBuf::from("E:/Projects/svelte");
    let config = validate_config(
        &json!({
            "kit": {
                "prerender": {
                    "origin": "http://prerender.origin"
                }
            }
        }),
        &cwd,
    )
    .expect("validate config");

    assert_eq!(config.kit.prerender.origin, "http://prerender.origin");
    assert_eq!(fallback_page_filename("200.html"), "200.html");
}

#[test]
fn generates_public_env_modules_for_prerendered_output() {
    let env = Map::from_iter([("PUBLIC_STATIC".to_string(), json!("accessible anywhere"))]);
    let module = create_dynamic_module(EnvKind::Public, Some(&env), "");
    assert!(module.contains("PUBLIC_STATIC"));
    assert!(module.contains("accessible anywhere"));
}

#[test]
fn writes_grouped_pages_and_missing_ids_outputs() {
    assert_eq!(
        prerender_output_filename("", "/grouped", true),
        "grouped.html"
    );
    assert_eq!(
        serialize_missing_ids_jsonl(&["missing-id"]),
        "\"missing-id\","
    );
}

#[test]
fn renders_relative_service_worker_registration_and_prerender_paths() {
    assert_eq!(relative_service_worker_path("/"), "./service-worker.js");
    assert_eq!(
        render_service_worker_registration("/"),
        "navigator.serviceWorker.register('./service-worker.js')"
    );

    let paths = service_worker_prerender_paths(
        "",
        &[
            "/trailing-slash/page/".to_string(),
            "/trailing-slash/page/__data.json".to_string(),
            "/trailing-slash/standalone-endpoint.json".to_string(),
        ],
    );

    assert_eq!(
        paths,
        vec![
            "base + \"/trailing-slash/page/\"",
            "base + \"/trailing-slash/page/__data.json\"",
            "base + \"/trailing-slash/standalone-endpoint.json\"",
        ]
    );
}
