use crate::{Error, Result};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

pub fn random() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}
pub fn hash(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

pub fn seal(key: &[u8; 32], context: &str, value: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| Error::internal())?;
    let nonce = rand::random::<[u8; 12]>();
    let data = cipher
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: value,
                aad: context.as_bytes(),
            },
        )
        .map_err(|_| Error::internal())?;
    Ok([nonce.as_slice(), &data].concat())
}

pub fn open(key: &[u8; 32], context: &str, value: &[u8]) -> Result<Vec<u8>> {
    if value.len() < 28 {
        return Err(Error::internal());
    }
    let nonce: [u8; 12] = value[..12].try_into().map_err(|_| Error::internal())?;
    Aes256Gcm::new_from_slice(key)
        .map_err(|_| Error::internal())?
        .decrypt(
            &Nonce::from(nonce),
            Payload {
                msg: &value[12..],
                aad: context.as_bytes(),
            },
        )
        .map_err(|_| Error::internal())
}
