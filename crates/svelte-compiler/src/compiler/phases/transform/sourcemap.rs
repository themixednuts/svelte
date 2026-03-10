use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

use crate::api::SourceMap;

const BASE64_VLQ: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecodedSegment {
    pub generated_column: usize,
    pub source_index: usize,
    pub original_line: usize,
    pub original_column: usize,
    pub name_index: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TracedSegment {
    pub source_index: usize,
    pub original_line: usize,
    pub original_column: usize,
    pub name_index: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct GeneratedSegment {
    generated_offset: usize,
    source_index: usize,
    original_offset: usize,
    name_index: Option<usize>,
    priority: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceMapSource<'a> {
    pub filename: Arc<str>,
    pub code: &'a str,
}

#[derive(Debug, Clone)]
pub(crate) struct SparseMappingHint<'a> {
    pub original: &'a str,
    pub generated: &'a str,
    pub name: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub(crate) struct SparseMappingOptions<'a> {
    pub output: &'a str,
    pub output_filename: Option<&'a Utf8Path>,
    pub sources: Vec<SourceMapSource<'a>>,
    pub hints: Vec<SparseMappingHint<'a>>,
}

pub(crate) fn build_sparse_sourcemap(options: SparseMappingOptions<'_>) -> SourceMap {
    let source_paths = options
        .sources
        .iter()
        .map(|source| relativize_source_path(options.output_filename, source.filename.as_ref()))
        .collect::<Vec<_>>();

    let mut names = Vec::<Arc<str>>::new();
    let mut name_index_by_value = BTreeMap::<Arc<str>, usize>::new();
    let mut segments_by_generated = BTreeMap::<usize, GeneratedSegment>::new();

    for (source_index, source) in options.sources.iter().enumerate() {
        add_sparse_identity_matches(
            options.output,
            source.code,
            source_index,
            &mut segments_by_generated,
            &mut names,
            &mut name_index_by_value,
        );
    }

    for hint in &options.hints {
        for (source_index, source) in options.sources.iter().enumerate() {
            add_sparse_hint_matches(
                options.output,
                source.code,
                source_index,
                hint,
                &mut segments_by_generated,
                &mut names,
                &mut name_index_by_value,
            );
        }
    }

    let output_lines = LineIndex::new(options.output);
    let source_lines = options
        .sources
        .iter()
        .map(|source| LineIndex::new(source.code))
        .collect::<Vec<_>>();

    let mappings = encode_segments(
        segments_by_generated.values().copied().collect(),
        &output_lines,
        &source_lines,
    );

    SourceMap {
        version: 3,
        file: options
            .output_filename
            .and_then(Utf8Path::file_name)
            .map(Arc::from),
        source_root: None,
        sources: source_paths.into_boxed_slice(),
        sources_content: None,
        names: names.into_boxed_slice(),
        mappings: Arc::from(mappings),
    }
}

pub(crate) fn decode_mappings(mappings: &str) -> Vec<Vec<DecodedSegment>> {
    let mut lines = Vec::<Vec<DecodedSegment>>::new();

    let mut source_index = 0i64;
    let mut original_line = 0i64;
    let mut original_column = 0i64;
    let mut name_index = 0i64;

    for line in mappings.split(';') {
        let mut decoded_line = Vec::new();
        let mut generated_column = 0i64;

        if !line.is_empty() {
            for entry in line.split(',').filter(|entry| !entry.is_empty()) {
                let mut cursor = 0usize;
                generated_column += decode_vlq_value(entry, &mut cursor);
                source_index += decode_vlq_value(entry, &mut cursor);
                original_line += decode_vlq_value(entry, &mut cursor);
                original_column += decode_vlq_value(entry, &mut cursor);
                let decoded_name = if cursor < entry.len() {
                    name_index += decode_vlq_value(entry, &mut cursor);
                    Some(name_index as usize)
                } else {
                    None
                };

                decoded_line.push(DecodedSegment {
                    generated_column: generated_column as usize,
                    source_index: source_index as usize,
                    original_line: original_line as usize,
                    original_column: original_column as usize,
                    name_index: decoded_name,
                });
            }
        }

        lines.push(decoded_line);
    }

    lines
}

pub(crate) fn encode_decoded_mappings(lines: &[Vec<DecodedSegment>]) -> Arc<str> {
    let mut encoded = String::new();
    let mut previous_source = 0i64;
    let mut previous_original_line = 0i64;
    let mut previous_original_column = 0i64;
    let mut previous_name = 0i64;

    for (line_index, line) in lines.iter().enumerate() {
        if line_index > 0 {
            encoded.push(';');
        }

        let mut previous_generated_column = 0i64;
        for (entry_index, segment) in line.iter().enumerate() {
            if entry_index > 0 {
                encoded.push(',');
            }

            encode_vlq_value(
                segment.generated_column as i64 - previous_generated_column,
                &mut encoded,
            );
            previous_generated_column = segment.generated_column as i64;

            encode_vlq_value(segment.source_index as i64 - previous_source, &mut encoded);
            previous_source = segment.source_index as i64;

            encode_vlq_value(
                segment.original_line as i64 - previous_original_line,
                &mut encoded,
            );
            previous_original_line = segment.original_line as i64;

            encode_vlq_value(
                segment.original_column as i64 - previous_original_column,
                &mut encoded,
            );
            previous_original_column = segment.original_column as i64;

            if let Some(name_index) = segment.name_index {
                encode_vlq_value(name_index as i64 - previous_name, &mut encoded);
                previous_name = name_index as i64;
            }
        }
    }

    Arc::from(encoded)
}

pub(crate) fn trace_original_position(
    map: &SourceMap,
    decoded: &[Vec<DecodedSegment>],
    generated_line: usize,
    generated_column: usize,
) -> Option<TracedSegment> {
    let line = decoded.get(generated_line)?;
    let mut last = None;

    for segment in line {
        if segment.generated_column > generated_column {
            break;
        }
        last = Some(*segment);
    }

    let segment = last?;
    Some(TracedSegment {
        source_index: segment.source_index,
        original_line: segment.original_line,
        original_column: segment.original_column + (generated_column - segment.generated_column),
        name_index: segment
            .name_index
            .filter(|index| map.names.get(*index).is_some()),
    })
}

pub(crate) fn compose_sourcemaps(outer: &SourceMap, inner: &SourceMap) -> SourceMap {
    let decoded_outer = decode_mappings(outer.mappings.as_ref());
    let decoded_inner = decode_mappings(inner.mappings.as_ref());

    let mut names = inner.names.iter().cloned().collect::<Vec<_>>();
    let mut name_lookup = names
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, value)| (value, index))
        .collect::<BTreeMap<Arc<str>, usize>>();

    let mut sources = inner.sources.iter().cloned().collect::<Vec<_>>();
    let mut source_lookup = sources
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, value)| (value, index))
        .collect::<BTreeMap<Arc<str>, usize>>();

    let mut composed_lines = Vec::<Vec<DecodedSegment>>::with_capacity(decoded_outer.len());

    for (line_index, line) in decoded_outer.iter().enumerate() {
        let mut composed_line = Vec::with_capacity(line.len());
        for segment in line {
            let traced = trace_original_position(
                inner,
                &decoded_inner,
                segment.original_line,
                segment.original_column,
            );

            let Some(traced) = traced else {
                continue;
            };

            let source = inner.sources.get(traced.source_index).cloned();
            let Some(source) = source else {
                continue;
            };
            let source_index = intern_arc(&mut sources, &mut source_lookup, source);

            let name_index = traced
                .name_index
                .and_then(|index| inner.names.get(index).cloned())
                .map(|name| intern_arc(&mut names, &mut name_lookup, name));

            composed_line.push(DecodedSegment {
                generated_column: segment.generated_column,
                source_index,
                original_line: traced.original_line,
                original_column: traced.original_column,
                name_index,
            });
        }
        if line_index < decoded_outer.len() {
            composed_lines.push(composed_line);
        }
    }

    SourceMap {
        version: 3,
        file: outer.file.clone(),
        source_root: None,
        sources: sources.into_boxed_slice(),
        sources_content: None,
        names: names.into_boxed_slice(),
        mappings: encode_decoded_mappings(&composed_lines),
    }
}

