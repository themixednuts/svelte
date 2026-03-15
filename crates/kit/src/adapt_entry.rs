use crate::{
    BuildData, BuilderFacade, BuilderPrerendered, BuilderServerMetadata, RemoteChunk,
    ValidatedConfig,
    error::{AdaptError, Result},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptProjectResult<T> {
    pub adapter_name: String,
    pub output: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptStatus {
    pub using_message: String,
    pub success_message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptInvocationResult<T> {
    pub adapter_name: String,
    pub output: T,
    pub status: AdaptStatus,
}

pub fn adapt_project<'a, T, F>(
    config: &'a ValidatedConfig,
    build_data: &'a BuildData<'a>,
    server_metadata: &BuilderServerMetadata,
    prerendered: &'a BuilderPrerendered,
    remotes: &'a [RemoteChunk],
    callback: F,
) -> Result<AdaptProjectResult<T>>
where
    F: FnOnce(&BuilderFacade<'a>) -> Result<T>,
{
    let adapter = config
        .kit
        .adapter
        .as_ref()
        .ok_or(AdaptError::MissingConfiguredAdapter)?;

    let adapter_name = adapter
        .raw
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("adapter")
        .to_string();

    let facade = BuilderFacade::new(
        config,
        build_data,
        server_metadata,
        &build_data.manifest_data.manifest_routes,
        prerendered,
        remotes,
    );

    let output = callback(&facade)?;
    Ok(AdaptProjectResult {
        adapter_name,
        output,
    })
}

pub fn invoke_adapter<'a, T, F>(
    config: &'a ValidatedConfig,
    build_data: &'a BuildData<'a>,
    server_metadata: &BuilderServerMetadata,
    prerendered: &'a BuilderPrerendered,
    remotes: &'a [RemoteChunk],
    callback: F,
) -> Result<AdaptInvocationResult<T>>
where
    F: FnOnce(&BuilderFacade<'a>) -> Result<T>,
{
    let result = adapt_project(
        config,
        build_data,
        server_metadata,
        prerendered,
        remotes,
        callback,
    )?;

    Ok(AdaptInvocationResult {
        adapter_name: result.adapter_name.clone(),
        output: result.output,
        status: AdaptStatus {
            using_message: format!("> Using {}", result.adapter_name),
            success_message: "done".to_string(),
        },
    })
}
