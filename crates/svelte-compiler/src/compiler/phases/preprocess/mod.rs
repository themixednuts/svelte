use std::borrow::Cow;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use futures::executor::block_on;

use crate::api::SourceMap;
use crate::{
    CompileError, PreprocessAttribute, PreprocessAttributeValue, PreprocessAttributes,
    PreprocessMarkup, PreprocessOptions, PreprocessResult, PreprocessTag, TagPreprocessor,
};

pub(crate) fn preprocess(
    source: &str,
    options: PreprocessOptions,
) -> Result<PreprocessResult, CompileError> {
    let filename = options.filename.as_deref();
    let mut code = Arc::<str>::from(source);
    let mut dependencies = Vec::<Utf8PathBuf>::new();
    let mut map = None;

    for group in options.groups.iter() {
        if let Some(markup) = &group.markup
            && let Some(output) = markup(PreprocessMarkup {
                content: &code,
                filename,
            })?
        {
            dependencies.extend(output.dependencies.iter().cloned());
            if output.map.is_some() {
                map = output.map;
            }
            code = output.code;
        }

        if let Some(markup) = &group.markup_async
            && let Some(output) = run_preprocess_future(markup(PreprocessMarkup {
                content: &code,
                filename,
            }))?
        {
            dependencies.extend(output.dependencies.iter().cloned());
            if output.map.is_some() {
                map = output.map;
            }
            code = output.code;
        }

        if let Some(script) = &group.script {
            let transformed = apply_tag_preprocessor(&code, "script", script, filename)?;
            dependencies.extend(transformed.dependencies);
            if transformed.map.is_some() {
                map = transformed.map;
            }
            code = transformed.code;
        }

        if let Some(script) = &group.script_async {
            let transformed = apply_async_tag_preprocessor(&code, "script", script, filename)?;
            dependencies.extend(transformed.dependencies);
            if transformed.map.is_some() {
                map = transformed.map;
            }
            code = transformed.code;
        }

        if let Some(style) = &group.style {
            let transformed = apply_tag_preprocessor(&code, "style", style, filename)?;
            dependencies.extend(transformed.dependencies);
            if transformed.map.is_some() {
                map = transformed.map;
            }
            code = transformed.code;
        }

        if let Some(style) = &group.style_async {
            let transformed = apply_async_tag_preprocessor(&code, "style", style, filename)?;
            dependencies.extend(transformed.dependencies);
            if transformed.map.is_some() {
                map = transformed.map;
            }
            code = transformed.code;
        }
    }

    Ok(PreprocessResult {
        code,
        dependencies: dependencies.into_boxed_slice(),
        map,
    })
}

fn run_preprocess_future<F>(future: F) -> Result<Option<crate::PreprocessOutput>, CompileError>
where
    F: Future<Output = Result<Option<crate::PreprocessOutput>, CompileError>>,
{
    block_on(future)
}

struct TagPassResult {
    code: Arc<str>,
    dependencies: Vec<Utf8PathBuf>,
    map: Option<SourceMap>,
}

fn apply_tag_preprocessor(
    source: &str,
    tag_name: &str,
    preprocessor: &TagPreprocessor,
    filename: Option<&Utf8Path>,
) -> Result<TagPassResult, CompileError> {
    let mut out = String::with_capacity(source.len());
    let mut dependencies = Vec::new();
    let mut map = None;
    let mut cursor = 0;
    let mut last_emitted = 0;

    while cursor < source.len() {
        let remaining = &source[cursor..];

        if remaining.starts_with("<!--") {
            cursor = skip_html_comment(source, cursor);
            continue;
        }

        if !remaining.starts_with('<') {
            cursor += 1;
            continue;
        }

        let Some(block) = parse_tag_block(source, cursor, tag_name) else {
            cursor += 1;
            continue;
        };

        out.push_str(&source[last_emitted..block.start]);

        let result = preprocessor(PreprocessTag {
            content: block.content,
            attributes: &block.attributes,
            filename,
        })?;

        if let Some(output) = result {
            dependencies.extend(output.dependencies.iter().cloned());
            if output.map.is_some() {
                map = output.map;
            }
            let open_tag = match output.attributes.as_deref() {
                Some(attributes) => render_open_tag(block.name, attributes),
                None if block.self_closing => {
                    format!("<{}{}>", block.name, block.raw_attributes)
                }
                None => block.open_tag.into_owned(),
            };

            out.push_str(&open_tag);
            out.push_str(&output.code);
            out.push_str("</");
            out.push_str(block.name);
            out.push('>');
        } else {
            out.push_str(&source[block.start..block.end]);
        }

        last_emitted = block.end;
        cursor = block.end;
    }

    out.push_str(&source[last_emitted..]);

    Ok(TagPassResult {
        code: Arc::from(out),
        dependencies,
        map,
    })
}

