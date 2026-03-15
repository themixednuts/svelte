use std::collections::BTreeMap;

use svelte_kit::{
    BINARY_FORM_CONTENT_TYPE, BinaryFormRequest, FormData, FormFile, FormInputValue, FormObject,
    FormValue, convert_formdata, deep_set, deserialize_binary_form, serialize_binary_form,
    set_nested_value, split_path,
};

fn build_raw_binary_request(
    header: serde_json::Value,
    offsets_json: &str,
    file_data: &[u8],
) -> BinaryFormRequest {
    let header_bytes = serde_json::to_vec(&header).expect("header should serialize");
    let offsets_bytes = offsets_json.as_bytes().to_vec();
    let mut bytes =
        Vec::with_capacity(7 + header_bytes.len() + offsets_bytes.len() + file_data.len());
    bytes.push(0);
    bytes.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(offsets_bytes.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&header_bytes);
    bytes.extend_from_slice(&offsets_bytes);
    bytes.extend_from_slice(file_data);

    BinaryFormRequest {
        content_type: Some(BINARY_FORM_CONTENT_TYPE.to_string()),
        content_length: Some(bytes.len().to_string()),
        chunks: vec![bytes],
        form_data: None,
    }
}

#[test]
fn split_path_accepts_valid_paths() {
    assert_eq!(split_path("foo").expect("path should parse"), vec!["foo"]);
    assert_eq!(
        split_path("foo.bar.baz").expect("path should parse"),
        vec!["foo", "bar", "baz"]
    );
    assert_eq!(
        split_path("foo[0][1][2]").expect("path should parse"),
        vec!["foo", "0", "1", "2"]
    );
}

#[test]
fn split_path_rejects_invalid_paths() {
    for input in ["[0]", "foo.0", "foo[bar]"] {
        assert_eq!(
            split_path(input)
                .expect_err("path should be rejected")
                .to_string(),
            format!("Invalid path {input}")
        );
    }
}

#[test]
fn convert_formdata_builds_nested_objects_and_arrays() {
    let mut data = FormData::default();
    data.append_text("foo", "foo");
    data.append_text("object.nested.property", "property");
    data.append_text("array[]", "a");
    data.append_text("array[]", "b");
    data.append_text("array[]", "c");

    assert_eq!(
        convert_formdata(&data).expect("formdata should convert"),
        FormObject(BTreeMap::from([
            (
                "array".to_string(),
                FormValue::Array(vec![
                    FormValue::String("a".to_string()),
                    FormValue::String("b".to_string()),
                    FormValue::String("c".to_string()),
                ])
            ),
            ("foo".to_string(), FormValue::String("foo".to_string())),
            (
                "object".to_string(),
                FormValue::Object(BTreeMap::from([(
                    "nested".to_string(),
                    FormValue::Object(BTreeMap::from([(
                        "property".to_string(),
                        FormValue::String("property".to_string())
                    )]))
                )]))
            ),
        ]))
    );
}

#[test]
fn convert_formdata_handles_multiple_fields_at_same_nested_level() {
    let mut data = FormData::default();
    data.append_text("user.name.first", "first");
    data.append_text("user.name.last", "last");

    assert_eq!(
        convert_formdata(&data).expect("formdata should convert"),
        FormObject(BTreeMap::from([(
            "user".to_string(),
            FormValue::Object(BTreeMap::from([(
                "name".to_string(),
                FormValue::Object(BTreeMap::from([
                    ("first".to_string(), FormValue::String("first".to_string())),
                    ("last".to_string(), FormValue::String("last".to_string())),
                ]))
            )]))
        )]))
    );
}

#[test]
fn convert_formdata_rejects_prototype_pollution_paths() {
    for attack in [
        "__proto__.polluted",
        "constructor.polluted",
        "prototype.polluted",
        "user.__proto__.polluted",
        "user.constructor.polluted",
    ] {
        let mut data = FormData::default();
        data.append_text(attack, "bad");
        let error = convert_formdata(&data).expect_err("prototype pollution should be rejected");
        assert!(error.to_string().contains("Invalid key \""));
    }
}

