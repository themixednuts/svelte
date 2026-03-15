use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

pub struct TransformPageChunk {
    inner: Arc<dyn Fn(PageChunk) -> BoxFuture<String> + Send + Sync>,
}

impl Clone for TransformPageChunk {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl TransformPageChunk {
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: Fn(PageChunk) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = String> + Send + 'static,
    {
        Self {
            inner: Arc::new(move |chunk| Box::pin(f(chunk))),
        }
    }

    pub fn call(&self, chunk: PageChunk) -> BoxFuture<String> {
        (self.inner)(chunk)
    }
}

pub struct Preload {
    inner: Arc<dyn Fn(PreloadInput<'_>) -> bool + Send + Sync>,
}

impl Clone for Preload {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Preload {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(PreloadInput<'_>) -> bool + Send + Sync + 'static,
    {
        Self { inner: Arc::new(f) }
    }

    pub fn call(&self, input: PreloadInput<'_>) -> bool {
        (self.inner)(input)
    }
}

pub struct FilterSerializedResponseHeaders {
    inner: Arc<dyn Fn(&str, &str) -> bool + Send + Sync>,
}

impl Clone for FilterSerializedResponseHeaders {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl FilterSerializedResponseHeaders {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&str, &str) -> bool + Send + Sync + 'static,
    {
        Self { inner: Arc::new(f) }
    }

    pub fn call(&self, name: &str, value: &str) -> bool {
        (self.inner)(name, value)
    }
}

#[derive(Clone, Copy)]
pub struct PreloadInput<'a> {
    pub path: &'a str,
    pub r#type: &'a str,
}

#[derive(Clone, Default)]
pub struct ResolveOptions {
    pub transform_page_chunk: Option<TransformPageChunk>,
    pub preload: Option<Preload>,
    pub filter_serialized_response_headers: Option<FilterSerializedResponseHeaders>,
}

pub struct Resolve<E, R> {
    inner: Arc<dyn Fn(E, ResolveOptions) -> BoxFuture<R> + Send + Sync>,
}

impl<E, R> Clone for Resolve<E, R> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<E, R> Resolve<E, R> {
    pub fn new<F, Fut>(f: F) -> Self
    where
        E: Send + 'static,
        R: Send + 'static,
        F: Fn(E, ResolveOptions) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
    {
        Self {
            inner: Arc::new(move |event, options| Box::pin(f(event, options))),
        }
    }

    pub fn call(&self, event: E, options: ResolveOptions) -> BoxFuture<R> {
        (self.inner)(event, options)
    }
}

pub struct HandleContext<E, R> {
    pub event: E,
    pub resolve: Resolve<E, R>,
}

pub struct Handle<E, R> {
    inner: Arc<dyn Fn(HandleContext<E, R>) -> BoxFuture<R> + Send + Sync>,
}

impl<E, R> Clone for Handle<E, R> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<E, R> Handle<E, R> {
    pub fn new<F, Fut>(f: F) -> Self
    where
        E: Send + 'static,
        R: Send + 'static,
        F: Fn(HandleContext<E, R>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
    {
        Self {
            inner: Arc::new(move |context| Box::pin(f(context))),
        }
    }

    pub fn call(&self, context: HandleContext<E, R>) -> BoxFuture<R> {
        (self.inner)(context)
    }
}

pub fn handle<E, R, F, Fut>(f: F) -> Handle<E, R>
where
    E: Send + 'static,
    R: Send + 'static,
    F: Fn(HandleContext<E, R>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
{
    Handle::new(f)
}

pub fn resolve_fn<E, R, F, Fut>(f: F) -> Resolve<E, R>
where
    E: Send + 'static,
    R: Send + 'static,
    F: Fn(E, ResolveOptions) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
{
    Resolve::new(f)
}

pub fn transform_page_chunk<F, Fut>(f: F) -> TransformPageChunk
where
    F: Fn(PageChunk) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = String> + Send + 'static,
{
    TransformPageChunk::new(f)
}

pub fn preload<F>(f: F) -> Preload
where
    F: Fn(PreloadInput<'_>) -> bool + Send + Sync + 'static,
{
    Preload::new(f)
}

pub fn filter_serialized_response_headers<F>(f: F) -> FilterSerializedResponseHeaders
where
    F: Fn(&str, &str) -> bool + Send + Sync + 'static,
{
    FilterSerializedResponseHeaders::new(f)
}

#[derive(Clone)]
pub struct PageChunk {
    pub html: String,
    pub done: bool,
}

pub fn sequence<E, R>(handlers: Vec<Handle<E, R>>) -> Handle<E, R>
where
    E: Clone + Send + Sync + 'static,
    R: Send + Sync + 'static,
{
    if handlers.is_empty() {
        return Handle::new(|context| async move {
            context
                .resolve
                .call(context.event, ResolveOptions::default())
                .await
        });
    }

    let handlers = Arc::new(handlers);
    Handle::new(move |context| {
        let handlers = Arc::clone(&handlers);
        async move {
            apply_handle(
                0,
                context.event,
                ResolveOptions::default(),
                context.resolve,
                handlers,
            )
            .await
        }
    })
}

fn apply_handle<E, R>(
    index: usize,
    event: E,
    parent_options: ResolveOptions,
    resolve: Resolve<E, R>,
    handlers: Arc<Vec<Handle<E, R>>>,
) -> BoxFuture<R>
where
    E: Clone + Send + Sync + 'static,
    R: Send + Sync + 'static,
{
    let handle = handlers[index].clone();
    Box::pin(async move {
        let child_resolve = Resolve {
            inner: Arc::new(move |event, options| {
                let resolve = resolve.clone();
                let handlers = Arc::clone(&handlers);
                let merged = merge_options(&parent_options, &options);

                if index + 1 < handlers.len() {
                    apply_handle(index + 1, event, merged, resolve, handlers)
                } else {
                    resolve.call(event, merged)
                }
            }),
        };

        handle
            .call(HandleContext {
                event,
                resolve: child_resolve,
            })
            .await
    })
}

fn merge_options(parent: &ResolveOptions, current: &ResolveOptions) -> ResolveOptions {
    let transform_page_chunk = match (
        current.transform_page_chunk.clone(),
        parent.transform_page_chunk.clone(),
    ) {
        (None, None) => None,
        (Some(current), None) => Some(current),
        (None, Some(parent)) => Some(parent),
        (Some(current), Some(parent)) => {
            let combined = TransformPageChunk::new(move |chunk: PageChunk| {
                let current = current.clone();
                let parent = parent.clone();
                async move {
                    let html = current.call(chunk.clone()).await;
                    parent
                        .call(PageChunk {
                            html,
                            done: chunk.done,
                        })
                        .await
                }
            });
            Some(combined)
        }
    };

    ResolveOptions {
        transform_page_chunk,
        preload: parent.preload.clone().or_else(|| current.preload.clone()),
        filter_serialized_response_headers: parent
            .filter_serialized_response_headers
            .clone()
            .or_else(|| current.filter_serialized_response_headers.clone()),
    }
}
