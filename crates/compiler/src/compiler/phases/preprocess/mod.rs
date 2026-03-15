use std::borrow::Cow;
use std::collections::BTreeMap;
use std::future::{Future, ready};
use std::pin::Pin;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use camino::{Utf8Path, Utf8PathBuf};
use futures::executor::block_on;
use rustc_hash::FxHashSet;

use crate::api::SourceMap;
use crate::compiler::phases::transform::sourcemap;
use crate::{
    CompileError, PreprocessAttribute, PreprocessAttributeValue, PreprocessAttributes,
    PreprocessMarkup, PreprocessOptions, PreprocessResult, PreprocessTag,
};

pub(crate) fn preprocess(
    source: &str,
    options: PreprocessOptions,
) -> Result<PreprocessResult, CompileError> {
    block_on(preprocess_async(source, options))
}

pub(crate) async fn preprocess_async(
    source: &str,
    options: PreprocessOptions,
) -> Result<PreprocessResult, CompileError> {
    let filename = options.filename.as_deref();
    let mut state = PreprocessState::new(source);

    for group in options.groups.iter() {
        if let Some(markup) = &group.markup
            && let Some(output) = markup(PreprocessMarkup {
                content: state.code.as_ref(),
                filename,
            })?
        {
            state.apply_output(output);
        }

        if let Some(markup) = &group.markup_async
            && let Some(output) = markup(PreprocessMarkup {
                content: state.code.as_ref(),
                filename,
            })
            .await?
        {
            state.apply_output(output);
        }

        if let Some(script) = &group.script {
            let transformed = apply_tag_pass(state.code.as_ref(), "script", filename, |tag| {
                Box::pin(ready(script(tag)))
            })
            .await?;
            state.apply_tag_pass(transformed);
        }

        if let Some(script) = &group.script_async {
            let transformed =
                apply_tag_pass(state.code.as_ref(), "script", filename, |tag| script(tag)).await?;
            state.apply_tag_pass(transformed);
        }

        if let Some(style) = &group.style {
            let transformed = apply_tag_pass(state.code.as_ref(), "style", filename, |tag| {
                Box::pin(ready(style(tag)))
            })
            .await?;
            state.apply_tag_pass(transformed);
        }

        if let Some(style) = &group.style_async {
            let transformed =
                apply_tag_pass(state.code.as_ref(), "style", filename, |tag| style(tag)).await?;
            state.apply_tag_pass(transformed);
        }
    }

    Ok(state.finish())
}

#[derive(Default)]
struct DependencyCollector {
    ordered: Vec<Utf8PathBuf>,
    seen: FxHashSet<Utf8PathBuf>,
}

impl DependencyCollector {
    fn extend<I>(&mut self, dependencies: I)
    where
        I: IntoIterator<Item = Utf8PathBuf>,
    {
        for dependency in dependencies {
            if self.seen.insert(dependency.clone()) {
                self.ordered.push(dependency);
            }
        }
    }

    fn into_boxed_slice(self) -> Box<[Utf8PathBuf]> {
        self.ordered.into_boxed_slice()
    }
}

#[derive(Default)]
struct PreprocessState {
    code: Arc<str>,
    dependencies: DependencyCollector,
    map: Option<SourceMap>,
}

impl PreprocessState {
    fn new(source: &str) -> Self {
        Self {
            code: Arc::from(source),
            ..Self::default()
        }
    }

    fn apply_output(&mut self, output: crate::PreprocessOutput) {
        self.dependencies.extend(output.dependencies.into_vec());
        self.compose_map(output.map);
        self.code = output.code;
    }

    fn apply_tag_pass(&mut self, result: TagPassResult) {
        self.dependencies.extend(result.dependencies);
        self.compose_map(result.map);
        self.code = result.code;
    }