#[test]
fn deep_set_creates_own_property_entries() {
    let mut target = FormObject::default();
    deep_set(
        &mut target,
        &["toString".to_string(), "property".to_string()],
        FormValue::String("hello".to_string()),
    )
    .expect("deep_set should succeed");

    assert_eq!(
        target,
        FormObject(BTreeMap::from([(
            "toString".to_string(),
            FormValue::Object(BTreeMap::from([(
                "property".to_string(),
                FormValue::String("hello".to_string())
            )]))
        )]))
    );
}

#[test]
fn set_nested_value_applies_boolean_and_numeric_prefixes() {
    let mut target = FormObject::default();
    set_nested_value(
        &mut target,
        "n:user.age",
        FormInputValue::Text("42".to_string()),
    )
    .expect("numeric field should set");
    set_nested_value(
        &mut target,
        "b:user.active",
        FormInputValue::Text("on".to_string()),
    )
    .expect("boolean field should set");
    set_nested_value(
        &mut target,
        "avatar",
        FormInputValue::File(FormFile::new("a.txt", "text/plain", b"a".to_vec())),
    )
    .expect("file field should set");

    assert_eq!(
        target,
        FormObject(BTreeMap::from([
            (
                "avatar".to_string(),
                FormValue::File(FormFile::new("a.txt", "text/plain", b"a".to_vec()))
            ),
            (
                "user".to_string(),
                FormValue::Object(BTreeMap::from([
                    ("active".to_string(), FormValue::Bool(true)),
                    ("age".to_string(), FormValue::Number(42.0)),
                ]))
            ),
        ]))
    );
}

#[test]
fn binary_form_round_trips_simple_payloads() {
    let cases = [
        (FormObject(BTreeMap::new()), serde_json::json!({})),
        (
            FormObject(BTreeMap::from([
                ("foo".to_string(), FormValue::String("foo".to_string())),
                (
                    "nested".to_string(),
                    FormValue::Object(BTreeMap::from([(
                        "prop".to_string(),
                        FormValue::String("prop".to_string()),
                    )])),
                ),
            ])),
            serde_json::json!({ "pathname": "/foo", "validate_only": true }),
        ),
    ];

    for (data, meta) in cases {
        let serialized =
            serialize_binary_form(&data, meta.clone()).expect("binary form should serialize");
        let request = BinaryFormRequest {
            content_type: Some(BINARY_FORM_CONTENT_TYPE.to_string()),
            content_length: Some(serialized.bytes.len().to_string()),
            chunks: vec![serialized.bytes],
            form_data: None,
        };
        let deserialized =
            deserialize_binary_form(&request).expect("binary form should deserialize");

        assert_eq!(deserialized.form_data, None);
        assert_eq!(deserialized.data, data);
        assert_eq!(deserialized.meta, meta);
    }
}