#[allow(dead_code)]
pub(crate) fn merge_sourcemaps(base: &SourceMap, overlay: &SourceMap) -> SourceMap {
    let base_decoded = decode_mappings(base.mappings.as_ref());
    let overlay_decoded = decode_mappings(overlay.mappings.as_ref());

    let mut sources = base.sources.iter().cloned().collect::<Vec<_>>();
    let mut source_lookup = sources
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, value)| (value, index))
        .collect::<BTreeMap<Arc<str>, usize>>();

    let mut names = base.names.iter().cloned().collect::<Vec<_>>();
    let mut name_lookup = names
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, value)| (value, index))
        .collect::<BTreeMap<Arc<str>, usize>>();

    let max_len = base_decoded.len().max(overlay_decoded.len());
    let mut merged = Vec::with_capacity(max_len);

    for line_index in 0..max_len {
        let mut line = BTreeMap::<usize, DecodedSegment>::new();

        if let Some(base_line) = base_decoded.get(line_index) {
            for segment in base_line {
                let source = base.sources.get(segment.source_index).cloned();
                let Some(source) = source else {
                    continue;
                };
                let source_index = intern_arc(&mut sources, &mut source_lookup, source);
                let name_index = segment
                    .name_index
                    .and_then(|index| base.names.get(index).cloned())
                    .map(|name| intern_arc(&mut names, &mut name_lookup, name));
                line.insert(
                    segment.generated_column,
                    DecodedSegment {
                        generated_column: segment.generated_column,
                        source_index,
                        original_line: segment.original_line,
                        original_column: segment.original_column,
                        name_index,
                    },
                );
            }
        }

        if let Some(overlay_line) = overlay_decoded.get(line_index) {
            for segment in overlay_line {
                let source = overlay.sources.get(segment.source_index).cloned();
                let Some(source) = source else {
                    continue;
                };
                let source_index = intern_arc(&mut sources, &mut source_lookup, source);
                let name_index = segment
                    .name_index
                    .and_then(|index| overlay.names.get(index).cloned())
                    .map(|name| intern_arc(&mut names, &mut name_lookup, name));
                line.insert(
                    segment.generated_column,
                    DecodedSegment {
                        generated_column: segment.generated_column,
                        source_index,
                        original_line: segment.original_line,
                        original_column: segment.original_column,
                        name_index,
                    },
                );
            }
        }

        merged.push(line.into_values().collect());
    }

    SourceMap {
        version: 3,
        file: overlay.file.clone().or_else(|| base.file.clone()),
        source_root: None,
        sources: sources.into_boxed_slice(),
        sources_content: None,
        names: names.into_boxed_slice(),
        mappings: encode_decoded_mappings(&merged),
    }
}