    fn compose_map(&mut self, next: Option<SourceMap>) {
        let Some(next) = next else {
            return;
        };

        self.map = match self.map.take() {
            Some(current) if has_map_content(&current) => {
                Some(sourcemap::compose_sourcemaps(&next, &current))
            }
            _ => Some(next),
        };
    }

    fn finish(self) -> PreprocessResult {
        PreprocessResult {
            code: self.code,
            dependencies: self.dependencies.into_boxed_slice(),
            map: self.map,
        }
    }
}

fn has_map_content(map: &SourceMap) -> bool {
    !map.mappings.is_empty() || !map.sources.is_empty() || !map.names.is_empty()
}

struct TagPassResult {
    code: Arc<str>,
    dependencies: Vec<Utf8PathBuf>,
    map: Option<SourceMap>,
}

struct TagMapOverlayRequest {
    generated_start: usize,
    original_start: usize,
    original_tag_open_len: usize,
    original_content_len: usize,
    original_end: usize,
    tag_head_len: usize,
    generated_tag_open: Arc<str>,
    generated_content: Arc<str>,
    generated_tag_close: Arc<str>,
    content_map: SourceMap,
}

type TagPassFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<crate::PreprocessOutput>, CompileError>> + 'a>>;

async fn apply_tag_pass(
    source: &str,
    tag_name: &str,
    filename: Option<&Utf8Path>,
    mut preprocessor: impl for<'a> FnMut(PreprocessTag<'a>) -> TagPassFuture<'a>,
) -> Result<TagPassResult, CompileError> {
    let mut out = String::with_capacity(source.len());
    let mut dependencies = Vec::new();
    let mut overlays = Vec::new();
    let mut direct_maps = Vec::new();
    let mut last_emitted = 0;

    for block in TagScanner::new(source, tag_name) {
        out.push_str(&source[last_emitted..block.start]);

        let result = preprocessor(PreprocessTag {
            content: block.content,
            attributes: &block.attributes,
            markup: source,
            filename,
        })
        .await?;

        if let Some(output) = result {
            let output = normalize_tag_output(block.name, output)?;
            dependencies.extend(output.dependencies.iter().cloned());
            let original_tag_open_len = block.open_tag.len();
            let open_tag = match output.attributes.as_deref() {
                Some(attributes) => render_open_tag(block.name, attributes),
                None if block.self_closing => {
                    format!("<{}{}>", block.name, block.raw_attributes)
                }
                None => block.open_tag.into_owned(),
            };
            let generated_start = out.len();
            let generated_close_tag = Arc::<str>::from(format!("</{}>", block.name));

            if let Some(content_map) = output.map {
                if should_wrap_tag_map(output.code.as_ref(), filename, &content_map) {
                    overlays.push(TagMapOverlayRequest {
                        generated_start,
                        original_start: block.start,
                        original_tag_open_len,
                        original_content_len: block.content.len(),
                        original_end: block.end,
                        tag_head_len: block.name.len() + 1,
                        generated_tag_open: Arc::from(open_tag.as_str()),
                        generated_content: output.code.clone(),
                        generated_tag_close: generated_close_tag.clone(),
                        content_map,
                    });
                } else {
                    direct_maps.push(content_map);
                }
            }

            out.push_str(&open_tag);
            out.push_str(&output.code);
            out.push_str(&generated_close_tag);
        } else {
            out.push_str(&source[block.start..block.end]);
        }

        last_emitted = block.end;
    }

    out.push_str(&source[last_emitted..]);
    let map = build_tag_pass_map(source, out.as_str(), filename, &overlays, direct_maps);

    Ok(TagPassResult {
        code: Arc::from(out),
        dependencies,
        map,
    })
}

fn build_tag_pass_map(
    source: &str,
    output: &str,
    filename: Option<&Utf8Path>,
    overlays: &[TagMapOverlayRequest],
    direct_maps: Vec<SourceMap>,
) -> Option<SourceMap> {
    direct_maps
        .into_iter()
        .chain(
            overlays
                .iter()
                .map(|overlay| build_tag_map_overlay(source, output, filename, overlay)),
        )
        .reduce(|merged, overlay| sourcemap::merge_sourcemaps(&merged, &overlay))
}

