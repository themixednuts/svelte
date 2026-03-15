use std::sync::{Arc, Mutex};

use svelte_kit::{
    HandleContext, PageChunk, PreloadInput, ResolveOptions, filter_serialized_response_headers,
    handle, preload, resolve_fn, sequence, transform_page_chunk,
};

#[tokio::test]
async fn applies_handlers_in_sequence() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let handler = sequence::<(), String>(vec![
        {
            let order = Arc::clone(&order);
            handle(move |context: HandleContext<(), String>| {
                let order = Arc::clone(&order);
                async move {
                    order.lock().expect("order").push("1a");
                    let response = context
                        .resolve
                        .call(context.event, ResolveOptions::default())
                        .await;
                    order.lock().expect("order").push("1b");
                    response
                }
            })
        },
        {
            let order = Arc::clone(&order);
            handle(move |context: HandleContext<(), String>| {
                let order = Arc::clone(&order);
                async move {
                    order.lock().expect("order").push("2a");
                    let response = context
                        .resolve
                        .call(context.event, ResolveOptions::default())
                        .await;
                    order.lock().expect("order").push("2b");
                    response
                }
            })
        },
        {
            let order = Arc::clone(&order);
            handle(move |context: HandleContext<(), String>| {
                let order = Arc::clone(&order);
                async move {
                    order.lock().expect("order").push("3a");
                    let response = context
                        .resolve
                        .call(context.event, ResolveOptions::default())
                        .await;
                    order.lock().expect("order").push("3b");
                    response
                }
            })
        },
    ]);

    let resolve = resolve_fn(|_: (), _: ResolveOptions| async move { "response".to_string() });
    let response = handler.call(HandleContext { event: (), resolve }).await;
    assert_eq!(response, "response");
    assert_eq!(
        *order.lock().expect("order"),
        vec!["1a", "2a", "3a", "3b", "2b", "1b"]
    );
}

#[tokio::test]
async fn merges_transform_page_chunk_in_reverse_order() {
    let handler = sequence::<(), String>(vec![
        handle(|context: HandleContext<(), String>| async move {
            let options = ResolveOptions {
                transform_page_chunk: Some(transform_page_chunk(|chunk: PageChunk| async move {
                    format!(
                        "{}{}",
                        chunk.html,
                        if chunk.done { "-1-done" } else { "-1" }
                    )
                })),
                ..ResolveOptions::default()
            };
            context.resolve.call(context.event, options).await
        }),
        handle(|context: HandleContext<(), String>| async move {
            let options = ResolveOptions {
                transform_page_chunk: Some(transform_page_chunk(|chunk: PageChunk| async move {
                    format!(
                        "{}{}",
                        chunk.html,
                        if chunk.done { "-2-done" } else { "-2" }
                    )
                })),
                ..ResolveOptions::default()
            };
            context.resolve.call(context.event, options).await
        }),
        handle(|context: HandleContext<(), String>| async move {
            let options = ResolveOptions {
                transform_page_chunk: Some(transform_page_chunk(|chunk: PageChunk| async move {
                    format!(
                        "{}{}",
                        chunk.html,
                        if chunk.done { "-3-done" } else { "-3" }
                    )
                })),
                ..ResolveOptions::default()
            };
            context.resolve.call(context.event, options).await
        }),
    ]);

    let resolve = resolve_fn(|_: (), options: ResolveOptions| async move {
        let transform = options.transform_page_chunk.expect("transform");
        let mut html = String::new();
        html.push_str(
            &transform
                .call(PageChunk {
                    html: "0".to_string(),
                    done: false,
                })
                .await,
        );
        html.push_str(
            &transform
                .call(PageChunk {
                    html: " 0".to_string(),
                    done: true,
                })
                .await,
        );
        html
    });

    let response = handler.call(HandleContext { event: (), resolve }).await;
    assert_eq!(response, "0-3-2-1 0-3-done-2-done-1-done");
}

#[tokio::test]
async fn uses_first_defined_preload_and_filter() {
    let handler = sequence::<(), String>(vec![
        handle(|context: HandleContext<(), String>| async move {
            context
                .resolve
                .call(context.event, ResolveOptions::default())
                .await
        }),
        handle(|context: HandleContext<(), String>| async move {
            let options = ResolveOptions {
                preload: Some(preload(|input: PreloadInput<'_>| input.r#type == "js")),
                filter_serialized_response_headers: Some(filter_serialized_response_headers(
                    |name, _| name == "a",
                )),
                ..ResolveOptions::default()
            };
            context.resolve.call(context.event, options).await
        }),
        handle(|context: HandleContext<(), String>| async move {
            let options = ResolveOptions {
                preload: Some(preload(|_: PreloadInput<'_>| true)),
                filter_serialized_response_headers: Some(filter_serialized_response_headers(
                    |_, _| true,
                )),
                ..ResolveOptions::default()
            };
            context.resolve.call(context.event, options).await
        }),
    ]);

    let resolve = resolve_fn(|_: (), options: ResolveOptions| async move {
        let preload = options.preload.expect("preload");
        let filter = options.filter_serialized_response_headers.expect("filter");
        format!(
            "{}{}{}{}",
            preload.call(PreloadInput {
                path: "",
                r#type: "js"
            }),
            preload.call(PreloadInput {
                path: "",
                r#type: "css"
            }),
            filter.call("a", ""),
            filter.call("b", "")
        )
    });

    let response = handler.call(HandleContext { event: (), resolve }).await;
    assert_eq!(response, "truefalsetruefalse");
}
