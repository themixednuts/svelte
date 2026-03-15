#![allow(clippy::needless_pass_by_value)]

use napi::Env;
use napi::Error;
use napi::bindgen_prelude::FunctionRef;
use napi_derive::napi;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::{Arc, Mutex};
use svelte_compiler::ast::Root;
use svelte_compiler::ast::modern::{
    Attribute, Comment, Css, CssNode, Fragment, Node, Options, Root as ModernRoot, Script,
};
use svelte_compiler::{
    CompileOptions, CssHashGetterCallback, MigrateOptions, ParseOptions,
    PrintOptions,
};

#[napi]
pub fn version() -> String {
    svelte_compiler::VERSION.to_string()
}

#[napi]
pub fn compile_json(source: String, options_json: Option<String>) -> napi::Result<String> {
    let options = deserialize_or_default::<CompileOptions>(options_json)?;
    let result = svelte_compiler::compile(&source, options).map_err(compile_error_to_napi)?;
    serialize_json(&result)
}

#[napi]
pub fn compile_json_with_callbacks(
    env: Env,
    source: String,
    options_json: Option<String>,
    css_hash_callback: Option<FunctionRef<(String,), String>>,
) -> napi::Result<String> {
    let mut options = deserialize_or_default::<CompileOptions>(options_json)?;
    let callback_error = Arc::new(Mutex::new(None));

    if let Some(callback) = css_hash_callback {
        let callback_error_ref = Arc::clone(&callback_error);
        options.css_hash_getter = Some(CssHashGetterCallback::new(move |input| {
            let payload = json!({
                "name": input.name,
                "filename": input.filename,
                "css": input.css,
                "hash_input": if input.filename == "(unknown)" { input.css } else { input.filename },
            });

            match serde_json::to_string(&payload)
                .map_err(|error| {
                    Error::from_reason(format!("failed to serialize cssHash payload: {error}"))
                })
                .and_then(|payload| call_js_function(&env, &callback, payload))
            {
                Ok(result) => Arc::from(result),
                Err(error) => {
                    store_callback_error(&callback_error_ref, error);
                    Arc::from("svelte-callback-error")
                }
            }
        }));
    }

    let result = svelte_compiler::compile(&source, options).map_err(compile_error_to_napi)?;
    if let Some(error) = take_callback_error(&callback_error) {
        return Err(error);
    }
    serialize_json(&result)
}

#[napi]
pub fn compile_module_json(source: String, options_json: Option<String>) -> napi::Result<String> {
    let options = deserialize_or_default::<CompileOptions>(options_json)?;
    let result =
        svelte_compiler::compile_module(&source, options).map_err(compile_error_to_napi)?;
    serialize_json(&result)
}

#[napi]
pub fn parse_json(source: String, options_json: Option<String>) -> napi::Result<String> {
    let options = deserialize_or_default::<ParseOptions>(options_json)?;
    let result = svelte_compiler::parse(&source, options).map_err(compile_error_to_napi)?;
    serialize_json(&result.root)
}

#[napi]
pub fn parse_css_json(source: String) -> napi::Result<String> {
    let result = svelte_compiler::parse_css(&source).map_err(compile_error_to_napi)?;
    serialize_json(&result)
}

#[napi]
pub fn print_json(
    kind: String,
    source: String,
    ast_json: String,
    options_json: Option<String>,
) -> napi::Result<String> {
    let options = deserialize_or_default::<PrintOptions>(options_json)?;
    let result = print_impl(kind, source, ast_json, options)?;
    serialize_json(&result)
}

#[napi]
pub fn print_source_json(source: String, options_json: Option<String>) -> napi::Result<String> {
    let options = deserialize_or_default::<PrintOptions>(options_json)?;
    let result = print_source_impl(source, options)?;
    serialize_json(&result)
}

#[napi]
pub fn print_json_with_callbacks(
    env: Env,
    kind: String,
    source: String,
    ast_json: String,
    options_json: Option<String>,
    leading_comments_callback: Option<FunctionRef<(String,), String>>,
    trailing_comments_callback: Option<FunctionRef<(String,), String>>,
) -> napi::Result<String> {
    let (options, callback_error) = build_print_options_with_callbacks(
        env,
        options_json,
        leading_comments_callback,
        trailing_comments_callback,
    )?;

    let result = print_impl(kind, source, ast_json, options)?;
    if let Some(error) = take_callback_error(&callback_error) {
        return Err(error);
    }
    serialize_json(&result)
}