fn should_wrap_tag_map(content: &str, filename: Option<&Utf8Path>, map: &SourceMap) -> bool {
    if !has_map_content(map) || !map_generated_positions_fit_content(map, content) {
        return false;
    }

    match filename.and_then(Utf8Path::file_name) {
        Some(filename) => map.file.as_deref().is_none_or(|file| file == filename),
        None => true,
    }
}

fn map_generated_positions_fit_content(map: &SourceMap, content: &str) -> bool {
    let content_lines = LineIndex::new(content);
    let decoded = sourcemap::decode_mappings(map.mappings.as_ref());

    if decoded.len() > content_lines.line_count() {
        return false;
    }

    decoded.iter().enumerate().all(|(line_index, segments)| {
        let Some(line_len) = content_lines.line_len(line_index) else {
            return false;
        };
        segments
            .iter()
            .all(|segment| segment.generated_column <= line_len)
    })
}

fn build_tag_map_overlay(
    source: &str,
    output: &str,
    filename: Option<&Utf8Path>,
    request: &TagMapOverlayRequest,
) -> SourceMap {
    let output_lines = LineIndex::new(output);
    let source_lines = LineIndex::new(source);
    let content_lines = LineIndex::new(request.generated_content.as_ref());
    let decoded_content = sourcemap::decode_mappings(request.content_map.mappings.as_ref());

    let mut sources = request.content_map.sources.to_vec();
    let mut source_lookup = BTreeMap::new();
    for (index, value) in sources.iter().cloned().enumerate() {
        source_lookup.insert(value, index);
    }

    let component_source = component_source_name(filename, &request.content_map);
    let component_source_index =
        intern_arc(&mut sources, &mut source_lookup, component_source.clone());
    let component_input_index = request
        .content_map
        .sources
        .iter()
        .position(|source_name| source_name.as_ref() == component_source.as_ref());

    let mut names = request.content_map.names.to_vec();
    let mut name_lookup = BTreeMap::new();
    for (index, value) in names.iter().cloned().enumerate() {
        name_lookup.insert(value, index);
    }

    let mut lines = (0..output_lines.line_count())
        .map(|_| BTreeMap::new())
        .collect::<Vec<BTreeMap<usize, sourcemap::DecodedSegment>>>();
    let original_close_len = request.original_end
        - (request.original_start + request.original_tag_open_len + request.original_content_len);

    insert_offset_segment(
        &mut lines,
        &output_lines,
        &source_lines,
        request.generated_start,
        component_source_index,
        request.original_start,
        None,
    );
    if request.original_tag_open_len != request.generated_tag_open.len() {
        insert_offset_segment(
            &mut lines,
            &output_lines,
            &source_lines,
            request.generated_start + request.tag_head_len,
            component_source_index,
            request.original_start + request.tag_head_len,
            None,
        );
        insert_offset_segment(
            &mut lines,
            &output_lines,
            &source_lines,
            request.generated_start + request.generated_tag_open.len(),
            component_source_index,
            request.original_start + request.original_tag_open_len,
            None,
        );
    }

    let generated_content_start = request.generated_start + request.generated_tag_open.len();
    let original_content_start = request.original_start + request.original_tag_open_len;
    let (content_base_line, content_base_column) = source_lines.line_col(original_content_start);

    for (line_index, segments) in decoded_content.iter().enumerate() {
        for segment in segments {
            let Some(generated_within_content) =
                content_lines.offset(line_index, segment.generated_column)
            else {
                continue;
            };
            let generated_offset = generated_content_start + generated_within_content;

            let source_name = request
                .content_map
                .sources
                .get(segment.source_index)
                .cloned()
                .unwrap_or_else(|| component_source.clone());
            let source_index = intern_arc(&mut sources, &mut source_lookup, source_name.clone());

            let (original_line, original_column) = if Some(segment.source_index)
                == component_input_index
                || source_name.as_ref() == component_source.as_ref()
            {
                offset_position(
                    content_base_line,
                    content_base_column,
                    segment.original_line,
                    segment.original_column,
                )
            } else {
                (segment.original_line, segment.original_column)
            };

            let name_index = segment
                .name_index
                .and_then(|index| request.content_map.names.get(index).cloned())
                .map(|name| intern_arc(&mut names, &mut name_lookup, name));

            insert_line_column_segment(
                &mut lines,
                &output_lines,
                generated_offset,
                source_index,
                original_line,
                original_column,
                name_index,
            );
        }
    }

    let generated_close_start = request.generated_start
        + request.generated_tag_open.len()
        + request.generated_content.len();
    let original_close_start = original_content_start + request.original_content_len;

    insert_offset_segment(
        &mut lines,
        &output_lines,
        &source_lines,
        generated_close_start,
        component_source_index,
        original_close_start,
        None,
    );
    if original_close_len != request.generated_tag_close.len() {
        insert_offset_segment(
            &mut lines,
            &output_lines,
            &source_lines,
            generated_close_start + request.generated_tag_close.len(),
            component_source_index,
            request.original_end,
            None,
        );
    }

    SourceMap {
        version: 3,
        file: None,
        source_root: None,
        sources: sources.into_boxed_slice(),
        sources_content: None,
        names: names.into_boxed_slice(),
        mappings: sourcemap::encode_decoded_mappings(
            &lines
                .into_iter()
                .map(BTreeMap::into_values)
                .map(Iterator::collect)
                .collect::<Vec<Vec<sourcemap::DecodedSegment>>>(),
        ),
    }
}

