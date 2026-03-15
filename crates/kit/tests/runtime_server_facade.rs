use std::{
    collections::BTreeMap,
    fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use camino::Utf8PathBuf;
use http::Method;
use serde_json::{Map, json};
use svelte_kit::{
    EndpointModule, ErrorPageRequestResult, ExecutedErrorPage, Hooks, KitManifest, ManifestConfig,
    RouteResolutionAssets, RuntimeRequestOptions, Server, ServerHandle, ServerHookInit,
    ServerHookLoader, ServerHooks, ServerInitOptions, ServerRead, ServerRequest, ServerReroute,
    ServerResponse,
};
use url::Url;

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

fn temp_dir(name: &str) -> Utf8PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("svelte-kit-{name}-{unique}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    Utf8PathBuf::from_path_buf(dir).expect("utf8 temp dir")
}

fn write_file(path: &Utf8PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write file");
}

#[test]
fn init_filters_env_updates_read_and_runs_hooks_once() {
    let hook_inits = Arc::new(AtomicUsize::new(0));
    let hook_inits_clone = hook_inits.clone();
    let hook_loader_calls = Arc::new(AtomicUsize::new(0));
    let hook_loader_calls_clone = hook_loader_calls.clone();

    let mut server = Server::new(
        empty_manifest(),
        runtime_options(),
        "PUBLIC_",
        "PRIVATE_",
        Some(ServerHookLoader::new(move || {
            hook_loader_calls_clone.fetch_add(1, Ordering::SeqCst);
            Ok(ServerHooks::with_init({
                let hook_inits_clone = hook_inits_clone.clone();
                ServerHookInit::new(move || {
                    hook_inits_clone.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }))
        })),
    );

    let first_read = ServerRead::new(|file| Ok(Some(format!("first:{file}").into_bytes())));
    let second_read = ServerRead::new(|file| Ok(Some(format!("second:{file}").into_bytes())));

    server
        .init(ServerInitOptions {
            env: Map::from_iter([
                ("PUBLIC_FOO".to_string(), json!("bar")),
                ("PRIVATE_BAR".to_string(), json!("baz")),
                ("OTHER".to_string(), json!("ignored")),
            ]),
            read: Some(first_read),
        })
        .expect("first init succeeds");
    server
        .init(ServerInitOptions {
            env: Map::from_iter([
                ("PUBLIC_HELLO".to_string(), json!("world")),
                ("PRIVATE_TOKEN".to_string(), json!("secret")),
            ]),
            read: Some(second_read),
        })
        .expect("second init succeeds");

    assert_eq!(hook_loader_calls.load(Ordering::SeqCst), 1);
    assert_eq!(hook_inits.load(Ordering::SeqCst), 1);
    assert_eq!(
        server.public_env(),
        &Map::from_iter([("PUBLIC_HELLO".to_string(), json!("world"))])
    );
    assert_eq!(
        server.private_env(),
        &Map::from_iter([("PRIVATE_TOKEN".to_string(), json!("secret"))])
    );
    assert_eq!(
        server.read("asset.txt").expect("read succeeds"),
        Some(b"second:asset.txt".to_vec())
    );
}

#[test]
fn respond_exposes_filtered_public_env_after_init() {
    let mut server = Server::new(
        empty_manifest(),
        runtime_options(),
        "PUBLIC_",
        "PRIVATE_",
        None,
    );
    server
        .init(ServerInitOptions {
            env: Map::from_iter([
                ("PUBLIC_ANSWER".to_string(), json!("42")),
                ("PRIVATE_SECRET".to_string(), json!("nope")),
            ]),
            read: None,
        })
        .expect("init succeeds");

    let request = ServerRequest::builder()
        .url(Url::parse("https://example.com/_app/env.js").expect("valid url"))
        .build()
        .expect("request succeeds");

    let response = server
        .respond(
            &request,
            false,
            false,
            |status, message| format!("{status}:{message}"),
            |_, _| false,
            |_| None,
            |_, _, _| Ok(None),
            |_, _, _, _| {
                Ok(svelte_kit::ActionJsonResult::Success {
                    status: 200,
                    data: None,
                })
            },
            |_, _, _, _| {
                Ok(svelte_kit::ActionRequestResult::Success {
                    status: 200,
                    data: None,
                })
            },
            |_, _, _, _, _| Ok(svelte_kit::RemoteFormExecutionResult::Success),
            |_, _, _, _, _, _, _| {
                Ok(svelte_kit::PageLoadResult::Loaded {
                    server_data: None,
                    data: None,
                })
            },
            |status, error, _| {
                Ok(ErrorPageRequestResult::Rendered(ExecutedErrorPage {
                    plan: svelte_kit::ErrorPageRenderPlan {
                        status,
                        error,
                        ssr: true,
                        csr: true,
                        branch_node_indexes: Vec::new(),
                    },
                    branch: Vec::new(),
                }))
            },
            |_, _, _| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
        )
        .expect("respond succeeds");

    assert_eq!(response.status, 200);
    let body = response.body.expect("env module body");
    assert!(body.contains("PUBLIC_ANSWER"));
    assert!(body.contains("\"42\""));
    assert!(!body.contains("PRIVATE_SECRET"));
}

#[test]
fn handle_hook_can_short_circuit_response() {
    let mut server = Server::new(
        empty_manifest(),
        runtime_options(),
        "PUBLIC_",
        "PRIVATE_",
        Some(ServerHookLoader::new(|| {
            Ok(ServerHooks {
                init: None,
                handle: Some(ServerHandle::new(|ctx| {
                    let _ = ctx.request;
                    Ok(ServerResponse::new(418))
                })),
                reroute: None,
                transport: BTreeMap::new(),
            })
        })),
    );
    server
        .init(ServerInitOptions {
            env: Map::new(),
            read: None,
        })
        .expect("init succeeds");

    let request = ServerRequest::builder()
        .url(Url::parse("https://example.com/_app/env.js").expect("valid url"))
        .build()
        .expect("request succeeds");

    let response = server
        .respond(
            &request,
            false,
            false,
            |status, message| format!("{status}:{message}"),
            |_, _| false,
            |_| None,
            |_, _, _| Ok(None),
            |_, _, _, _| {
                Ok(svelte_kit::ActionJsonResult::Success {
                    status: 200,
                    data: None,
                })
            },
            |_, _, _, _| {
                Ok(svelte_kit::ActionRequestResult::Success {
                    status: 200,
                    data: None,
                })
            },
            |_, _, _, _, _| Ok(svelte_kit::RemoteFormExecutionResult::Success),
            |_, _, _, _, _, _, _| {
                Ok(svelte_kit::PageLoadResult::Loaded {
                    server_data: None,
                    data: None,
                })
            },
            |status, error, _| {
                Ok(ErrorPageRequestResult::Rendered(ExecutedErrorPage {
                    plan: svelte_kit::ErrorPageRenderPlan {
                        status,
                        error,
                        ssr: true,
                        csr: true,
                        branch_node_indexes: Vec::new(),
                    },
                    branch: Vec::new(),
                }))
            },
            |_, _, _| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
        )
        .expect("respond succeeds");

    assert_eq!(response.status, 418);
}

#[test]
fn handle_hook_can_delegate_to_resolve() {
    let mut server = Server::new(
        empty_manifest(),
        runtime_options(),
        "PUBLIC_",
        "PRIVATE_",
        Some(ServerHookLoader::new(|| {
            Ok(ServerHooks {
                init: None,
                handle: Some(ServerHandle::new(|ctx| (ctx.resolve)(ctx.request))),
                reroute: None,
                transport: BTreeMap::new(),
            })
        })),
    );
    server
        .init(ServerInitOptions {
            env: Map::from_iter([("PUBLIC_HELLO".to_string(), json!("world"))]),
            read: None,
        })
        .expect("init succeeds");

    let request = ServerRequest::builder()
        .url(Url::parse("https://example.com/_app/env.js").expect("valid url"))
        .build()
        .expect("request succeeds");

    let response = server
        .respond(
            &request,
            false,
            false,
            |status, message| format!("{status}:{message}"),
            |_, _| false,
            |_| None,
            |_, _, _| Ok(None),
            |_, _, _, _| {
                Ok(svelte_kit::ActionJsonResult::Success {
                    status: 200,
                    data: None,
                })
            },
            |_, _, _, _| {
                Ok(svelte_kit::ActionRequestResult::Success {
                    status: 200,
                    data: None,
                })
            },
            |_, _, _, _, _| Ok(svelte_kit::RemoteFormExecutionResult::Success),
            |_, _, _, _, _, _, _| {
                Ok(svelte_kit::PageLoadResult::Loaded {
                    server_data: None,
                    data: None,
                })
            },
            |status, error, _| {
                Ok(ErrorPageRequestResult::Rendered(ExecutedErrorPage {
                    plan: svelte_kit::ErrorPageRenderPlan {
                        status,
                        error,
                        ssr: true,
                        csr: true,
                        branch_node_indexes: Vec::new(),
                    },
                    branch: Vec::new(),
                }))
            },
            |_, _, _| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
        )
        .expect("respond succeeds");

    assert_eq!(response.status, 200);
    assert!(
        response.body.expect("body").contains("PUBLIC_HELLO"),
        "delegate hook should preserve resolved response"
    );
}

#[test]
fn reroute_hook_changes_route_resolution() {
    let cwd = temp_dir("server-reroute");
    let routes_dir = cwd.join("src").join("routes");
    write_file(
        &routes_dir.join("to").join("+server.ts"),
        "export const GET = true;",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let mut server = Server::new(
        manifest,
        runtime_options(),
        "PUBLIC_",
        "PRIVATE_",
        Some(ServerHookLoader::new(|| {
            Ok(ServerHooks {
                init: None,
                handle: None,
                reroute: Some(ServerReroute::new(|url| {
                    if url.path() == "/from" {
                        Ok(Some("/to".to_string()))
                    } else {
                        Ok(None)
                    }
                })),
                transport: BTreeMap::new(),
            })
        })),
    );
    server
        .init(ServerInitOptions {
            env: Map::new(),
            read: None,
        })
        .expect("init succeeds");

    let request = ServerRequest::builder()
        .url(Url::parse("https://example.com/from").expect("valid url"))
        .header("accept", "application/json")
        .expect("valid header")
        .build()
        .expect("request succeeds");

    let response = server
        .respond(
            &request,
            false,
            false,
            |status, message| format!("{status}:{message}"),
            |_, _| true,
            |resolved| {
                resolved.and_then(|resolved| {
                    (resolved.route.id == "/to").then(|| {
                        EndpointModule::new().with_handler(Method::GET, |_| {
                            Ok(ServerResponse::builder(200)
                                .body("rerouted".to_string())
                                .build()
                                .expect("response"))
                        })
                    })
                })
            },
            |_, _, _| Ok(None),
            |_, _, _, _| {
                Ok(svelte_kit::ActionJsonResult::Success {
                    status: 200,
                    data: None,
                })
            },
            |_, _, _, _| {
                Ok(svelte_kit::ActionRequestResult::Success {
                    status: 200,
                    data: None,
                })
            },
            |_, _, _, _, _| Ok(svelte_kit::RemoteFormExecutionResult::Success),
            |_, _, _, _, _, _, _| {
                Ok(svelte_kit::PageLoadResult::Loaded {
                    server_data: None,
                    data: None,
                })
            },
            |status, error, _| {
                Ok(ErrorPageRequestResult::Rendered(ExecutedErrorPage {
                    plan: svelte_kit::ErrorPageRenderPlan {
                        status,
                        error,
                        ssr: true,
                        csr: true,
                        branch_node_indexes: Vec::new(),
                    },
                    branch: Vec::new(),
                }))
            },
            |_, _, _| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
            |_| Ok(ServerResponse::new(204)),
        )
        .expect("respond succeeds");

    assert_eq!(response.status, 200);
    assert_eq!(response.body.as_deref(), Some("rerouted"));
}
