use std::collections::BTreeSet;

use svelte_kit::{NodePolyfill, available_node_polyfills, install_node_polyfills};

#[test]
fn available_node_polyfills_exposes_crypto_and_file() {
    assert_eq!(
        available_node_polyfills(),
        BTreeSet::from([NodePolyfill::Crypto, NodePolyfill::File])
    );
}

#[test]
fn install_node_polyfills_only_returns_new_entries() {
    let mut installed = BTreeSet::from([NodePolyfill::Crypto]);
    let added = install_node_polyfills(&mut installed);

    assert_eq!(added, vec![NodePolyfill::File]);
    assert_eq!(
        installed,
        BTreeSet::from([NodePolyfill::Crypto, NodePolyfill::File])
    );
}