fn component_source_name(filename: Option<&Utf8Path>, map: &SourceMap) -> Arc<str> {
    filename
        .and_then(Utf8Path::file_name)
        .map(Arc::<str>::from)
        .or_else(|| map.sources.first().cloned())
        .unwrap_or_else(|| Arc::from("source.svelte"))
}

fn intern_arc(
    values: &mut Vec<Arc<str>>,
    lookup: &mut BTreeMap<Arc<str>, usize>,
    value: Arc<str>,
) -> usize {
    if let Some(index) = lookup.get(&value).copied() {
        return index;
    }

    let index = values.len();
    values.push(value.clone());
    lookup.insert(value, index);
    index
}

fn offset_position(
    base_line: usize,
    base_column: usize,
    relative_line: usize,
    relative_column: usize,
) -> (usize, usize) {
    if relative_line == 0 {
        (base_line, base_column + relative_column)
    } else {
        (base_line + relative_line, relative_column)
    }
}

fn insert_offset_segment(
    lines: &mut [BTreeMap<usize, sourcemap::DecodedSegment>],
    output_lines: &LineIndex,
    source_lines: &LineIndex,
    generated_offset: usize,
    source_index: usize,
    original_offset: usize,
    name_index: Option<usize>,
) {
    let (original_line, original_column) = source_lines.line_col(original_offset);
    insert_line_column_segment(
        lines,
        output_lines,
        generated_offset,
        source_index,
        original_line,
        original_column,
        name_index,
    );
}

fn insert_line_column_segment(
    lines: &mut [BTreeMap<usize, sourcemap::DecodedSegment>],
    output_lines: &LineIndex,
    generated_offset: usize,
    source_index: usize,
    original_line: usize,
    original_column: usize,
    name_index: Option<usize>,
) {
    let (generated_line, generated_column) = output_lines.line_col(generated_offset);
    let Some(line) = lines.get_mut(generated_line) else {
        return;
    };

    line.insert(
        generated_column,
        sourcemap::DecodedSegment {
            generated_column,
            source_index,
            original_line,
            original_column,
            name_index,
        },
    );
}