fn add_sparse_identity_matches(
    output: &str,
    source: &str,
    source_index: usize,
    segments_by_generated: &mut BTreeMap<usize, GeneratedSegment>,
    names: &mut Vec<Arc<str>>,
    name_index_by_value: &mut BTreeMap<Arc<str>, usize>,
) {
    let candidates = extract_sparse_candidates(source);
    for candidate in candidates {
        add_sparse_pair_matches(
            output,
            source,
            source_index,
            &candidate,
            &candidate,
            None,
            segments_by_generated,
            names,
            name_index_by_value,
        );
    }
}

fn add_sparse_hint_matches(
    output: &str,
    source: &str,
    source_index: usize,
    hint: &SparseMappingHint<'_>,
    segments_by_generated: &mut BTreeMap<usize, GeneratedSegment>,
    names: &mut Vec<Arc<str>>,
    name_index_by_value: &mut BTreeMap<Arc<str>, usize>,
) {
    add_sparse_pair_matches(
        output,
        source,
        source_index,
        hint.original,
        hint.generated,
        hint.name,
        segments_by_generated,
        names,
        name_index_by_value,
    );
}

#[allow(clippy::too_many_arguments)]
fn add_sparse_pair_matches(
    output: &str,
    source: &str,
    source_index: usize,
    original: &str,
    generated: &str,
    name: Option<&str>,
    segments_by_generated: &mut BTreeMap<usize, GeneratedSegment>,
    names: &mut Vec<Arc<str>>,
    name_index_by_value: &mut BTreeMap<Arc<str>, usize>,
) {
    if original.is_empty() || generated.is_empty() {
        return;
    }

    let original_offsets = find_all_occurrences(source, original);
    let generated_offsets = find_all_occurrences(output, generated);
    let shared = original_offsets.len().min(generated_offsets.len());
    if shared == 0 {
        return;
    }

    let priority = generated.len().max(original.len());
    let name_index = name.map(|value| intern_name(value, names, name_index_by_value));

    for index in 0..shared {
        let original_offset = original_offsets[index];
        let generated_offset = generated_offsets[index];
        register_segment(
            segments_by_generated,
            GeneratedSegment {
                generated_offset,
                source_index,
                original_offset,
                name_index,
                priority,
            },
        );
        register_segment(
            segments_by_generated,
            GeneratedSegment {
                generated_offset: generated_offset + generated.len(),
                source_index,
                original_offset: original_offset + original.len(),
                name_index,
                priority,
            },
        );
    }
}

