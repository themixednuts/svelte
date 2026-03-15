use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};

pub fn sha256(data: &str) -> String {
    let digest = Sha256::digest(data.as_bytes());
    STANDARD.encode(digest)
}
