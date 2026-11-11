//! The on-disk container of a Project Dream save file.
//!
//! A save file is:
//!
//! ```text
//! u32_le(ciphertext_len) || AES-128-CBC(plaintext)
//! ```
//!
//! and the plaintext is a small line based property store:
//!
//! ```text
//! <file_type>
//! <entry_count>
//! <property_id>
//! <value>
//! ... repeated <entry_count> times ...
//! <zero padding to a 16 byte boundary>
//! ```
//!
//! The key is the ASCII of `base64(passphrase)` truncated to 16 bytes and the IV is a
//! fixed string literal. Both were recovered from `HLSaveMethodEncrypted::.ctor` in the
//! game's IL2CPP binary.

use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use thiserror::Error;

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// Passphrase passed to `HLSaveMethodEncrypted`, recovered from the metadata string heap.
pub const PASSPHRASE: &str = "H4RDl1ght_S4VE";
/// The fixed IV literal, recovered from the method's string literal table.
pub const IV: [u8; 16] = *b"7%o18m'b~)mQD%o1";
const BLOCK: usize = 16;

/// Property ids used by the game. They are the CRC-like keys of the property store.
pub mod property {
    pub const SAVE_DATA_GAME: u32 = 3_294_653_793;
    pub const SETTINGS: u32 = 4_229_724_440;
    pub const GAME_VERSION: u32 = 3_246_210_213;
    pub const LANGUAGE: u32 = 2_277_579_936;
    pub const LAST_SAVED_TICKS: u32 = 2_370_327_302;
    pub const SESSION: u32 = 3_233_545_909;
    pub const NOTIFICATIONS: u32 = 1_953_936_914;
}

#[derive(Debug, Error)]
pub enum ContainerError {
    #[error("file is too short ({0} bytes) for a save container")]
    TooShort(usize),
    #[error("declared length {declared} does not match payload length {actual}")]
    LengthMismatch { declared: usize, actual: usize },
    #[error("ciphertext is not a multiple of the AES block size ({0} bytes)")]
    Unaligned(usize),
    #[error("decrypted payload is not UTF-8")]
    NotUtf8,
    #[error("malformed plaintext: {0}")]
    Malformed(&'static str),
    #[error("entry value {0:?} contains a newline")]
    NewlineInValue(u32),
}

/// One property of the store: a numeric id and its textual value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: u32,
    pub value: String,
}

impl Entry {
    pub fn new(id: u32, value: impl Into<String>) -> Self {
        Self { id, value: value.into() }
    }
}

/// A decoded save container. Order and ids are preserved so that re-encoding an
/// untouched container reproduces the file byte for byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    pub file_type: u32,
    pub entries: Vec<Entry>,
}

impl Container {
    pub fn new(file_type: u32, entries: Vec<Entry>) -> Self {
        Self { file_type, entries }
    }

    pub fn get(&self, id: u32) -> Option<&str> {
        self.entries.iter().find(|e| e.id == id).map(|e| e.value.as_str())
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut String> {
        self.entries.iter_mut().find(|e| e.id == id).map(|e| &mut e.value)
    }

    pub fn set(&mut self, id: u32, value: impl Into<String>) {
        let value = value.into();
        match self.entries.iter_mut().find(|e| e.id == id) {
            Some(entry) => entry.value = value,
            None => self.entries.push(Entry::new(id, value)),
        }
    }

    /// Decode the plaintext body of a save file (without the length prefix).
    pub fn from_plaintext(plaintext: &[u8]) -> Result<Self, ContainerError> {
        let body = without_padding(plaintext);
        let text = std::str::from_utf8(body).map_err(|_| ContainerError::NotUtf8)?;
        let mut lines = text.split('\n');

        let file_type = parse_u32(lines.next().ok_or(ContainerError::Malformed("missing file type"))?, "file type")?;
        let count = parse_u32(
            lines.next().ok_or(ContainerError::Malformed("missing entry count"))?,
            "entry count",
        )? as usize;

        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let id = parse_u32(lines.next().ok_or(ContainerError::Malformed("missing entry id"))?, "entry id")?;
            let value = lines.next().ok_or(ContainerError::Malformed("missing entry value"))?.to_string();
            entries.push(Entry { id, value });
        }
        if lines.next().is_some_and(|rest| !rest.is_empty()) {
            return Err(ContainerError::Malformed("trailing data after last entry"));
        }

        Ok(Self { file_type, entries })
    }

    /// Encode the plaintext body, zero padded to the AES block size.
    pub fn to_plaintext(&self) -> Result<Vec<u8>, ContainerError> {
        let mut out = String::with_capacity(64 * 1024);
        out.push_str(&self.file_type.to_string());
        out.push('\n');
        out.push_str(&self.entries.len().to_string());
        out.push('\n');
        for entry in &self.entries {
            if entry.value.contains('\n') {
                return Err(ContainerError::NewlineInValue(entry.id));
            }
            out.push_str(&entry.id.to_string());
            out.push('\n');
            out.push_str(&entry.value);
            out.push('\n');
        }
        let mut bytes = out.into_bytes();
        // Padding always follows from the content: the game pads the text to the
        // next 16 byte boundary, so an edit must recompute it, not reuse the old
        // count. An untouched save keeps its length, and so its exact padding.
        let padding = padding_for(bytes.len());
        bytes.resize(bytes.len() + padding, 0);
        Ok(bytes)
    }

