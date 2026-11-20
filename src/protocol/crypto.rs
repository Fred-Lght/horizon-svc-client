// SPDX-License-Identifier: GPL-3.0-or-later
//! AES-128-GCM for SVC UDP datagrams.
//!
//! SVC's `Secret.encrypt` emits `IV(12) || ciphertext || tag(16)`. The IV is fresh
//! for every packet; reusing one would break GCM, so it is generated here.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};

#[derive(Debug)]
pub enum CryptoError {
    Encryption,
    Decryption,
}

pub struct Cipher {
    inner: Aes128Gcm,
}

impl Cipher {
    pub fn new(key: &[u8; 16]) -> Self {
        Self {
            // A 16-byte slice is always a valid AES-128 key.
            inner: Aes128Gcm::new_from_slice(key).expect("16-byte AES key"),
        }
    }

    pub fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let iv = rand::random::<[u8; 12]>();
        let nonce = Nonce::from(iv);
        let encrypted = self
            .inner
            .encrypt(&nonce, plain)
            .map_err(|_| CryptoError::Encryption)?;
        let mut out = Vec::with_capacity(iv.len() + encrypted.len());
        out.extend_from_slice(&iv);
        out.extend_from_slice(&encrypted);
        Ok(out)
    }

    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if sealed.len() < 12 + 16 {
            return Err(CryptoError::Decryption);
        }
        let (iv, body) = sealed.split_at(12);
        let iv: [u8; 12] = iv.try_into().map_err(|_| CryptoError::Decryption)?;
        let nonce = Nonce::from(iv);
        self.inner
            .decrypt(&nonce, body)
            .map_err(|_| CryptoError::Decryption)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_fresh_iv() {
        let cipher = Cipher::new(&[5u8; 16]);
        let a = cipher.seal(b"hello").unwrap();
        let b = cipher.seal(b"hello").unwrap();
        assert_ne!(a, b, "each packet has its own nonce");
        assert_eq!(cipher.open(&a).unwrap(), b"hello");
        assert_eq!(cipher.open(&b).unwrap(), b"hello");
    }

    #[test]
    fn a_wrong_key_or_tampering_is_rejected() {
        let cipher = Cipher::new(&[5u8; 16]);
        let other = Cipher::new(&[6u8; 16]);
        let sealed = cipher.seal(b"hello").unwrap();
        assert!(other.open(&sealed).is_err());
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(cipher.open(&tampered).is_err());
        assert!(cipher.open(&[0u8; 12]).is_err());
    }
}
