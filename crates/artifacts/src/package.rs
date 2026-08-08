use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("artifact path {0} is not inside base dir {1}")]
    OutsideBase(PathBuf, PathBuf),
}

/// The values PROMPT.md's `ArtifactReady` message needs, computed once the
/// archive has been written to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipSummary {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Zips `files` (each must live inside `base_dir`) into `output_path`,
/// using each file's path relative to `base_dir` as its archive entry
/// name, then returns the resulting archive's size and SHA-256.
pub fn package_zip(
    files: &[PathBuf],
    base_dir: &Path,
    output_path: &Path,
) -> Result<ZipSummary, PackageError> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| PackageError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let output_file = File::create(output_path).map_err(|source| PackageError::Io {
        path: output_path.to_path_buf(),
        source,
    })?;
    let mut writer = ZipWriter::new(BufWriter::new(output_file));
    let options = SimpleFileOptions::default();

    for file in files {
        let relative = file
            .strip_prefix(base_dir)
            .map_err(|_| PackageError::OutsideBase(file.clone(), base_dir.to_path_buf()))?;
        let entry_name = relative.to_string_lossy().replace('\\', "/");

        writer.start_file(entry_name, options)?;
        let mut input = File::open(file).map_err(|source| PackageError::Io {
            path: file.clone(),
            source,
        })?;
        io::copy(&mut input, &mut writer).map_err(|source| PackageError::Io {
            path: file.clone(),
            source,
        })?;
    }

    writer.finish()?;

    let size_bytes = std::fs::metadata(output_path)
        .map_err(|source| PackageError::Io {
            path: output_path.to_path_buf(),
            source,
        })?
        .len();
    let sha256 = sha256_file(output_path)?;

    Ok(ZipSummary {
        path: output_path.to_path_buf(),
        size_bytes,
        sha256,
    })
}

fn sha256_file(path: &Path) -> Result<String, PackageError> {
    let file = File::open(path).map_err(|source| PackageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = reader.read(&mut buf).map_err(|source| PackageError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn packages_files_preserving_relative_structure_and_reports_correct_hash() {
        let src = tempfile::tempdir().expect("tempdir");
        let nested = src.path().join("dist").join("assets");
        fs::create_dir_all(&nested).expect("mkdir");
        fs::write(nested.join("app.js"), b"console.log('hi');").expect("write");
        fs::write(src.path().join("dist").join("index.html"), b"<html></html>").expect("write");

        let files = vec![
            src.path().join("dist").join("index.html"),
            nested.join("app.js"),
        ];

        let out_dir = tempfile::tempdir().expect("tempdir");
        let output_path = out_dir.path().join("artifacts.distbuild.zip");

        let summary = package_zip(&files, src.path(), &output_path).expect("package_zip");

        assert_eq!(summary.path, output_path);
        assert!(summary.size_bytes > 0);
        assert_eq!(summary.sha256.len(), 64);

        // Independently recompute the hash to confirm it matches what package_zip reported.
        let mut file_bytes = Vec::new();
        File::open(&output_path)
            .expect("open zip")
            .read_to_end(&mut file_bytes)
            .expect("read zip");
        let mut hasher = Sha256::new();
        hasher.update(&file_bytes);
        assert_eq!(summary.sha256, hex::encode(hasher.finalize()));

        // Confirm the archive actually contains both entries with forward-slash paths.
        let archive_file = File::open(&output_path).expect("reopen zip");
        let mut archive = zip::ZipArchive::new(archive_file).expect("read archive");
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).expect("entry").name().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["dist/assets/app.js", "dist/index.html"]);
    }

    #[test]
    fn rejects_files_outside_base_dir() {
        let base = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let stray_file = outside.path().join("stray.txt");
        fs::write(&stray_file, b"nope").expect("write");

        let out_dir = tempfile::tempdir().expect("tempdir");
        let output_path = out_dir.path().join("artifacts.distbuild.zip");

        let result = package_zip(&[stray_file], base.path(), &output_path);
        assert!(matches!(result, Err(PackageError::OutsideBase(_, _))));
    }
}