#[test]
fn binary_form_round_trips_files_and_file_methods() {
    let data = FormObject(BTreeMap::from([
        (
            "small".to_string(),
            FormValue::File(FormFile::new("a.txt", "text/plain", b"a".to_vec())),
        ),
        (
            "large".to_string(),
            FormValue::File(FormFile::new_with_last_modified(
                "large.txt",
                "text/plain",
                vec![b'a'; 1024],
                100,
            )),
        ),
        (
            "empty".to_string(),
            FormValue::File(FormFile::new("empty.txt", "text/plain", Vec::new())),
        ),
    ]));

    let serialized =
        serialize_binary_form(&data, serde_json::json!({})).expect("binary form should serialize");
    let one_byte_chunks = serialized
        .bytes
        .into_iter()
        .map(|byte| vec![byte])
        .collect::<Vec<_>>();
    let request = BinaryFormRequest {
        content_type: Some(BINARY_FORM_CONTENT_TYPE.to_string()),
        content_length: Some(
            one_byte_chunks
                .iter()
                .map(Vec::len)
                .sum::<usize>()
                .to_string(),
        ),
        chunks: one_byte_chunks,
        form_data: None,
    };
    let deserialized = deserialize_binary_form(&request).expect("binary form should deserialize");

    let empty = match deserialized
        .data
        .0
        .get("empty")
        .expect("empty file should exist")
    {
        FormValue::File(file) => file,
        value => panic!("expected file, got {value:?}"),
    };
    assert_eq!(empty.name, "empty.txt");
    assert_eq!(empty.content_type, "text/plain");
    assert_eq!(empty.size(), 0);
    assert_eq!(empty.text().expect("text should decode"), "");

    let small = match deserialized
        .data
        .0
        .get("small")
        .expect("small file should exist")
    {
        FormValue::File(file) => file,
        value => panic!("expected file, got {value:?}"),
    };
    assert_eq!(small.name, "a.txt");
    assert_eq!(small.content_type, "text/plain");
    assert_eq!(small.size(), 1);
    assert_eq!(small.text().expect("text should decode"), "a");

    let large = match deserialized
        .data
        .0
        .get("large")
        .expect("large file should exist")
    {
        FormValue::File(file) => file,
        value => panic!("expected file, got {value:?}"),
    };
    assert_eq!(large.name, "large.txt");
    assert_eq!(large.content_type, "text/plain");
    assert_eq!(large.size(), 1024);
    assert_eq!(large.last_modified, 100);
    assert_eq!(large.bytes(), vec![b'a'; 1024]);
    assert_eq!(large.text().expect("text should decode"), "a".repeat(1024));
    assert_eq!(large.array_buffer(), vec![b'a'; 1024]);

    let ello_slice = large.slice(Some(1), Some(5), Some("test/content-type"));
    assert_eq!(ello_slice.content_type, "test/content-type");
    assert_eq!(ello_slice.text().expect("text should decode"), "aaaa");

    let world_slice =
        FormFile::new("a.txt", "", b"Hello World".to_vec()).slice(Some(-5), None, None);
    assert_eq!(world_slice.text().expect("text should decode"), "World");
}

#[test]
fn binary_form_rejects_invalid_content_length() {
    let request = BinaryFormRequest {
        content_type: Some(BINARY_FORM_CONTENT_TYPE.to_string()),
        content_length: None,
        chunks: vec![b"foo".to_vec()],
        form_data: None,
    };
    assert_eq!(
        deserialize_binary_form(&request)
            .expect_err("missing content length should fail")
            .to_string(),
        "Could not deserialize binary form: invalid Content-Length header"
    );

    let request = BinaryFormRequest {
        content_type: Some(BINARY_FORM_CONTENT_TYPE.to_string()),
        content_length: Some("invalid".to_string()),
        chunks: vec![b"foo".to_vec()],
        form_data: None,
    };
    assert_eq!(
        deserialize_binary_form(&request)
            .expect_err("invalid content length should fail")
            .to_string(),
        "Could not deserialize binary form: invalid Content-Length header"
    );
}

#[test]
fn binary_form_rejects_header_and_file_overflows() {
    let payload = FormObject(BTreeMap::from([(
        "foo".to_string(),
        FormValue::String("bar".to_string()),
    )]));
    let serialized = serialize_binary_form(&payload, serde_json::json!({}))
        .expect("binary form should serialize");

    let request = BinaryFormRequest {
        content_type: Some(BINARY_FORM_CONTENT_TYPE.to_string()),
        content_length: Some((serialized.bytes.len() - 1).to_string()),
        chunks: vec![serialized.bytes.clone()],
        form_data: None,
    };
    assert_eq!(
        deserialize_binary_form(&request)
            .expect_err("data overflow should fail")
            .to_string(),
        "Could not deserialize binary form: data overflow"
    );

    let file_payload = FormObject(BTreeMap::from([(
        "file".to_string(),
        FormValue::File(FormFile::new("a.txt", "text/plain", b"a".to_vec())),
    )]));
    let serialized = serialize_binary_form(&file_payload, serde_json::json!({}))
        .expect("binary form should serialize");

    let request = BinaryFormRequest {
        content_type: Some(BINARY_FORM_CONTENT_TYPE.to_string()),
        content_length: Some((serialized.bytes.len() - 1).to_string()),
        chunks: vec![serialized.bytes],
        form_data: None,
    };
    let error = deserialize_binary_form(&request)
        .expect_err("file offset overflow should fail")
        .to_string();
    assert!(
        error == "Could not deserialize binary form: file offset table overflow"
            || error == "Could not deserialize binary form: file data overflow"
    );
}

