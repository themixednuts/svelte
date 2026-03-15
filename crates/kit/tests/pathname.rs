use svelte_kit::{
    add_data_suffix, add_resolution_suffix, has_data_suffix, has_resolution_suffix,
    strip_data_suffix, strip_resolution_suffix,
};

#[test]
fn handles_data_suffixes_like_upstream() {
    assert!(has_data_suffix("/foo/__data.json"));
    assert!(has_data_suffix("/index.html__data.json"));
    assert!(!has_data_suffix("/foo"));

    assert_eq!(add_data_suffix("/foo/"), "/foo/__data.json");
    assert_eq!(add_data_suffix("/index.html"), "/index.html__data.json");
    assert_eq!(strip_data_suffix("/foo/__data.json"), "/foo");
    assert_eq!(strip_data_suffix("/index.html__data.json"), "/index.html");
}

#[test]
fn handles_resolution_suffixes_like_upstream() {
    assert!(!has_resolution_suffix("/foo"));
    assert!(has_resolution_suffix("/foo/__route.js"));

    assert_eq!(add_resolution_suffix("/foo/"), "/foo/__route.js");
    assert_eq!(strip_resolution_suffix("/foo/__route.js"), "/foo");
}