fn register_segment(
    segments_by_generated: &mut BTreeMap<usize, GeneratedSegment>,
    candidate: GeneratedSegment,
) {
    match segments_by_generated.get(&candidate.generated_offset) {
        Some(existing) if existing.priority > candidate.priority => {}
        _ => {
            segments_by_generated.insert(candidate.generated_offset, candidate);
        }
    }
}

fn extract_sparse_candidates(source: &str) -> Vec<String> {
    let mut values = BTreeSet::<String>::new();

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.len() >= 2 {
            values.insert(trimmed.to_string());
            if let Some(without_semicolon) = trimmed.strip_suffix(';')
                && without_semicolon.len() >= 2
            {
                values.insert(without_semicolon.trim_end().to_string());
            }
        }
    }

    let bytes = source.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let ch = bytes[cursor] as char;
        if is_ident_start(ch) {
            let start = cursor;
            let mut segment_ranges = Vec::new();
            cursor += 1;
            while cursor < bytes.len() && is_ident_continue(bytes[cursor] as char) {
                cursor += 1;
            }
            segment_ranges.push((start, cursor));
            while cursor < bytes.len() && bytes[cursor] == b'.' {
                let next = cursor + 1;
                if next >= bytes.len() || !is_ident_start(bytes[next] as char) {
                    break;
                }
                cursor = next + 1;
                while cursor < bytes.len() && is_ident_continue(bytes[cursor] as char) {
                    cursor += 1;
                }
                segment_ranges.push((next, cursor));
            }
            for start_index in 0..segment_ranges.len() {
                for end_index in start_index..segment_ranges.len() {
                    let segment_start = segment_ranges[start_index].0;
                    let segment_end = segment_ranges[end_index].1;
                    if let Some(candidate) = source.get(segment_start..segment_end) {
                        values.insert(candidate.to_string());
                    }
                }
            }
            continue;
        }

        if ch == '\'' || ch == '"' {
            let quote = ch;
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len() {
                let current = bytes[cursor] as char;
                if current == '\\' {
                    cursor += 2;
                    continue;
                }
                cursor += 1;
                if current == quote {
                    break;
                }
            }
            if let Some(candidate) = source.get(start..cursor) {
                values.insert(candidate.to_string());
            }
            continue;
        }

        if ch == '.' && cursor + 1 < bytes.len() && is_css_ident_start(bytes[cursor + 1] as char) {
            let start = cursor;
            cursor += 2;
            while cursor < bytes.len() && is_css_ident_continue(bytes[cursor] as char) {
                cursor += 1;
            }
            if let Some(candidate) = source.get(start..cursor) {
                values.insert(candidate.to_string());
            }
            continue;
        }

        if ch == '-'
            && cursor + 2 < bytes.len()
            && bytes[cursor + 1] == b'-'
            && is_css_ident_start(bytes[cursor + 2] as char)
        {
            let start = cursor;
            cursor += 3;
            while cursor < bytes.len() && is_css_ident_continue(bytes[cursor] as char) {
                cursor += 1;
            }
            if let Some(candidate) = source.get(start..cursor) {
                values.insert(candidate.to_string());
            }
            continue;
        }

        if ch.is_ascii_digit() {
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len() && (bytes[cursor] as char).is_ascii_digit() {
                cursor += 1;
            }
            if let Some(candidate) = source.get(start..cursor) {
                values.insert(candidate.to_string());
            }
            continue;
        }

        cursor += 1;
    }

    values.into_iter().collect()
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()
}

