use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::{DigitallySignedStruct, Error, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};

/// Installs `aws-lc-rs` as the process-wide default rustls crypto
/// provider. Idempotent — safe to call from multiple places; the `Err`
/// case just means another call already installed it.
pub fn install_crypto_provider() {
    let _ = CryptoProvider::install_default(rustls::crypto::aws_lc_rs::default_provider());
}

pub(crate) fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// SHA-256 over the end-entity certificate's DER bytes — the fingerprint
/// pinned per worker after pairing, per PROMPT.md's security section.
pub fn fingerprint(cert: &CertificateDer<'_>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(cert.as_ref());
    hasher.finalize().into()
}

/// Accepts any presented certificate (workers self-sign; there is no CA
/// to chain to) but records its fingerprint, so the caller can pin it
/// immediately after a successful pairing. Used only for the single
/// connection that carries the pairing handshake.
#[derive(Debug)]
pub struct TofuVerifier {
    provider: Arc<CryptoProvider>,
    captured: Mutex<Option<[u8; 32]>>,
}

impl TofuVerifier {
    pub fn new(provider: Arc<CryptoProvider>) -> Arc<Self> {
        Arc::new(Self {
            provider,
            captured: Mutex::new(None),
        })
    }

    pub fn captured_fingerprint(&self) -> Option<[u8; 32]> {
        *self.captured.lock().expect("tls_verify lock poisoned")
    }
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        *self.captured.lock().expect("tls_verify lock poisoned") = Some(fingerprint(end_entity));
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Used for every connection after pairing: only accepts a certificate
/// whose fingerprint matches the one pinned for this peer. Rejects
/// everything else, including a legitimately-renewed certificate — a
/// changed fingerprint means either the worker was reinstalled (re-pair
/// deliberately) or a MITM is in progress, and this deliberately does not
/// try to distinguish the two automatically.
#[derive(Debug)]
pub struct PinnedVerifier {
    provider: Arc<CryptoProvider>,
    expected: [u8; 32],
}

impl PinnedVerifier {
    pub fn new(provider: Arc<CryptoProvider>, expected: [u8; 32]) -> Arc<Self> {
        Arc::new(Self { provider, expected })
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if fingerprint(end_entity) == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(Error::General(
                "certificate fingerprint does not match the pinned peer".to_string(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn self_signed_cert() -> CertificateDer<'static> {
        let cert_key =
            rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("rcgen");
        CertificateDer::from(cert_key.cert.der().to_vec())
    }

    #[test]
    fn tofu_verifier_accepts_and_captures_the_fingerprint() {
        install_crypto_provider();
        let verifier = TofuVerifier::new(provider());
        let cert = self_signed_cert();

        let result = verifier.verify_server_cert(
            &cert,
            &[],
            &ServerName::try_from("localhost").unwrap(),
            &[],
            UnixTime::now(),
        );
        assert!(result.is_ok());
        assert_eq!(verifier.captured_fingerprint(), Some(fingerprint(&cert)));
    }

    #[test]
    fn pinned_verifier_accepts_a_matching_fingerprint() {
        install_crypto_provider();
        let cert = self_signed_cert();
        let verifier = PinnedVerifier::new(provider(), fingerprint(&cert));

        let result = verifier.verify_server_cert(
            &cert,
            &[],
            &ServerName::try_from("localhost").unwrap(),
            &[],
            UnixTime::now(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn pinned_verifier_rejects_a_different_certificate() {
        install_crypto_provider();
        let pinned_cert = self_signed_cert();
        let presented_cert = self_signed_cert();
        let verifier = PinnedVerifier::new(provider(), fingerprint(&pinned_cert));

        let result = verifier.verify_server_cert(
            &presented_cert,
            &[],
            &ServerName::try_from("localhost").unwrap(),
            &[],
            UnixTime::now(),
        );
        assert!(result.is_err());
    }
}