struct LineIndex {
    starts: Vec<usize>,
    text_len: usize,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        for (index, ch) in text.char_indices() {
            if ch == '\n' {
                starts.push(index + 1);
            }
        }
        Self {
            starts,
            text_len: text.len(),
        }
    }

    fn line_count(&self) -> usize {
        self.starts.len()
    }

    fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = match self.starts.binary_search(&offset) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        let column = offset.saturating_sub(self.starts[line]);
        (line, column)
    }

    fn offset(&self, line: usize, column: usize) -> Option<usize> {
        self.starts.get(line).map(|start| start + column)
    }

    fn line_len(&self, line: usize) -> Option<usize> {
        let start = *self.starts.get(line)?;
        let end = self
            .starts
            .get(line + 1)
            .map_or(self.text_len, |next| next.saturating_sub(1));
        Some(end.saturating_sub(start))
    }
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

struct TagScanner<'a> {
    source: &'a str,
    tag_name: &'a str,
    cursor: usize,
}

impl<'a> TagScanner<'a> {
    fn new(source: &'a str, tag_name: &'a str) -> Self {
        Self {
            source,
            tag_name,
            cursor: 0,
        }
    }
}

impl<'a> Iterator for TagScanner<'a> {
    type Item = TagBlock<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.cursor < self.source.len() {
            let remaining = &self.source[self.cursor..];

            if remaining.starts_with("<!--") {
                self.cursor = skip_html_comment(self.source, self.cursor);
                continue;
            }

            if !remaining.starts_with('<') {
                self.cursor += 1;
                continue;
            }

            let Some(block) = parse_tag_block(self.source, self.cursor, self.tag_name) else {
                self.cursor += 1;
                continue;
            };

            self.cursor = block.end;
            return Some(block);
        }

        None
    }
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

fn normalize_tag_output(
    tag_name: &str,
    mut output: crate::PreprocessOutput,
) -> Result<crate::PreprocessOutput, CompileError> {
    let Some(attached) = parse_attached_sourcemap(tag_name, output.code.as_ref())? else {
        return Ok(output);
    };

    output.code = attached.code;
    if output.map.is_none() {
        output.map = attached.map;
    }

    Ok(output)
}

fn parse_attached_sourcemap(
    tag_name: &str,
    code: &str,
) -> Result<Option<AttachedSourcemap>, CompileError> {
    let Some(comment) = find_trailing_sourcemap_comment(tag_name, code) else {
        return Ok(None);
    };

    let stripped = Arc::<str>::from(format!(
        "{}{}",
        &code[..comment.start],
        &code[comment.end..]
    ));
    let map = decode_sourcemap_url(comment.url)?;

    Ok(Some(AttachedSourcemap {
        code: stripped,
        map,
    }))
}

fn find_trailing_sourcemap_comment<'a>(
    tag_name: &str,
    code: &'a str,
) -> Option<SourcemapComment<'a>> {
    parse_trailing_block_sourcemap_comment(code).or_else(|| {
        if tag_name == "script" {
            parse_trailing_line_sourcemap_comment(code)
        } else {
            None
        }
    })
}

fn parse_trailing_block_sourcemap_comment(code: &str) -> Option<SourcemapComment<'_>> {
    let trimmed_end = code.trim_end_matches(char::is_whitespace).len();
    let relevant = &code[..trimmed_end];
    let start = relevant.rfind("/*")?;
    let body = relevant.get(start + 2..relevant.len().checked_sub(2)?)?;
    let url = parse_sourcemap_directive(body.trim())?;

    Some(SourcemapComment {
        start,
        end: trimmed_end,
        url,
    })
}

fn parse_trailing_line_sourcemap_comment(code: &str) -> Option<SourcemapComment<'_>> {
    let trimmed_end = code.trim_end_matches(char::is_whitespace).len();
    let relevant = &code[..trimmed_end];
    let line_start = relevant.rfind('\n').map_or(0, |index| index + 1);
    let body = relevant.get(line_start..)?;
    let url = parse_sourcemap_directive(body.trim_start_matches(char::is_whitespace))?;

    Some(SourcemapComment {
        start: line_start,
        end: trimmed_end,
        url,
    })
}

