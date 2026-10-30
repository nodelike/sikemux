//! A client reads the port and token from the endpoint file, which can outlive a
//! crashed app. Before sending the token or a request it asks whoever answers on
//! that port to prove it holds the same token, so another program on a reused
//! port never sees the request or gets its answer trusted.

use sha2::{Digest, Sha256};

pub fn new_nonce() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub fn server_proof(token: &str, port: u16, nonce: &str) -> String {
    let message = format!("sikemux-cli-server\n{port}\n{nonce}");
    hex::encode(hmac_sha256(token.as_bytes(), message.as_bytes()))
}

pub fn same_secret(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner = Sha256::new()
        .chain_update(block.map(|byte| byte ^ 0x36))
        .chain_update(message)
        .finalize();
    let outer = Sha256::new()
        .chain_update(block.map(|byte| byte ^ 0x5c))
        .chain_update(inner)
        .finalize();
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&outer);
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_the_rfc_4231_vectors() {
        assert_eq!(
            hex::encode(hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex::encode(hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn proofs_depend_on_token_port_and_nonce() {
        let proof = server_proof("token", 4000, "nonce");
        assert!(same_secret(&proof, &server_proof("token", 4000, "nonce")));
        assert!(!same_secret(&proof, &server_proof("other", 4000, "nonce")));
        assert!(!same_secret(&proof, &server_proof("token", 4001, "nonce")));
        assert!(!same_secret(&proof, &server_proof("token", 4000, "other")));
        assert!(!same_secret("abc", "abcd"));
    }
}
