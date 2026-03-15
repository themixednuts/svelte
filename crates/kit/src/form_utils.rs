use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::{FormError, Result};

pub const BINARY_FORM_CONTENT_TYPE: &str = "application/x-sveltekit-formdata";
const BINARY_FORM_VERSION: u8 = 0;
const BINARY_FORM_HEADER_BYTES: usize = 1 + 4 + 2;

#[derive(Debug, Clone, PartialEq)]
pub struct FormFile {
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub last_modified: u64,
}

impl FormFile {
    pub fn new(name: impl Into<String>, content_type: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self::new_with_last_modified(name, content_type, bytes, 0)
    }

    pub fn new_with_last_modified(
        name: impl Into<String>,
        content_type: impl Into<String>,
        bytes: Vec<u8>,
        last_modified: u64,
    ) -> Self {
        Self {
            name: name.into(),
            content_type: content_type.into(),
            bytes,
            last_modified,
        }
    }

    pub fn size(&self) -> usize {
        self.bytes.len()
    }

    pub fn text(&self) -> Result<String> {
        String::from_utf8(self.bytes.clone()).map_err(|error| {
            FormError::InvalidUtf8FileText {
                message: error.to_string(),
            }
            .into()
        })
    }

    pub fn array_buffer(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    pub fn slice(
        &self,
        start: Option<isize>,
        end: Option<isize>,
        content_type: Option<&str>,
    ) -> Self {
        let size = self.size() as isize;
        let start = match start.unwrap_or(0) {
            value if value < 0 => (size + value).max(0),
            value => value.min(size),
        };
        let end = match end.unwrap_or(size) {
            value if value < 0 => (size + value).max(0),
            value => value.min(size),
        };
        let len = (end - start).max(0) as usize;
        let start = start as usize;
        let end = start + len;

        Self {
            name: self.name.clone(),
            content_type: content_type.unwrap_or(&self.content_type).to_string(),
            bytes: self.bytes[start..end].to_vec(),
            last_modified: self.last_modified,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FormInputValue {
    Text(String),
    File(FormFile),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FormData {
    entries: Vec<(String, FormInputValue)>,
}

impl FormData {
    pub fn append_text(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.entries
            .push((key.into(), FormInputValue::Text(value.into())));
    }

    pub fn append_file(&mut self, key: impl Into<String>, file: FormFile) {
        self.entries.push((key.into(), FormInputValue::File(file)));
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(key, _)| key.as_str())
    }

    pub fn get_all(&self, key: &str) -> Vec<FormInputValue> {
        self.entries
            .iter()
            .filter(|(entry_key, _)| entry_key == key)
            .map(|(_, value)| value.clone())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FormObject(pub BTreeMap<String, FormValue>);

#[derive(Debug, Clone, PartialEq)]
pub enum FormValue {
    String(String),
    Number(f64),
    Bool(bool),
    File(FormFile),
    Array(Vec<FormValue>),
    Object(BTreeMap<String, FormValue>),
    Undefined,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SerializedBinaryForm {
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BinaryFormRequest {
    pub content_type: Option<String>,
    pub content_length: Option<String>,
    pub chunks: Vec<Vec<u8>>,
    pub form_data: Option<FormData>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeserializedBinaryForm {
    pub data: FormObject,
    pub meta: Value,
    pub form_data: Option<FormData>,
}

pub fn split_path(path: &str) -> Result<Vec<String>> {
    static PATH_REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = PATH_REGEX.get_or_init(|| {
        Regex::new(r"^[a-zA-Z_$]\w*(\.[a-zA-Z_$]\w*|\[\d+\])*$").expect("valid split_path regex")
    });

    if !regex.is_match(path) {
        return Err(FormError::InvalidPath {
            path: path.to_string(),
        }
        .into());
    }

    Ok(path
        .split(['.', '[', ']'])
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn deep_set(object: &mut FormObject, keys: &[String], value: FormValue) -> Result<()> {
    if keys.is_empty() {
        return Ok(());
    }
    set_in_object(&mut object.0, keys, value)
}

pub fn set_nested_value(
    object: &mut FormObject,
    path_string: &str,
    value: FormInputValue,
) -> Result<()> {
    let (path_string, value) = if let Some(path) = path_string.strip_prefix("n:") {
        let converted = match value {
            FormInputValue::Text(text) if text.is_empty() => FormValue::Undefined,
            FormInputValue::Text(text) => FormValue::Number(
                text.parse::<f64>()
                    .map_err(|_| FormError::InvalidNumber { text: text.clone() })?,
            ),
            FormInputValue::File(_) => {
                return Err(FormError::NumericFieldCannotContainFiles.into());
            }
        };
        (path, converted)
    } else if let Some(path) = path_string.strip_prefix("b:") {
        let converted = match value {
            FormInputValue::Text(text) => FormValue::Bool(text == "on"),
            FormInputValue::File(_) => {
                return Err(FormError::BooleanFieldCannotContainFiles.into());
            }
        };
        (path, converted)
    } else {
        (path_string, input_to_value(value))
    };

    deep_set(object, &split_path(path_string)?, value)
}

pub fn convert_formdata(data: &FormData) -> Result<FormObject> {
    let mut result = FormObject::default();
    let mut seen = BTreeSet::new();

    for key in data.keys() {
        if !seen.insert(key.to_string()) {
            continue;
        }

        let is_array = key.ends_with("[]");
        let mut normalized_key = if is_array {
            key[..key.len() - 2].to_string()
        } else {
            key.to_string()
        };

        let mut values = data
            .get_all(key)
            .into_iter()
            .filter(|entry| matches!(entry, FormInputValue::Text(_)) || !is_empty_file(entry))
            .collect::<Vec<_>>();

        if values.len() > 1 && !is_array {
            return Err(FormError::DuplicatedKey {
                key: normalized_key,
                count: values.len(),
            }
            .into());
        }

        if let Some(stripped) = normalized_key.strip_prefix("n:") {
            normalized_key = stripped.to_string();
            let converted = values
                .drain(..)
                .map(|value| match value {
                    FormInputValue::Text(text) if text.is_empty() => Ok(FormValue::Undefined),
                    FormInputValue::Text(text) => text
                        .parse::<f64>()
                        .map(FormValue::Number)
                        .map_err(|_| FormError::InvalidNumber { text: text.clone() }.into()),
                    FormInputValue::File(_) => {
                        Err(FormError::NumericFieldCannotContainFiles.into())
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            let value = if is_array {
                FormValue::Array(converted)
            } else {
                converted.into_iter().next().unwrap_or(FormValue::Undefined)
            };
            deep_set(&mut result, &split_path(&normalized_key)?, value)?;
            continue;
        }

        if let Some(stripped) = normalized_key.strip_prefix("b:") {
            normalized_key = stripped.to_string();
            let converted = values
                .drain(..)
                .map(|value| match value {
                    FormInputValue::Text(text) => Ok(FormValue::Bool(text == "on")),
                    FormInputValue::File(_) => {
                        Err(FormError::BooleanFieldCannotContainFiles.into())
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            let value = if is_array {
                FormValue::Array(converted)
            } else {
                converted
                    .into_iter()
                    .next()
                    .unwrap_or(FormValue::Bool(false))
            };
            deep_set(&mut result, &split_path(&normalized_key)?, value)?;
            continue;
        }

        let converted = values.into_iter().map(input_to_value).collect::<Vec<_>>();
        let value = if is_array {
            FormValue::Array(converted)
        } else {
            converted.into_iter().next().unwrap_or(FormValue::Undefined)
        };
        deep_set(&mut result, &split_path(&normalized_key)?, value)?;
    }

    Ok(result)
}

pub fn serialize_binary_form(data: &FormObject, meta: Value) -> Result<SerializedBinaryForm> {
    let mut files = Vec::<(FormFile, usize)>::new();
    let serialized_data = serialize_form_object(data, &mut files);
    let mut meta = meta;
    if let Value::Object(map) = &mut meta {
        if matches!(map.get("remote_refreshes"), Some(Value::Array(values)) if values.is_empty()) {
            map.remove("remote_refreshes");
        }
    }

    let header = serde_json::to_vec(&json!([serialized_data, meta])).map_err(|error| {
        FormError::Serialization {
            message: error.to_string(),
        }
    })?;

    let mut sorted_files = files;
    sorted_files.sort_by_key(|(file, _)| file.size());
    let mut offsets = vec![0usize; sorted_files.len()];
    let mut start = 0usize;
    for (file, original_index) in &sorted_files {
        offsets[*original_index] = start;
        start += file.size();
    }

    let offsets_json = if sorted_files.is_empty() {
        Vec::new()
    } else {
        serde_json::to_vec(&offsets).map_err(|error| FormError::Serialization {
            message: error.to_string(),
        })?
    };

    let mut bytes = Vec::with_capacity(
        BINARY_FORM_HEADER_BYTES
            + header.len()
            + offsets_json.len()
            + sorted_files
                .iter()
                .map(|(file, _)| file.size())
                .sum::<usize>(),
    );
    bytes.push(BINARY_FORM_VERSION);
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(offsets_json.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&offsets_json);
    for (file, _) in sorted_files {
        bytes.extend_from_slice(&file.bytes);
    }

    Ok(SerializedBinaryForm { bytes })
}

pub fn deserialize_binary_form(request: &BinaryFormRequest) -> Result<DeserializedBinaryForm> {
    if request.content_type.as_deref() != Some(BINARY_FORM_CONTENT_TYPE) {
        let form_data = request
            .form_data
            .clone()
            .ok_or(FormError::MissingFormData)?;
        return Ok(DeserializedBinaryForm {
            data: convert_formdata(&form_data)?,
            meta: Value::Object(Map::new()),
            form_data: Some(form_data),
        });
    }

    let content_length = request
        .content_length
        .as_deref()
        .ok_or_else(|| deserialize_error("invalid Content-Length header"))?
        .parse::<usize>()
        .map_err(|_| deserialize_error("invalid Content-Length header"))?;

    let body = request.chunks.concat();
    if body.len() < BINARY_FORM_HEADER_BYTES {
        return Err(deserialize_error("too short"));
    }

    if body[0] != BINARY_FORM_VERSION {
        return Err(deserialize_error(format!(
            "got version {}, expected version {}",
            body[0], BINARY_FORM_VERSION
        )));
    }

    let header_length = u32::from_le_bytes(body[1..5].try_into().expect("header bytes")) as usize;
    if BINARY_FORM_HEADER_BYTES + header_length > content_length {
        return Err(deserialize_error("data overflow"));
    }

    let offsets_length = u16::from_le_bytes(body[5..7].try_into().expect("offset bytes")) as usize;
    if BINARY_FORM_HEADER_BYTES + header_length + offsets_length > content_length {
        return Err(deserialize_error("file offset table overflow"));
    }

    if body.len() < BINARY_FORM_HEADER_BYTES + header_length {
        return Err(deserialize_error("data too short"));
    }
    let header_slice = &body[BINARY_FORM_HEADER_BYTES..BINARY_FORM_HEADER_BYTES + header_length];

    let offsets_start = BINARY_FORM_HEADER_BYTES + header_length;
    if body.len() < offsets_start + offsets_length {
        return Err(deserialize_error("file offset table too short"));
    }
    let offsets_slice = &body[offsets_start..offsets_start + offsets_length];

    let mut offsets = if offsets_length == 0 {
        Vec::new()
    } else {
        let parsed: Value = serde_json::from_slice(offsets_slice)
            .map_err(|_| deserialize_error("invalid file offset table"))?;
        parse_offsets(parsed)?
    };

    let files_start = offsets_start + offsets_length;
    let header_value: Value = serde_json::from_slice(header_slice)
        .map_err(|error| deserialize_error(format!("invalid data payload: {error}")))?;
    let pair = header_value
        .as_array()
        .filter(|values| values.len() == 2)
        .ok_or_else(|| deserialize_error("invalid data payload"))?;

    let mut file_spans = Vec::new();
    let data = deserialize_form_object(
        &pair[0],
        &mut offsets,
        &body,
        files_start,
        content_length,
        &mut file_spans,
    )?;
    let meta = pair[1].clone();

    file_spans.sort_by_key(|span: &FileSpan| (span.offset, span.size));
    for window in file_spans.windows(2) {
        let previous = &window[0];
        let current = &window[1];
        let previous_end = previous.offset + previous.size;
        if previous_end < current.offset {
            return Err(deserialize_error("gaps in file data"));
        }
        if previous_end > current.offset {
            return Err(deserialize_error("overlapping file data"));
        }
    }

    Ok(DeserializedBinaryForm {
        data,
        meta,
        form_data: None,
    })
}

fn input_to_value(value: FormInputValue) -> FormValue {
    match value {
        FormInputValue::Text(text) => FormValue::String(text),
        FormInputValue::File(file) => FormValue::File(file),
    }
}

fn is_empty_file(value: &FormInputValue) -> bool {
    matches!(value, FormInputValue::File(file) if file.name.is_empty() && file.size() == 0)
}

fn set_in_object(
    map: &mut BTreeMap<String, FormValue>,
    keys: &[String],
    value: FormValue,
) -> Result<()> {
    if keys.len() == 1 {
        check_prototype_pollution(&keys[0])?;
        map.insert(keys[0].clone(), value);
        return Ok(());
    }

    let key = &keys[0];
    check_prototype_pollution(key)?;
    let next_is_array = keys[1].chars().all(|ch| ch.is_ascii_digit());

    let entry = map.entry(key.clone()).or_insert_with(|| {
        if next_is_array {
            FormValue::Array(Vec::new())
        } else {
            FormValue::Object(BTreeMap::new())
        }
    });

    match entry {
        FormValue::Object(inner) if !next_is_array => set_in_object(inner, &keys[1..], value),
        FormValue::Array(inner) if next_is_array => set_in_array(inner, &keys[1..], value),
        _ => Err(FormError::InvalidArrayKey {
            key: keys[1].clone(),
        }
        .into()),
    }
}

fn set_in_array(items: &mut Vec<FormValue>, keys: &[String], value: FormValue) -> Result<()> {
    let index = keys[0]
        .parse::<usize>()
        .map_err(|_| FormError::InvalidPathSegment {
            segment: keys[0].clone(),
        })?;

    if keys.len() == 1 {
        resize_with_undefined(items, index + 1);
        items[index] = value;
        return Ok(());
    }

    let next_is_array = keys[1].chars().all(|ch| ch.is_ascii_digit());
    resize_with_undefined(items, index + 1);

    if matches!(items[index], FormValue::Undefined) {
        items[index] = if next_is_array {
            FormValue::Array(Vec::new())
        } else {
            FormValue::Object(BTreeMap::new())
        };
    }

    match &mut items[index] {
        FormValue::Object(inner) if !next_is_array => set_in_object(inner, &keys[1..], value),
        FormValue::Array(inner) if next_is_array => set_in_array(inner, &keys[1..], value),
        _ => Err(FormError::InvalidArrayKey {
            key: keys[1].clone(),
        }
        .into()),
    }
}

fn resize_with_undefined(items: &mut Vec<FormValue>, len: usize) {
    if items.len() < len {
        items.resize(len, FormValue::Undefined);
    }
}

fn check_prototype_pollution(key: &str) -> Result<()> {
    if matches!(key, "__proto__" | "constructor" | "prototype") {
        return Err(FormError::PrototypePollutionKey {
            key: key.to_string(),
        }
        .into());
    }
    Ok(())
}

fn serialize_form_object(data: &FormObject, files: &mut Vec<(FormFile, usize)>) -> Value {
    Value::Object(
        data.0
            .iter()
            .map(|(key, value)| (key.clone(), serialize_form_value(value, files)))
            .collect(),
    )
}

fn serialize_form_value(value: &FormValue, files: &mut Vec<(FormFile, usize)>) -> Value {
    match value {
        FormValue::String(value) => Value::String(value.clone()),
        FormValue::Number(value) => serde_json::Number::from_f64(*value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        FormValue::Bool(value) => Value::Bool(*value),
        FormValue::File(file) => {
            let index = files.len();
            files.push((file.clone(), index));
            json!({
                "$file": [file.name, file.content_type, file.size(), file.last_modified, index]
            })
        }
        FormValue::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| serialize_form_value(value, files))
                .collect(),
        ),
        FormValue::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), serialize_form_value(value, files)))
                .collect(),
        ),
        FormValue::Undefined => Value::Null,
    }
}

fn deserialize_form_object(
    value: &Value,
    offsets: &mut [usize],
    body: &[u8],
    files_start: usize,
    content_length: usize,
    file_spans: &mut Vec<FileSpan>,
) -> Result<FormObject> {
    let object = value
        .as_object()
        .ok_or_else(|| deserialize_error("invalid data payload"))?;
    let mut result = BTreeMap::new();
    for (key, value) in object {
        result.insert(
            key.clone(),
            deserialize_form_value(
                value,
                offsets,
                body,
                files_start,
                content_length,
                file_spans,
            )?,
        );
    }
    Ok(FormObject(result))
}

fn deserialize_form_value(
    value: &Value,
    offsets: &mut [usize],
    body: &[u8],
    files_start: usize,
    content_length: usize,
    file_spans: &mut Vec<FileSpan>,
) -> Result<FormValue> {
    match value {
        Value::Null => Ok(FormValue::Undefined),
        Value::Bool(value) => Ok(FormValue::Bool(*value)),
        Value::Number(value) => {
            Ok(FormValue::Number(value.as_f64().ok_or_else(|| {
                deserialize_error("invalid numeric value")
            })?))
        }
        Value::String(value) => Ok(FormValue::String(value.clone())),
        Value::Array(values) => Ok(FormValue::Array(
            values
                .iter()
                .map(|value| {
                    deserialize_form_value(
                        value,
                        offsets,
                        body,
                        files_start,
                        content_length,
                        file_spans,
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        Value::Object(object) => {
            if let Some(file_value) = object.get("$file") {
                return deserialize_file(
                    file_value,
                    offsets,
                    body,
                    files_start,
                    content_length,
                    file_spans,
                )
                .map(FormValue::File);
            }

            let mut result = BTreeMap::new();
            for (key, value) in object {
                result.insert(
                    key.clone(),
                    deserialize_form_value(
                        value,
                        offsets,
                        body,
                        files_start,
                        content_length,
                        file_spans,
                    )?,
                );
            }
            Ok(FormValue::Object(result))
        }
    }
}

fn deserialize_file(
    value: &Value,
    offsets: &mut [usize],
    body: &[u8],
    files_start: usize,
    content_length: usize,
    file_spans: &mut Vec<FileSpan>,
) -> Result<FormFile> {
    let values = value
        .as_array()
        .filter(|values| values.len() == 5)
        .ok_or_else(|| deserialize_error("invalid file metadata"))?;

    let name = values[0]
        .as_str()
        .ok_or_else(|| deserialize_error("invalid file metadata"))?;
    let content_type = values[1]
        .as_str()
        .ok_or_else(|| deserialize_error("invalid file metadata"))?;
    let size = values[2]
        .as_u64()
        .ok_or_else(|| deserialize_error("invalid file metadata"))? as usize;
    let last_modified = values[3]
        .as_u64()
        .ok_or_else(|| deserialize_error("invalid file metadata"))?;
    let index = values[4]
        .as_u64()
        .ok_or_else(|| deserialize_error("invalid file metadata"))? as usize;

    let offset = offsets
        .get_mut(index)
        .ok_or_else(|| deserialize_error("invalid file offset table"))?;
    let file_offset = *offset;
    *offset = usize::MAX;
    if file_offset == usize::MAX {
        return Err(deserialize_error("duplicate file offset table index"));
    }

    let start = files_start + file_offset;
    let end = start + size;
    if end > content_length {
        return Err(deserialize_error("file data overflow"));
    }
    if end > body.len() {
        return Err(deserialize_error("file data too short"));
    }
    file_spans.push(FileSpan {
        offset: file_offset,
        size,
    });

    Ok(FormFile::new_with_last_modified(
        name,
        content_type,
        body[start..end].to_vec(),
        last_modified,
    ))
}

fn parse_offsets(value: Value) -> Result<Vec<usize>> {
    let values = value
        .as_array()
        .ok_or_else(|| deserialize_error("invalid file offset table"))?;
    values
        .iter()
        .map(|value| {
            value
                .as_u64()
                .map(|value| value as usize)
                .ok_or_else(|| deserialize_error("invalid file offset table"))
        })
        .collect()
}

fn deserialize_error(message: impl Into<String>) -> crate::Error {
    FormError::BinaryDeserialize {
        message: message.into(),
    }
    .into()
}

#[derive(Debug, Clone, Copy)]
struct FileSpan {
    offset: usize,
    size: usize,
}
