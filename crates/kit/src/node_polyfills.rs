use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodePolyfill {
    Crypto,
    File,
}

pub fn available_node_polyfills() -> BTreeSet<NodePolyfill> {
    BTreeSet::from([NodePolyfill::Crypto, NodePolyfill::File])
}

pub fn install_node_polyfills(installed: &mut BTreeSet<NodePolyfill>) -> Vec<NodePolyfill> {
    let mut added = Vec::new();
    for polyfill in available_node_polyfills() {
        if installed.insert(polyfill) {
            added.push(polyfill);
        }
    }
    added
}
