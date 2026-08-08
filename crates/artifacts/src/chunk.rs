use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ChunkError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Header sent alongside each binary WebSocket frame when streaming an
/// artifact archive to the Master, per PROMPT.md's artifacts section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkHeader {
    pub job_id: String,
    pub chunk_index: u32,
    pub total_chunks: u32,
}

/// Splits the file at `path` into `chunk_size`-byte pieces (the last chunk
/// may be smaller), pairing each with a [`ChunkHeader`] describing its
/// position in the sequence. An empty file yields a single zero-length
/// chunk so the receiver still gets a `total_chunks = 1` framing.
pub fn chunk_file(
    path: &Path,
    job_id: &str,
    chunk_size: usize,
) -> Result<Vec<(ChunkHeader, Vec<u8>)>, ChunkError> {
    assert!(chunk_size > 0, "chunk_size must be positive");

    let mut file = File::open(path).map_err(|source| ChunkError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|source| ChunkError::Io {
            path: path.to_path_buf(),
            source,
        })?;

    if data.is_empty() {
        return Ok(vec![(
            ChunkHeader {
                job_id: job_id.to_string(),
                chunk_index: 0,
                total_chunks: 1,
            },
            Vec::new(),
        )]);
    }

    let total_chunks = data.len().div_ceil(chunk_size) as u32;
    let chunks = data
        .chunks(chunk_size)
        .enumerate()
        .map(|(i, bytes)| {
            (
                ChunkHeader {
                    job_id: job_id.to_string(),
                    chunk_index: i as u32,
                    total_chunks,
                },
                bytes.to_vec(),
            )
        })
        .collect();

    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn splits_data_into_expected_chunk_count_and_preserves_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("artifacts.zip");
        let data = vec![7u8; 2_500];
        fs::write(&path, &data).expect("write");

        let chunks = chunk_file(&path, "job-1", 1_000).expect("chunk_file");

        assert_eq!(chunks.len(), 3);
        for (i, (header, _bytes)) in chunks.iter().enumerate() {
            assert_eq!(header.job_id, "job-1");
            assert_eq!(header.chunk_index, i as u32);
            assert_eq!(header.total_chunks, 3);
        }
        assert_eq!(chunks[0].1.len(), 1_000);
        assert_eq!(chunks[1].1.len(), 1_000);
        assert_eq!(chunks[2].1.len(), 500);

        let reassembled: Vec<u8> = chunks.iter().flat_map(|(_, bytes)| bytes.clone()).collect();
        assert_eq!(reassembled, data);
    }

    #[test]
    fn empty_file_yields_single_empty_chunk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty.zip");
        fs::write(&path, []).expect("write");

        let chunks = chunk_file(&path, "job-2", 1_000).expect("chunk_file");

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].0.chunk_index, 0);
        assert_eq!(chunks[0].0.total_chunks, 1);
        assert!(chunks[0].1.is_empty());
    }

    #[test]
    fn exact_multiple_of_chunk_size_does_not_produce_trailing_empty_chunk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("exact.zip");
        fs::write(&path, vec![1u8; 2_000]).expect("write");

        let chunks = chunk_file(&path, "job-3", 1_000).expect("chunk_file");

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].0.total_chunks, 2);
    }
}