#[test]
fn binary_form_rejects_invalid_file_metadata_and_offset_tables() {
    let header = serde_json::json!([
        { "file": { "$file": [123, "text/plain", 0, 0, 0] } },
        {}
    ]);
    assert_eq!(
        deserialize_binary_form(&build_raw_binary_request(header, "[0]", b"a"))
            .expect_err("invalid file metadata should fail")
            .to_string(),
        "Could not deserialize binary form: invalid file metadata"
    );

    let header = serde_json::json!([
        { "file": { "$file": ["a.txt", "text/plain", 1, 0, 0] } },
        {}
    ]);
    assert_eq!(
        deserialize_binary_form(&build_raw_binary_request(
            header.clone(),
            r#"{"0":0}"#,
            b"a"
        ))
        .expect_err("non-array offsets should fail")
        .to_string(),
        "Could not deserialize binary form: invalid file offset table"
    );
    assert_eq!(
        deserialize_binary_form(&build_raw_binary_request(header, r#"[0,"1"]"#, b"a"))
            .expect_err("string offsets should fail")
            .to_string(),
        "Could not deserialize binary form: invalid file offset table"
    );
}

#[test]
fn binary_form_rejects_duplicate_gap_and_overlap_file_ranges() {
    let duplicate_index_header = serde_json::json!([
        {
            "a": { "$file": ["a.txt", "text/plain", 1, 0, 0] },
            "b": { "$file": ["b.txt", "text/plain", 1, 0, 0] }
        },
        {}
    ]);
    assert_eq!(
        deserialize_binary_form(&build_raw_binary_request(
            duplicate_index_header,
            "[0]",
            b"A"
        ))
        .expect_err("duplicate offset index should fail")
        .to_string(),
        "Could not deserialize binary form: duplicate file offset table index"
    );

    let overlap_header = serde_json::json!([
        {
            "a": { "$file": ["a.txt", "text/plain", 3, 0, 0] },
            "b": { "$file": ["b.txt", "text/plain", 3, 0, 1] }
        },
        {}
    ]);
    assert_eq!(
        deserialize_binary_form(&build_raw_binary_request(overlap_header, "[0,1]", b"AAAA"))
            .expect_err("overlap should fail")
            .to_string(),
        "Could not deserialize binary form: overlapping file data"
    );

    let gap_header = serde_json::json!([
        {
            "a": { "$file": ["a.txt", "text/plain", 1, 0, 0] },
            "b": { "$file": ["b.txt", "text/plain", 1, 0, 1] }
        },
        {}
    ]);
    assert_eq!(
        deserialize_binary_form(&build_raw_binary_request(gap_header, "[0,3]", b"AAAA"))
            .expect_err("gap should fail")
            .to_string(),
        "Could not deserialize binary form: gaps in file data"
    );
}

#[test]
fn binary_form_accepts_zero_length_files() {
    let payload = FormObject(BTreeMap::from([
        (
            "a".to_string(),
            FormValue::File(FormFile::new("a.txt", "text/plain", Vec::new())),
        ),
        (
            "b".to_string(),
            FormValue::File(FormFile::new("b.txt", "text/plain", Vec::new())),
        ),
        (
            "c".to_string(),
            FormValue::File(FormFile::new("c.txt", "text/plain", b"x".to_vec())),
        ),
    ]));
    let serialized = serialize_binary_form(&payload, serde_json::json!({}))
        .expect("binary form should serialize");
    let request = BinaryFormRequest {
        content_type: Some(BINARY_FORM_CONTENT_TYPE.to_string()),
        content_length: Some(serialized.bytes.len().to_string()),
        chunks: vec![serialized.bytes],
        form_data: None,
    };
    let deserialized = deserialize_binary_form(&request).expect("binary form should deserialize");

    let a = match deserialized.data.0.get("a").expect("a should exist") {
        FormValue::File(file) => file,
        value => panic!("expected file, got {value:?}"),
    };
    let b = match deserialized.data.0.get("b").expect("b should exist") {
        FormValue::File(file) => file,
        value => panic!("expected file, got {value:?}"),
    };
    let c = match deserialized.data.0.get("c").expect("c should exist") {
        FormValue::File(file) => file,
        value => panic!("expected file, got {value:?}"),
    };
    assert_eq!(a.size(), 0);
    assert_eq!(b.size(), 0);
    assert_eq!(c.size(), 1);
    assert_eq!(c.text().expect("text should decode"), "x");
}

#[test]
fn binary_form_rejects_too_short_and_wrong_version_payloads() {
    let request = BinaryFormRequest {
        content_type: Some(BINARY_FORM_CONTENT_TYPE.to_string()),
        content_length: Some("3".to_string()),
        chunks: vec![vec![0, 1, 2]],
        form_data: None,
    };
    assert_eq!(
        deserialize_binary_form(&request)
            .expect_err("short body should fail")
            .to_string(),
        "Could not deserialize binary form: too short"
    );

    let mut request = build_raw_binary_request(serde_json::json!([{}, {}]), "[]", &[]);
    request.chunks[0][0] = 1;
    assert_eq!(
        deserialize_binary_form(&request)
            .expect_err("wrong version should fail")
            .to_string(),
        "Could not deserialize binary form: got version 1, expected version 0"
    );
}

#[test]
fn binary_form_rejects_truncated_data_and_file_regions() {
    let request = build_raw_binary_request(serde_json::json!([{}, {}]), "[]", &[]);
    let mut request = request;
    request.chunks[0].truncate(7);
    assert_eq!(
        deserialize_binary_form(&request)
            .expect_err("truncated data should fail")
            .to_string(),
        "Could not deserialize binary form: data too short"
    );

    let header = serde_json::json!([
        { "file": { "$file": ["a.txt", "text/plain", 2, 0, 0] } },
        {}
    ]);
    let request = build_raw_binary_request(header, "[0]", b"A");
    assert_eq!(
        deserialize_binary_form(&request)
            .expect_err("truncated file data should fail")
            .to_string(),
        "Could not deserialize binary form: file data overflow"
    );
}

#[test]
fn non_binary_form_requests_fall_back_to_formdata_conversion() {
    let mut form_data = FormData::default();
    form_data.append_text("user.name", "alice");
    form_data.append_text("tags[]", "a");
    form_data.append_text("tags[]", "b");

    let request = BinaryFormRequest {
        content_type: Some("multipart/form-data".to_string()),
        content_length: None,
        chunks: Vec::new(),
        form_data: Some(form_data.clone()),
    };
    let deserialized = deserialize_binary_form(&request).expect("formdata fallback should work");
    assert_eq!(deserialized.form_data, Some(form_data));
    assert_eq!(
        deserialized.data,
        FormObject(BTreeMap::from([
            (
                "tags".to_string(),
                FormValue::Array(vec![
                    FormValue::String("a".to_string()),
                    FormValue::String("b".to_string())
                ])
            ),
            (
                "user".to_string(),
                FormValue::Object(BTreeMap::from([(
                    "name".to_string(),
                    FormValue::String("alice".to_string())
                )]))
            )
        ]))
    );
}