fn parse_sourcemap_directive(comment: &str) -> Option<&str> {
    let directive = comment
        .strip_prefix("//")
        .or_else(|| comment.strip_prefix("/*"))
        .unwrap_or(comment)
        .trim();
    let directive = directive
        .strip_prefix('#')
        .or_else(|| directive.strip_prefix('@'))?
        .trim_start();
    let remainder = directive.strip_prefix("sourceMappingURL")?.trim_start();
    let url = remainder.strip_prefix('=')?.trim_start();
    (!url.is_empty()).then_some(url)
}

fn decode_sourcemap_url(url: &str) -> Result<Option<SourceMap>, CompileError> {
    let Some(data) = url.strip_prefix("data:") else {
        return Ok(None);
    };
    let Some((metadata, payload)) = data.split_once(',') else {
        return Ok(None);
    };
    let metadata = metadata.to_ascii_lowercase();

    if !metadata.contains("json") || !metadata.contains("base64") {
        return Ok(None);
    }

    let decoded = BASE64_STANDARD.decode(payload).map_err(|error| {
        CompileError::internal(Arc::from(format!("invalid attached sourcemap: {error}")))
    })?;
    let map = serde_json::from_slice::<SourceMap>(&decoded).map_err(|error| {
        CompileError::internal(Arc::from(format!(
            "invalid attached sourcemap json: {error}"
        )))
    })?;

    Ok(Some(map))
}

struct SourcemapComment<'a> {
    start: usize,
    end: usize,
    url: &'a str,
}

struct AttachedSourcemap {
    code: Arc<str>,
    map: Option<SourceMap>,
}

