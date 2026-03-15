use std::collections::BTreeMap;

use serde_json::json;
use svelte_kit::{
    ActionFailure, Error, ExportsInternalError, HttpErrorClass, RedirectClass, RemoteExport,
    RemoteFunctionInfo, RemoteFunctionKind, SvelteKitErrorClass, ValidationErrorClass,
    init_remote_functions,
};

#[test]
fn http_error_message_body_matches_upstream_shape() {
    let error = HttpErrorClass::from_message(404, "Nope");
    assert_eq!(error.status, 404);
    assert_eq!(error.body, json!({ "message": "Nope" }));
    assert_eq!(error.to_string(), "{\"message\":\"Nope\"}");
}

#[test]
fn http_error_defaults_message_when_missing() {
    let error = HttpErrorClass::new(500);
    assert_eq!(error.body, json!({ "message": "Error: 500" }));
}

#[test]
fn redirect_and_action_failure_preserve_payload() {
    let redirect = RedirectClass::new(307, "/next");
    assert_eq!(redirect.status, 307);
    assert_eq!(redirect.location, "/next");

    let failure = ActionFailure::new(422, json!({ "field": "bad" }));
    assert_eq!(failure.status, 422);
    assert_eq!(failure.data, json!({ "field": "bad" }));
}

#[test]
fn sveltekit_error_and_validation_error_preserve_fields() {
    let error = SvelteKitErrorClass::new(400, "Bad Request", "payload invalid");
    assert_eq!(error.status, 400);
    assert_eq!(error.text, "Bad Request");
    assert_eq!(error.to_string(), "payload invalid");

    let validation = ValidationErrorClass::new(vec![json!({ "message": "bad" })]);
    assert_eq!(validation.issues, vec![json!({ "message": "bad" })]);
    assert_eq!(validation.to_string(), "Validation failed");
}

#[test]
fn init_remote_functions_assigns_id_and_name() {
    let mut module = BTreeMap::from([
        (
            "answer".to_string(),
            RemoteExport::new(Some(RemoteFunctionInfo::new(RemoteFunctionKind::Query))),
        ),
        (
            "submit".to_string(),
            RemoteExport::new(Some(RemoteFunctionInfo::new(RemoteFunctionKind::Form))),
        ),
    ]);

    init_remote_functions(&mut module, "src/lib.remote.ts", "abc123")
        .expect("remote module should validate");

    assert_eq!(
        module["answer"].info().unwrap().id.as_deref(),
        Some("abc123/answer")
    );
    assert_eq!(
        module["answer"].info().unwrap().name.as_deref(),
        Some("answer")
    );
    assert_eq!(
        module["submit"].info().unwrap().id.as_deref(),
        Some("abc123/submit")
    );
}

#[test]
fn init_remote_functions_rejects_default_and_invalid_exports() {
    let mut has_default = BTreeMap::from([(
        "default".to_string(),
        RemoteExport::new(Some(RemoteFunctionInfo::new(RemoteFunctionKind::Query))),
    )]);
    let default_error = init_remote_functions(&mut has_default, "src/lib.remote.ts", "hash")
        .expect_err("default export should be rejected");
    assert!(
        default_error
            .to_string()
            .contains("Cannot export `default`")
    );
    assert!(matches!(
        default_error,
        Error::ExportsInternal(ExportsInternalError::DefaultRemoteExport { file })
        if file == "src/lib.remote.ts"
    ));

    let mut invalid = BTreeMap::from([("oops".to_string(), RemoteExport::new(None))]);
    let invalid_error = init_remote_functions(&mut invalid, "src/lib.remote.ts", "hash")
        .expect_err("non-remote export should be rejected");
    assert!(
        invalid_error
            .to_string()
            .contains("all exports from this file must be remote functions")
    );
    assert!(matches!(
        invalid_error,
        Error::ExportsInternal(ExportsInternalError::InvalidRemoteExport { name, file })
        if name == "oops" && file == "src/lib.remote.ts"
    ));
}