#[napi]
pub fn print_source_json_with_callbacks(
    env: Env,
    source: String,
    options_json: Option<String>,
    leading_comments_callback: Option<FunctionRef<(String,), String>>,
    trailing_comments_callback: Option<FunctionRef<(String,), String>>,
) -> napi::Result<String> {
    let (options, callback_error) = build_print_options_with_callbacks(
        env,
        options_json,
        leading_comments_callback,
        trailing_comments_callback,
    )?;
    let result = print_source_impl(source, options)?;
    if let Some(error) = take_callback_error(&callback_error) {
        return Err(error);
    }
    serialize_json(&result)
}

#[napi]
pub fn migrate_json(source: String, options_json: Option<String>) -> napi::Result<String> {
    let options = deserialize_or_default::<MigrateOptions>(options_json)?;
    let result = svelte_compiler::migrate(&source, options).map_err(compile_error_to_napi)?;
    serialize_json(&result)
}

fn deserialize_or_default<T>(json: Option<String>) -> napi::Result<T>
where
    T: DeserializeOwned + Default,
{
    match json {
        Some(json) if !json.trim().is_empty() => deserialize_required(&json),
        _ => Ok(T::default()),
    }
}

fn deserialize_required<T>(json: &str) -> napi::Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_str(json).map_err(|error| {
        Error::from_reason(format!("invalid compiler bridge json payload: {error}"))
    })
}

fn serialize_json<T>(value: &T) -> napi::Result<String>
where
    T: Serialize,
{
    serde_json::to_string(value).map_err(|error| {
        Error::from_reason(format!("invalid compiler bridge json response: {error}"))
    })
}

fn print_impl(
    kind: String,
    source: String,
    ast_json: String,
    options: PrintOptions,
) -> napi::Result<svelte_compiler::PrintedOutput> {
    let result = match kind.as_str() {
        "root" => {
            let ast = deserialize_required::<ModernRoot>(&ast_json)?;
            svelte_compiler::print_modern(
                svelte_compiler::ModernPrintTarget::root(&source, &ast),
                options,
            )
        }
        "fragment" => {
            let ast = deserialize_required::<Fragment>(&ast_json)?;
            svelte_compiler::print_modern(
                svelte_compiler::ModernPrintTarget::fragment(&source, &ast),
                options,
            )
        }
        "node" => {
            let ast = deserialize_required::<Node>(&ast_json)?;
            svelte_compiler::print_modern(
                svelte_compiler::ModernPrintTarget::node(&source, &ast),
                options,
            )
        }
        "script" => {
            let ast = deserialize_required::<Script>(&ast_json)?;
            svelte_compiler::print_modern(
                svelte_compiler::ModernPrintTarget::script(&source, &ast),
                options,
            )
        }
        "css" => {
            let ast = deserialize_required::<Css>(&ast_json)?;
            svelte_compiler::print_modern(
                svelte_compiler::ModernPrintTarget::css(&source, &ast),
                options,
            )
        }
        "css-node" => {
            let ast = deserialize_required::<CssNode>(&ast_json)?;
            svelte_compiler::print_modern(
                svelte_compiler::ModernPrintTarget::css_node(&source, &ast),
                options,
            )
        }
        "attribute" => {
            let ast = deserialize_required::<Attribute>(&ast_json)?;
            svelte_compiler::print_modern(
                svelte_compiler::ModernPrintTarget::attribute(&source, &ast),
                options,
            )
        }
        "options" => {
            let ast = deserialize_required::<Options>(&ast_json)?;
            svelte_compiler::print_modern(
                svelte_compiler::ModernPrintTarget::options(&source, &ast),
                options,
            )
        }
        "comment" => {
            let ast = deserialize_required::<Comment>(&ast_json)?;
            svelte_compiler::print_modern(
                svelte_compiler::ModernPrintTarget::comment(&source, &ast),
                options,
            )
        }
        "document" => {
            let ast = deserialize_required::<Root>(&ast_json)?;
            match ast {
                Root::Modern(ast) => svelte_compiler::print_modern(
                    svelte_compiler::ModernPrintTarget::root(&source, &ast),
                    options,
                ),
                Root::Legacy(_) => {
                    return Err(Error::from_reason(
                        "print(ast) requires a modern AST node".to_string(),
                    ));
                }
            }
        }
        _ => {
            return Err(Error::from_reason(format!(
                "unsupported print kind '{kind}'"
            )));
        }
    }
    .map_err(compile_error_to_napi)?;

    Ok(result)
}