fn apply_async_tag_preprocessor(
    source: &str,
    tag_name: &str,
    preprocessor: &crate::AsyncTagPreprocessor,
    filename: Option<&Utf8Path>,
) -> Result<TagPassResult, CompileError> {
    let mut out = String::with_capacity(source.len());
    let mut dependencies = Vec::new();
    let mut map = None;
    let mut cursor = 0;
    let mut last_emitted = 0;

    while cursor < source.len() {
        let remaining = &source[cursor..];

        if remaining.starts_with("<!--") {
            cursor = skip_html_comment(source, cursor);
            continue;
        }

        if !remaining.starts_with('<') {
            cursor += 1;
            continue;
        }

        let Some(block) = parse_tag_block(source, cursor, tag_name) else {
            cursor += 1;
            continue;
        };

        out.push_str(&source[last_emitted..block.start]);

        let result = run_preprocess_future(preprocessor(PreprocessTag {
            content: block.content,
            attributes: &block.attributes,
            filename,
        }))?;

        if let Some(output) = result {
            dependencies.extend(output.dependencies.iter().cloned());
            if output.map.is_some() {
                map = output.map;
            }
            let open_tag = match output.attributes.as_deref() {
                Some(attributes) => render_open_tag(block.name, attributes),
                None if block.self_closing => {
                    format!("<{}{}>", block.name, block.raw_attributes)
                }
                None => block.open_tag.into_owned(),
            };

            out.push_str(&open_tag);
            out.push_str(&output.code);
            out.push_str("</");
            out.push_str(block.name);
            out.push('>');
        } else {
            out.push_str(&source[block.start..block.end]);
        }

        last_emitted = block.end;
        cursor = block.end;
    }

    out.push_str(&source[last_emitted..]);

    Ok(TagPassResult {
        code: Arc::from(out),
        dependencies,
        map,
    })
}

fn render_open_tag(tag_name: &str, attributes: &[PreprocessAttribute]) -> String {
    let mut rendered = String::from("<");
    rendered.push_str(tag_name);

    for attribute in attributes {
        rendered.push(' ');
        rendered.push_str(&attribute.name);
        match &attribute.value {
            PreprocessAttributeValue::Bool(true) => {}
            PreprocessAttributeValue::Bool(false) => {
                rendered.push_str("=\"false\"");
            }
            PreprocessAttributeValue::String(value) => {
                rendered.push_str("=\"");
                rendered.push_str(value);
                rendered.push('"');
            }
        }
    }

    rendered.push('>');
    rendered
}

struct TagBlock<'a> {
    start: usize,
    end: usize,
    name: &'a str,
    content: &'a str,
    attributes: PreprocessAttributes,
    open_tag: Cow<'a, str>,
    raw_attributes: &'a str,
    self_closing: bool,
}

