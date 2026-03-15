use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde_json::{Map, json};
use svelte_kit::{
    AppState, Error, Hooks, KitManifest, RouteResolutionAssets, RuntimeAppError,
    RuntimeRequestOptions, Server, ServerHookLoader, ServerHooks, ServerInitOptions,
    ServerTransportDecoder, ServerTransportEncoder, ServerTransportHook, decode_app_value,
    encode_app_value, encode_transport_value,
};

fn empty_manifest() -> KitManifest {
    KitManifest {
        assets: Vec::new(),
        hooks: Hooks::default(),
        matchers: BTreeMap::new(),
        manifest_routes: Vec::new(),
        nodes: Vec::new(),
        routes: Vec::new(),
    }
}

fn runtime_options() -> RuntimeRequestOptions {
    RuntimeRequestOptions {
        base: String::new(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: Vec::new(),
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    }
}

fn decode_date(value: &serde_json::Value) -> svelte_kit::Result<serde_json::Value> {
    Ok(json!(format!("decoded:{value}")))
}

fn encode_date(value: &serde_json::Value) -> svelte_kit::Result<Option<serde_json::Value>> {
    if value.get("$type") == Some(&json!("date")) {
        Ok(Some(json!(
            value
                .get("value")
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        )))
    } else {
        Ok(None)
    }
}

#[test]
fn uses_explicit_app_transports() {
    let app_state = AppState {
        decoders: BTreeMap::from([(
            "date".to_string(),
            Arc::new(decode_date) as ServerTransportDecoder,
        )]),
        encoders: BTreeMap::from([(
            "date".to_string(),
            Arc::new(encode_date) as ServerTransportEncoder,
        )]),
    };

    assert_eq!(
        decode_app_value(&app_state, "date", &json!("2026-03-11")).expect("decoder succeeds"),
        json!("decoded:\"2026-03-11\"")
    );
    assert_eq!(
        encode_app_value(
            &app_state,
            "date",
            &json!({ "$type": "date", "value": "2026-03-11" })
        )
        .expect("encoder succeeds"),
        json!("2026-03-11")
    );
    assert_eq!(
        encode_transport_value(
            &app_state,
            &json!({ "published": { "$type": "date", "value": "2026-03-11" } }),
        )
        .expect("tree transport encode succeeds"),
        json!({
            "published": {
                "kind": "date",
                "type": "Transport",
                "value": "2026-03-11"
            }
        })
    );
    assert!(app_state.decoders.contains_key("date"));
    assert!(app_state.encoders.contains_key("date"));
}

#[test]
fn server_init_registers_transport_codecs() {
    let loader_calls = Arc::new(AtomicUsize::new(0));
    let loader_calls_clone = Arc::clone(&loader_calls);
    let mut server = Server::new(
        empty_manifest(),
        runtime_options(),
        "PUBLIC_",
        "PRIVATE_",
        Some(ServerHookLoader::new(move || {
            loader_calls_clone.fetch_add(1, Ordering::SeqCst);
            Ok(ServerHooks {
                init: None,
                handle: None,
                reroute: None,
                transport: BTreeMap::from([(
                    "thing".to_string(),
                    ServerTransportHook {
                        decode: Arc::new(|value| Ok(json!({ "decoded": value }))),
                        encode: Some(Arc::new(|value| Ok(Some(json!({ "encoded": value }))))),
                    },
                )]),
            })
        })),
    );

    server
        .init(ServerInitOptions {
            env: Map::new(),
            read: None,
        })
        .expect("init succeeds");
    server
        .init(ServerInitOptions {
            env: Map::new(),
            read: None,
        })
        .expect("second init succeeds");

    assert_eq!(loader_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        decode_app_value(server.app_state(), "thing", &json!(123))
            .expect("transport decoder succeeds"),
        json!({ "decoded": 123 })
    );
    assert_eq!(
        encode_app_value(server.app_state(), "thing", &json!(123))
            .expect("transport encoder succeeds"),
        json!({ "encoded": 123 })
    );
}

#[test]
fn missing_transport_decoder_is_typed() {
    let app_state = AppState::default();

    let error = decode_app_value(&app_state, "missing", &json!(123))
        .expect_err("missing decoder should fail");
    assert!(matches!(
        error,
        Error::RuntimeApp(RuntimeAppError::MissingTransportDecoder { ref kind })
            if kind == "missing"
    ));
    assert_eq!(
        error.to_string(),
        "No transport decoder registered for `missing`"
    );
}
