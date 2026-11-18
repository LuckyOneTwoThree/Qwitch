use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::domain::CredentialEnvelope;
use crate::storage::StorageError;

const NONCE_LEN: usize = 12;

pub fn encrypt_credential(
    master_key: &[u8; 32],
    envelope: &CredentialEnvelope,
) -> Result<Vec<u8>, StorageError> {
    let plaintext =
        serde_json::to_vec(envelope).map_err(|e| StorageError::Crypto(e.to_string()))?;

    let cipher =
        Aes256Gcm::new_from_slice(master_key).map_err(|e| StorageError::Crypto(e.to_string()))?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|e| StorageError::Crypto(format!("Encryption failed: {e}")))?;

    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn decrypt_credential(
    master_key: &[u8; 32],
    blob: &[u8],
) -> Result<CredentialEnvelope, StorageError> {
    if blob.len() <= NONCE_LEN {
        return Err(StorageError::Crypto("Encrypted blob is too short".into()));
    }

    let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
    let cipher =
        Aes256Gcm::new_from_slice(master_key).map_err(|e| StorageError::Crypto(e.to_string()))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| StorageError::Crypto(format!("Decryption failed: {e}")))?;

    serde_json::from_slice(&plaintext).map_err(|e| StorageError::Crypto(e.to_string()))
}

pub fn key_fingerprint(master_key: &[u8; 32]) -> String {
    hex::encode(&Sha256::digest(master_key)[..16])
}

pub fn compute_uid_hash(tool_id: &str, uid: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tool_id.as_bytes());
    hasher.update(b":");
    hasher.update(uid.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn mask_uid(uid: &str) -> String {
    let chars: Vec<char> = uid.chars().collect();
    if chars.len() <= 6 {
        return "***".to_string();
    }
    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars.iter().rev().take(2).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{prefix}****{suffix}")
}

pub fn mask_email(email: &str) -> String {
    if email.contains("****") {
        return email.to_string();
    }
    match email.split_once('@') {
        Some((local, domain)) => {
            let visible: String = local.chars().take(2).collect();
            format!("{visible}***@{domain}")
        }
        None => "***".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::CredentialPayload;
    use std::collections::HashMap;

    #[test]
    fn encrypt_decrypt_roundtrip_and_tamper_detection() {
        let key = [7u8; 32];
        let env = CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid: "user_12345678".to_string(),
            display_name: Some("Tester".to_string()),
            display_email: Some("alice@example.com".to_string()),
            payload: CredentialPayload::Token {
                access_token: "secret-token-abc".to_string(),
                refresh_token: Some("refresh-xyz".to_string()),
                expires_at: Some(1_900_000_000),
                extra: HashMap::new(),
            },
        };

        let mut encrypted = encrypt_credential(&key, &env).expect("encrypt ok");
        let decrypted = decrypt_credential(&key, &encrypted).expect("decrypt ok");
        assert_eq!(decrypted.uid, env.uid);
        assert_eq!(decrypted.display_email, env.display_email);

        let last_idx = encrypted.len() - 1;
        encrypted[last_idx] ^= 0xFF;
        assert!(decrypt_credential(&key, &encrypted).is_err());
    }

    #[test]
    fn uid_hash_and_masking_rules() {
        let h1 = compute_uid_hash("trae_cn", "u_12345");
        let h2 = compute_uid_hash("trae_cn", "u_12345");
        let h3 = compute_uid_hash("qoder_cn", "u_12345");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);

        assert_eq!(mask_uid("123456"), "***");
        assert_eq!(mask_uid("u_8f3a91c"), "u_8f****1c");
        assert_eq!(mask_email("alice@example.com"), "al***@example.com");
        assert_eq!(mask_email("not-an-email"), "***");
    }
}