fn is_css_ident_start(ch: char) -> bool {
    ch == '_' || ch == '-' || ch.is_ascii_alphabetic()
}

fn is_css_ident_continue(ch: char) -> bool {
    ch == '_' || ch == '-' || ch.is_ascii_alphanumeric()
}

fn find_all_occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut start = 0usize;
    while let Some(relative) = haystack.get(start..).and_then(|tail| tail.find(needle)) {
        let absolute = start + relative;
        out.push(absolute);
        start = absolute + needle.len();
    }
    out
}

fn intern_name(
    value: &str,
    names: &mut Vec<Arc<str>>,
    name_index_by_value: &mut BTreeMap<Arc<str>, usize>,
) -> usize {
    let key: Arc<str> = Arc::from(value);
    if let Some(index) = name_index_by_value.get(&key).copied() {
        return index;
    }
    let index = names.len();
    names.push(key.clone());
    name_index_by_value.insert(key, index);
    index
}

fn encode_segments(
    mut segments: Vec<GeneratedSegment>,
    output_lines: &LineIndex,
    source_lines: &[LineIndex],
) -> String {
    segments.sort_by(|left, right| {
        left.generated_offset
            .cmp(&right.generated_offset)
            .then_with(|| left.priority.cmp(&right.priority))
    });

    let mut by_line = BTreeMap::<usize, Vec<(usize, GeneratedSegment)>>::new();
    for segment in segments {
        let (line, column) = output_lines.line_col(segment.generated_offset);
        by_line.entry(line).or_default().push((column, segment));
    }

    let max_line = by_line.keys().copied().max().unwrap_or(0);
    let mut encoded = String::new();
    let mut previous_source = 0i64;
    let mut previous_original_line = 0i64;
    let mut previous_original_column = 0i64;
    let mut previous_name = 0i64;

    for line in 0..=max_line {
        if line > 0 {
            encoded.push(';');
        }

        let Some(entries) = by_line.get_mut(&line) else {
            continue;
        };
        entries.sort_by_key(|(column, _)| *column);

        let mut previous_generated_column = 0i64;
        for (index, (column, segment)) in entries.iter().enumerate() {
            if index > 0 {
                encoded.push(',');
            }

            let (original_line, original_column) =
                source_lines[segment.source_index].line_col(segment.original_offset);

            encode_vlq_value(*column as i64 - previous_generated_column, &mut encoded);
            previous_generated_column = *column as i64;

            encode_vlq_value(segment.source_index as i64 - previous_source, &mut encoded);
            previous_source = segment.source_index as i64;

            encode_vlq_value(original_line as i64 - previous_original_line, &mut encoded);
            previous_original_line = original_line as i64;

            encode_vlq_value(
                original_column as i64 - previous_original_column,
                &mut encoded,
            );
            previous_original_column = original_column as i64;

            if let Some(name_index) = segment.name_index {
                encode_vlq_value(name_index as i64 - previous_name, &mut encoded);
                previous_name = name_index as i64;
            }
        }
    }

    encoded
}

