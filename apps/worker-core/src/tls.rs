use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TlsError {
    #[error("failed to generate self-signed certificate: {0}")]
    Generate(#[from] rcgen::Error),
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// A worker's TLS identity as PEM-encoded bytes, ready for
/// `axum_server::tls_rustls::RustlsConfig::from_pem`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertPems {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
}

/// Loads the worker's self-signed certificate from `cert_dir` if one
/// already exists, or generates and persists a new one on first run.
/// Reusing the same certificate across restarts is required for the
/// Master's per-peer pinning (PROMPT.md's security section) to stay
/// valid — regenerating it every launch would break every paired Master.
pub fn load_or_generate_cert(cert_dir: &Path) -> Result<CertPems, TlsError> {
    let cert_path = cert_dir.join("cert.pem");
    let key_path = cert_dir.join("key.pem");

    if cert_path.is_file() && key_path.is_file() {
        let cert_pem = std::fs::read(&cert_path).map_err(|source| TlsError::Read {
            path: cert_path.clone(),
            source,
        })?;
        let key_pem = std::fs::read(&key_path).map_err(|source| TlsError::Read {
            path: key_path.clone(),
            source,
        })?;
        return Ok(CertPems { cert_pem, key_pem });
    }

    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    let cert_pem = cert.pem();
    let key_pem = signing_key.serialize_pem();

    std::fs::create_dir_all(cert_dir).map_err(|source| TlsError::Write {
        path: cert_dir.to_path_buf(),
        source,
    })?;
    std::fs::write(&cert_path, &cert_pem).map_err(|source| TlsError::Write {
        path: cert_path.clone(),
        source,
    })?;
    std::fs::write(&key_path, &key_pem).map_err(|source| TlsError::Write {
        path: key_path.clone(),
        source,
    })?;

    Ok(CertPems {
        cert_pem: cert_pem.into_bytes(),
        key_pem: key_pem.into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_a_certificate_on_first_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pems = load_or_generate_cert(dir.path()).expect("load_or_generate_cert");

        assert!(String::from_utf8_lossy(&pems.cert_pem).contains("BEGIN CERTIFICATE"));
        assert!(dir.path().join("cert.pem").is_file());
        assert!(dir.path().join("key.pem").is_file());
    }

    #[test]
    fn reuses_the_same_certificate_across_calls() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = load_or_generate_cert(dir.path()).expect("first load_or_generate_cert");
        let second = load_or_generate_cert(dir.path()).expect("second load_or_generate_cert");

        assert_eq!(first, second);
    }
}