fn skip_html_comment(source: &str, start: usize) -> usize {
    source
        .get(start + 4..)
        .and_then(|tail| tail.find("-->").map(|end| start + 4 + end + 3))
        .unwrap_or(source.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    use camino::Utf8PathBuf;

    use crate::{PreprocessorGroup, SourceMap};

    #[test]
    fn preprocess_wraps_content_local_script_maps() {
        let source = "<script>\n\tconsole.log(__THE_ANSWER__);\n</script>";
        let expected_full_map = SourceMap {
            version: 3,
            file: None,
            source_root: None,
            sources: vec![Arc::from("input.svelte")].into_boxed_slice(),
            sources_content: None,
            names: Vec::<Arc<str>>::new().into_boxed_slice(),
            mappings: Arc::from(
                "AAAA;AACA,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,CAAC,EAAc,CAAC;AAC5B",
            ),
        };
        let local_content_map = to_content_local_map(
            &expected_full_map,
            source,
            "<script>".len(),
            source.len() - "</script>".len(),
        );
        let local_content_map_without_file = SourceMap {
            file: None,
            ..local_content_map.clone()
        };

        let result = preprocess(
            source,
            PreprocessOptions {
                filename: Some(Utf8PathBuf::from("input.svelte")),
                groups: vec![PreprocessorGroup {
                    script: Some(Arc::new(move |script| {
                        Ok(Some(crate::PreprocessOutput {
                            code: Arc::from(script.content.replace("__THE_ANSWER__", "42")),
                            map: Some(local_content_map.clone()),
                            ..crate::PreprocessOutput::default()
                        }))
                    })),
                    ..PreprocessorGroup::default()
                }]
                .into_boxed_slice(),
            },
        )
        .expect("preprocess succeeds");

        assert_eq!(result.map, Some(expected_full_map.clone()));

        let result_without_file = preprocess(
            source,
            PreprocessOptions {
                filename: Some(Utf8PathBuf::from("input.svelte")),
                groups: vec![PreprocessorGroup {
                    script: Some(Arc::new(move |script| {
                        Ok(Some(crate::PreprocessOutput {
                            code: Arc::from(script.content.replace("__THE_ANSWER__", "42")),
                            map: Some(local_content_map_without_file.clone()),
                            ..crate::PreprocessOutput::default()
                        }))
                    })),
                    ..PreprocessorGroup::default()
                }]
                .into_boxed_slice(),
            },
        )
        .expect("preprocess succeeds without file hint");

        assert_eq!(result_without_file.map, Some(expected_full_map));
    }

    #[test]
    fn preprocess_parses_attached_script_sourcemap_data_urls() {
        let map_json = r#"{"version":3,"sources":["input.svelte"],"names":[],"mappings":"AAAA"}"#;
        let map_data = BASE64_STANDARD.encode(map_json);
        let source = "<script>console.log('x');</script>";

        let result = preprocess(
            source,
            PreprocessOptions {
                groups: vec![PreprocessorGroup {
                    script: Some(Arc::new(move |_| {
                        Ok(Some(crate::PreprocessOutput {
                            code: Arc::from(format!(
                                "console.log('y');\n//# sourceMappingURL=data:application/json;charset=utf-8;base64,{map_data}"
                            )),
                            ..crate::PreprocessOutput::default()
                        }))
                    })),
                    ..PreprocessorGroup::default()
                }]
                .into_boxed_slice(),
                filename: Some(Utf8PathBuf::from("input.svelte")),
            },
        )
        .expect("preprocess succeeds");

        assert_eq!(result.code.as_ref(), "<script>console.log('y');\n</script>");
        let map = result.map.expect("attached sourcemap should decode");
        assert_eq!(map.file, None);
        assert_eq!(map.sources.as_ref(), &[Arc::from("input.svelte")]);
        assert!(!map.mappings.is_empty());
    }

    fn to_content_local_map(
        full_map: &SourceMap,
        full_source: &str,
        content_start: usize,
        content_end: usize,
    ) -> SourceMap {
        let full_lines = LineIndex::new(full_source);
        let content_lines = LineIndex::new(&full_source[content_start..content_end]);
        let decoded = sourcemap::decode_mappings(full_map.mappings.as_ref());
        let mut local_lines = (0..content_lines.line_count())
            .map(|_| Vec::<sourcemap::DecodedSegment>::new())
            .collect::<Vec<_>>();
        let (content_start_line, content_start_column) = full_lines.line_col(content_start);

        for (generated_line, segments) in decoded.iter().enumerate() {
            for segment in segments {
                let Some(generated_offset) =
                    full_lines.offset(generated_line, segment.generated_column)
                else {
                    continue;
                };
                if !(content_start..content_end).contains(&generated_offset) {
                    continue;
                }

                let local_generated_offset = generated_offset - content_start;
                let (local_generated_line, local_generated_column) =
                    content_lines.line_col(local_generated_offset);

                let (original_line, original_column) = relative_position(
                    content_start_line,
                    content_start_column,
                    segment.original_line,
                    segment.original_column,
                );

                local_lines[local_generated_line].push(sourcemap::DecodedSegment {
                    generated_column: local_generated_column,
                    source_index: segment.source_index,
                    original_line,
                    original_column,
                    name_index: segment.name_index,
                });
            }
        }

        SourceMap {
            version: 3,
            file: Some(Arc::from("input.svelte")),
            source_root: None,
            sources: full_map.sources.clone(),
            sources_content: None,
            names: full_map.names.clone(),
            mappings: sourcemap::encode_decoded_mappings(&local_lines),
        }
    }

    fn relative_position(
        base_line: usize,
        base_column: usize,
        absolute_line: usize,
        absolute_column: usize,
    ) -> (usize, usize) {
        if absolute_line == base_line {
            (0, absolute_column - base_column)
        } else {
            (absolute_line - base_line, absolute_column)
        }
    }
}