fn parse_tag_block<'a>(source: &'a str, start: usize, tag_name: &str) -> Option<TagBlock<'a>> {
    let bytes = source.as_bytes();
    let name_start = start.checked_add(1)?;
    let name_end = name_start.checked_add(tag_name.len())?;
    let actual_name = source.get(name_start..name_end)?;

    if actual_name != tag_name {
        return None;
    }

    let next = bytes.get(name_end).copied();
    if !matches!(
        next,
        Some(b'>') | Some(b'/') | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n')
    ) {
        return None;
    }

    let open_end = find_tag_open_end(source, start)?;
    let tag_close = open_end.checked_sub(1)?;
    let slash_index = find_self_closing_slash(source, name_end, tag_close);
    let raw_attributes_end = slash_index.unwrap_or(tag_close);
    let raw_attributes = source.get(name_end..raw_attributes_end)?;
    let open_tag = source.get(start..open_end)?;
    let attributes = parse_attributes(raw_attributes);

    if let Some(slash_index) = slash_index
        && only_whitespace(source.get(slash_index + 1..tag_close)?)
    {
        return Some(TagBlock {
            start,
            end: open_end,
            name: actual_name,
            content: "",
            attributes,
            open_tag: Cow::Borrowed(open_tag),
            raw_attributes,
            self_closing: true,
        });
    }

    let closing = format!("</{tag_name}>");
    let relative_close = source.get(open_end..)?.find(&closing)?;
    let content_end = open_end + relative_close;
    let end = content_end + closing.len();

    Some(TagBlock {
        start,
        end,
        name: actual_name,
        content: source.get(open_end..content_end)?,
        attributes,
        open_tag: Cow::Borrowed(open_tag),
        raw_attributes,
        self_closing: false,
    })
}

fn only_whitespace(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
}

fn find_tag_open_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = start + 1;
    let mut quote = None;

    while let Some(&byte) = bytes.get(index) {
        match quote {
            Some(active) if byte == active => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Some(index + 1),
            _ => {}
        }
        index += 1;
    }

    None
}

fn find_self_closing_slash(source: &str, start: usize, end: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut quote = None;
    let mut index = start;
    let mut slash = None;

    while index < end {
        let byte = bytes[index];
        match quote {
            Some(active) if byte == active => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'/' => slash = Some(index),
            None if !matches!(byte, b' ' | b'\t' | b'\r' | b'\n') => slash = None,
            _ => {}
        }
        index += 1;
    }

    slash
}

fn parse_attributes(raw: &str) -> PreprocessAttributes {
    let bytes = raw.as_bytes();
    let mut attributes = BTreeMap::new();
    let mut index = 0;

    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }

        if index >= bytes.len() {
            break;
        }

        let name_start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && !matches!(bytes[index], b'=' | b'/' | b'>')
        {
            index += 1;
        }

        if name_start == index {
            index += 1;
            continue;
        }

        let name = &raw[name_start..index];

        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }

        let value = if index < bytes.len() && bytes[index] == b'=' {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }

            if index >= bytes.len() {
                PreprocessAttributeValue::String(Arc::from(""))
            } else {
                let value = if matches!(bytes[index], b'"' | b'\'') {
                    let quote = bytes[index];
                    index += 1;
                    let value_start = index;
                    while index < bytes.len() && bytes[index] != quote {
                        index += 1;
                    }
                    let value = Arc::<str>::from(&raw[value_start..index]);
                    if index < bytes.len() {
                        index += 1;
                    }
                    value
                } else {
                    let value_start = index;
                    while index < bytes.len()
                        && !bytes[index].is_ascii_whitespace()
                        && !matches!(bytes[index], b'>' | b'/')
                    {
                        index += 1;
                    }
                    Arc::<str>::from(&raw[value_start..index])
                };
                PreprocessAttributeValue::String(value)
            }
        } else {
            PreprocessAttributeValue::Bool(true)
        };

        attributes.insert(Arc::from(name), value);
    }

    attributes
}

fn skip_html_comment(source: &str, start: usize) -> usize {
    source
        .get(start + 4..)
        .and_then(|tail| tail.find("-->").map(|end| start + 4 + end + 3))
        .unwrap_or(source.len())
}
