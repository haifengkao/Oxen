use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::error::OxenError;
use crate::util::hasher;

pub const VERSION_METADATA_FILE_NAME: &str = "meta.json";

const MIN_COMPRESS_BYTES: usize = 16 * 1024;
const MAX_COMPRESS_BYTES: usize = 128 * 1024 * 1024;
const MIN_SAVED_BYTES: usize = 1024;
const ZSTD_LEVEL: i32 = 3;
const ZSTD_CODEC_VERSION: u32 = 1;

#[derive(Debug)]
pub struct EncodedVersion {
    pub bytes: Vec<u8>,
    pub metadata: Option<VersionEncodingMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionEncodingMetadata {
    pub encoding: VersionEncoding,
    pub raw_hash: String,
    pub raw_size: u64,
    pub stored_size: u64,
    pub codec_version: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VersionEncoding {
    Zstd,
}

impl VersionEncodingMetadata {
    pub fn is_compressed(&self) -> bool {
        matches!(self.encoding, VersionEncoding::Zstd)
    }
}

pub fn encode_for_storage(hash: &str, raw: &[u8]) -> Result<EncodedVersion, OxenError> {
    if !should_try_compression(hash, raw) {
        return Ok(identity(raw));
    }

    let compressed = zstd::bulk::compress(raw, ZSTD_LEVEL)?;
    if !compression_is_worthwhile(raw.len(), compressed.len()) {
        return Ok(identity(raw));
    }

    Ok(EncodedVersion {
        metadata: Some(VersionEncodingMetadata {
            encoding: VersionEncoding::Zstd,
            raw_hash: hash.to_string(),
            raw_size: raw.len() as u64,
            stored_size: compressed.len() as u64,
            codec_version: ZSTD_CODEC_VERSION,
        }),
        bytes: compressed,
    })
}

pub fn should_buffer_for_compression(size: u64) -> bool {
    size <= MAX_COMPRESS_BYTES as u64
}

pub async fn read_metadata(
    metadata_path: impl AsRef<Path>,
) -> Result<Option<VersionEncodingMetadata>, OxenError> {
    match fs::read(metadata_path).await {
        Ok(bytes) => {
            let metadata = serde_json::from_slice(&bytes)?;
            Ok(Some(metadata))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub async fn write_metadata(
    metadata_path: impl AsRef<Path>,
    metadata: &VersionEncodingMetadata,
) -> Result<(), OxenError> {
    let bytes = serde_json::to_vec_pretty(metadata)?;
    fs::write(metadata_path, bytes).await?;
    Ok(())
}

pub fn decode_from_storage(
    metadata: Option<&VersionEncodingMetadata>,
    stored: &[u8],
) -> Result<Vec<u8>, OxenError> {
    let Some(metadata) = metadata else {
        return Ok(stored.to_vec());
    };

    let raw = match metadata.encoding {
        VersionEncoding::Zstd => zstd::bulk::decompress(stored, metadata.raw_size as usize)?,
    };

    if raw.len() as u64 != metadata.raw_size {
        return Err(OxenError::basic_str(format!(
            "decoded version size mismatch: expected {}, got {}",
            metadata.raw_size,
            raw.len()
        )));
    }

    let actual_hash = hasher::hash_buffer(&raw);
    if actual_hash != metadata.raw_hash {
        return Err(OxenError::basic_str(format!(
            "decoded version hash mismatch: expected {}, got {}",
            metadata.raw_hash, actual_hash
        )));
    }

    Ok(raw)
}

fn should_try_compression(hash: &str, raw: &[u8]) -> bool {
    if raw.len() < MIN_COMPRESS_BYTES || raw.len() > MAX_COMPRESS_BYTES {
        return false;
    }

    // Existing callers sometimes pass synthetic hashes in tests. Preserve legacy behavior by only
    // compressing when the storage key is the raw content hash.
    hasher::hash_buffer(raw) == hash
}

fn compression_is_worthwhile(raw_len: usize, compressed_len: usize) -> bool {
    compressed_len < raw_len && raw_len.saturating_sub(compressed_len) >= MIN_SAVED_BYTES
}

fn identity(raw: &[u8]) -> EncodedVersion {
    EncodedVersion {
        bytes: raw.to_vec(),
        metadata: None,
    }
}
