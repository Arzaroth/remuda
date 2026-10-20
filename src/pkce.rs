use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

pub fn random() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// A random version 4 UUID.
pub fn uuid() -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uuid_is_version_4() {
        let id = uuid().unwrap();
        let parts: Vec<_> = id.split('-').map(str::len).collect();
        assert_eq!(parts, [8, 4, 4, 4, 12], "{id}");
        assert_eq!(&id[14..15], "4");
        assert!("89ab".contains(&id[19..20]), "{id}");
        assert_ne!(id, uuid().unwrap());
    }
}