    /// Decode a whole save file.
    pub fn decrypt(file: &[u8]) -> Result<Self, ContainerError> {
        if file.len() < 4 {
            return Err(ContainerError::TooShort(file.len()));
        }
        let declared = u32::from_le_bytes([file[0], file[1], file[2], file[3]]) as usize;
        let payload = &file[4..];
        if declared != payload.len() {
            return Err(ContainerError::LengthMismatch { declared, actual: payload.len() });
        }
        if payload.is_empty() || payload.len() % BLOCK != 0 {
            return Err(ContainerError::Unaligned(payload.len()));
        }
        let plaintext = Aes128CbcDec::new_from_slices(&derive_key(PASSPHRASE), &IV)
            .expect("key and IV have fixed, valid lengths")
            .decrypt_padded_vec_mut::<NoPadding>(payload)
            .map_err(|_| ContainerError::Unaligned(payload.len()))?;
        Self::from_plaintext(&plaintext)
    }

    /// Encode a whole save file.
    pub fn encrypt(&self) -> Result<Vec<u8>, ContainerError> {
        let plaintext = self.to_plaintext()?;
        let ciphertext = Aes128CbcEnc::new_from_slices(&derive_key(PASSPHRASE), &IV)
            .expect("key and IV have fixed, valid lengths")
            .encrypt_padded_vec_mut::<NoPadding>(&plaintext);
        let mut out = Vec::with_capacity(ciphertext.len() + 4);
        out.extend_from_slice(&(ciphertext.len() as u32).to_le_bytes());
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }
}

/// `UTF8(base64(passphrase))` truncated to the 16 byte AES-128 key.
pub fn derive_key(passphrase: &str) -> [u8; 16] {
    let encoded = base64(passphrase.as_bytes());
    let mut key = [0u8; 16];
    key.copy_from_slice(&encoded[..16]);
    key
}

fn parse_u32(text: &str, what: &'static str) -> Result<u32, ContainerError> {
    text.parse().map_err(|_| ContainerError::Malformed(what))
}

fn without_padding(plaintext: &[u8]) -> &[u8] {
    let end = plaintext.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    &plaintext[..end]
}

fn padding_for(len: usize) -> usize {
    (BLOCK - len % BLOCK) % BLOCK
}

/// Minimal standard base64 encoder (the key only needs the alphabet, no line breaks).
fn base64(input: &[u8]) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63]);
        out.push(TABLE[(n >> 12) as usize & 63]);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] } else { b'=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] } else { b'=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Container {
        Container::new(
            0,
            vec![
                Entry::new(property::SAVE_DATA_GAME, r#"{"m_levels":[]}"#),
                Entry::new(4_229_724_440, r#"{"m_masterVolume":1.0}"#),
                Entry::new(3_246_210_213, "1.10.1"),
                Entry::new(2277579936, ""),
            ],
        )
    }

    #[test]
    fn key_matches_the_game() {
        assert_eq!(&derive_key(PASSPHRASE), b"SDRSRGwxZ2h0X1M0");
    }

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64(b"H4RDl1ght_S4VE"), b"SDRSRGwxZ2h0X1M0VkU=");
        assert_eq!(base64(b"a"), b"YQ==");
        assert_eq!(base64(b"ab"), b"YWI=");
        assert_eq!(base64(b"abc"), b"YWJj");
    }

    #[test]
    fn plaintext_round_trip() {
        let container = sample();
        let plaintext = container.to_plaintext().unwrap();
        assert_eq!(plaintext.len() % BLOCK, 0);
        assert_eq!(Container::from_plaintext(&plaintext).unwrap(), container);
    }

    #[test]
    fn file_round_trip() {
        let container = sample();
        let file = container.encrypt().unwrap();
        let declared = u32::from_le_bytes([file[0], file[1], file[2], file[3]]) as usize;
        assert_eq!(declared, file.len() - 4);
        assert_eq!(Container::decrypt(&file).unwrap(), container);
    }

    #[test]
    fn rejects_truncated_length_prefix() {
        assert!(matches!(Container::decrypt(&[1, 2, 3]), Err(ContainerError::TooShort(3))));
    }

    #[test]
    fn rejects_length_mismatch() {
        let mut file = sample().encrypt().unwrap();
        file[0] ^= 0xff;
        assert!(matches!(Container::decrypt(&file), Err(ContainerError::LengthMismatch { .. })));
    }

    #[test]
    fn rejects_newline_in_value() {
        let mut container = sample();
        container.set(property::LANGUAGE, "en\nus");
        assert!(matches!(container.to_plaintext(), Err(ContainerError::NewlineInValue(_))));
    }

    #[test]
    fn editing_recomputes_the_padding() {
        let mut container = sample();
        let before = container.to_plaintext().unwrap().len();
        container.set(property::LANGUAGE, "a much longer language tag");
        let plaintext = container.to_plaintext().unwrap();
        assert_eq!(plaintext.len() % BLOCK, 0, "padding follows the new content");
        assert_ne!(plaintext.len(), before, "the edit changed the size");

        let file = container.encrypt().unwrap();
        assert_eq!(Container::decrypt(&file).unwrap(), container);
    }

    #[test]
    fn a_body_that_already_fits_a_block_is_not_padded() {
        let mut container = Container::new(0, vec![Entry::new(1, "")]);
        while container.to_plaintext().unwrap().len() % BLOCK != 0 {
            container.entries[0].value.push('x');
        }
        let plaintext = container.to_plaintext().unwrap();
        assert_eq!(plaintext.len() % BLOCK, 0);
        assert_eq!(Container::decrypt(&container.encrypt().unwrap()).unwrap(), container);
    }

    #[test]
    fn get_set() {
        let mut container = sample();
        assert_eq!(container.get(property::GAME_VERSION), Some("1.10.1"));
        container.set(property::GAME_VERSION, "1.10.2");
        assert_eq!(container.get(property::GAME_VERSION), Some("1.10.2"));
        container.set(123, "new");
        assert_eq!(container.get(123), Some("new"));
    }
}