fn decode_vlq_value(input: &str, cursor: &mut usize) -> i64 {
    let bytes = input.as_bytes();
    let mut shift = 0u32;
    let mut value = 0u64;

    while let Some(&byte) = bytes.get(*cursor) {
        *cursor += 1;
        let digit = BASE64_VLQ
            .iter()
            .position(|candidate| *candidate == byte)
            .expect("invalid base64-vlq digit") as u64;
        let continuation = (digit & 0b10_0000) != 0;
        value |= (digit & 0b1_1111) << shift;
        shift += 5;
        if !continuation {
            break;
        }
    }

    from_vlq_signed(value as i64)
}

fn encode_vlq_value(value: i64, out: &mut String) {
    let mut current = to_vlq_signed(value) as u64;
    loop {
        let mut digit = (current & 0b1_1111) as usize;
        current >>= 5;
        if current != 0 {
            digit |= 0b10_0000;
        }
        out.push(BASE64_VLQ[digit] as char);
        if current == 0 {
            break;
        }
    }
}

fn to_vlq_signed(value: i64) -> i64 {
    if value < 0 {
        ((-value) << 1) + 1
    } else {
        value << 1
    }
}

fn from_vlq_signed(value: i64) -> i64 {
    let negative = (value & 1) == 1;
    let shifted = value >> 1;
    if negative { -shifted } else { shifted }
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

fn relativize_source_path(output_filename: Option<&Utf8Path>, source_filename: &str) -> Arc<str> {
    let source_path = Utf8Path::new(source_filename);
    let Some(output_filename) = output_filename else {
        return Arc::from(
            source_path
                .file_name()
                .unwrap_or(source_filename)
                .replace('\\', "/"),
        );
    };
    let output_dir = output_filename
        .parent()
        .unwrap_or_else(|| Utf8Path::new(""));
    Arc::from(relative_path(output_dir, source_path).replace('\\', "/"))
}

fn relative_path(from_dir: &Utf8Path, to_path: &Utf8Path) -> String {
    let from = normalize_components(from_dir);
    let to = normalize_components(to_path);

    let shared = from
        .iter()
        .zip(to.iter())
        .take_while(|(left, right)| left == right)
        .count();

    let mut out = Utf8PathBuf::new();
    for _ in shared..from.len() {
        out.push("..");
    }
    for component in &to[shared..] {
        out.push(component);
    }

    if out.as_str().is_empty() {
        String::from(".")
    } else {
        out.into_string()
    }
}

fn normalize_components(path: &Utf8Path) -> Vec<String> {
    let mut out = Vec::new();
    for component in path.components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => {
                out.pop();
            }
            Utf8Component::Normal(value) => out.push(value.to_string()),
            Utf8Component::RootDir | Utf8Component::Prefix(_) => {
                out.clear();
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        for (index, ch) in text.char_indices() {
            if ch == '\n' {
                starts.push(index + 1);
            }
        }
        Self { starts }
    }

    fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = match self.starts.binary_search(&offset) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        let column = offset.saturating_sub(self.starts[line]);
        (line, column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_map_encodes_identity_segments() {
        let map = build_sparse_sourcemap(SparseMappingOptions {
            output: "console.log(answer);",
            output_filename: Some(Utf8Path::new("out.js")),
            sources: vec![SourceMapSource {
                filename: Arc::from("input.svelte"),
                code: "console.log(answer);",
            }],
            hints: vec![],
        });

        assert_eq!(map.sources.as_ref(), &[Arc::from("input.svelte")]);
        assert!(!map.mappings.is_empty());
    }

    #[test]
    fn sparse_map_relativizes_sources_against_output_file() {
        let map = build_sparse_sourcemap(SparseMappingOptions {
            output: "answer",
            output_filename: Some(Utf8Path::new("_output/client/input.svelte.js")),
            sources: vec![SourceMapSource {
                filename: Arc::from("input.svelte"),
                code: "answer",
            }],
            hints: vec![],
        });

        assert_eq!(map.sources.as_ref(), &[Arc::from("../../input.svelte")]);
    }
}