fn build_print_options_with_callbacks(
    _env: Env,
    options_json: Option<String>,
    _leading_comments_callback: Option<FunctionRef<(String,), String>>,
    _trailing_comments_callback: Option<FunctionRef<(String,), String>>,
) -> napi::Result<(PrintOptions, Arc<Mutex<Option<Error>>>)> {
    let options = deserialize_or_default::<PrintOptions>(options_json)?;
    let callback_error = Arc::new(Mutex::new(None));
    // TODO: Comment callbacks removed during EstreeNode migration.
    // Re-implement using JsComment when needed.
    Ok((options, callback_error))
}

fn print_source_impl(
    source: String,
    options: PrintOptions,
) -> napi::Result<svelte_compiler::PrintedOutput> {
    let ast = svelte_compiler::parse(
        &source,
        ParseOptions {
            modern: Some(true),
            ..Default::default()
        },
    )
    .map_err(compile_error_to_napi)?;

    let Root::Modern(root) = ast.root else {
        return Err(Error::from_reason(
            "print(source) requires a modern AST root".to_string(),
        ));
    };

    svelte_compiler::print_modern(
        svelte_compiler::ModernPrintTarget::root(&source, &root),
        options,
    )
    .map_err(compile_error_to_napi)
}

fn call_js_function<Return>(
    env: &Env,
    callback: &FunctionRef<(String,), Return>,
    payload: String,
) -> napi::Result<Return>
where
    Return: napi::bindgen_prelude::FromNapiValue,
{
    callback.borrow_back(env)?.call((payload,))
}

fn store_callback_error(target: &Arc<Mutex<Option<Error>>>, error: Error) {
    if let Ok(mut guard) = target.lock()
        && guard.is_none()
    {
        *guard = Some(error);
    }
}

fn take_callback_error(target: &Arc<Mutex<Option<Error>>>) -> Option<Error> {
    target.lock().ok().and_then(|mut guard| guard.take())
}

fn compile_error_to_napi(error: svelte_compiler::CompileError) -> Error {
    let payload = json!({
        "code": error.code.as_ref(),
        "message": error.message.as_ref(),
        "position": error.position.as_deref(),
        "start": error.start.as_deref(),
        "end": error.end.as_deref(),
        "filename": error.filename.as_deref().map(|path| path.as_str()),
    });

    match serde_json::to_string(&payload) {
        Ok(json) => Error::from_reason(json),
        Err(_) => Error::from_reason(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_returns_root_object() {
        let output = parse_json(
            "<h1>Hello</h1>".to_string(),
            Some("{\"modern\":true}".to_string()),
        )
        .expect("parse succeeds");
        let value: serde_json::Value = serde_json::from_str(&output).expect("valid json");
        assert_eq!(value["type"], "Root");
    }

    #[test]
    fn compile_json_returns_serialized_result() {
        let output = compile_json(
            "<h1>Hello</h1>".to_string(),
            Some(
                "{\"sourcemap\":{\"version\":3,\"sources\":[],\"names\":[],\"mappings\":\"\"}}"
                    .to_string(),
            ),
        )
        .expect("compile succeeds");
        let value: serde_json::Value = serde_json::from_str(&output).expect("valid json");
        assert!(value.get("js").is_some());
    }

    #[test]
    fn print_json_supports_modern_root() {
        let source = "<h1>Hello</h1>".to_string();
        let ast_json =
            parse_json(source.clone(), Some("{\"modern\":true}".to_string())).expect("parse");
        let printed = print_json("root".to_string(), source, ast_json, None).expect("print");
        let value: serde_json::Value = serde_json::from_str(&printed).expect("valid json");
        assert_eq!(value["code"], "<h1>Hello</h1>");
    }
}
