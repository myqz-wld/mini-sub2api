//! Native custom-CA precedence. Errors deliberately omit local paths and certificate bytes.
use anyhow::{Context, Result};
use rustls_pki_types::{
    CertificateDer,
    pem::{PemObject, SectionKind},
};
use std::ffi::OsString;
use std::path::PathBuf;

pub(crate) fn selected_path(get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    ["CODEX_CA_CERTIFICATE", "SSL_CERT_FILE"]
        .into_iter()
        .find_map(|key| get(key).filter(|v| !v.is_empty()).map(PathBuf::from))
}

pub(crate) fn certificates() -> Result<Option<Vec<CertificateDer<'static>>>> {
    let Some(path) = selected_path(|key| std::env::var_os(key)) else {
        return Ok(None);
    };
    let pem =
        std::fs::read(path).map_err(|_| anyhow::anyhow!("cannot read configured CA bundle"))?;
    parse(&pem).map(Some)
}

pub(crate) fn parse(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>> {
    let pem = String::from_utf8_lossy(pem);
    let trusted = pem.contains("TRUSTED CERTIFICATE");
    let normalized = pem
        .replace("BEGIN TRUSTED CERTIFICATE", "BEGIN CERTIFICATE")
        .replace("END TRUSTED CERTIFICATE", "END CERTIFICATE");
    let mut certs = Vec::new();
    for section in <(SectionKind, Vec<u8>)>::pem_slice_iter(normalized.as_bytes()) {
        let (kind, der) =
            section.map_err(|_| anyhow::anyhow!("invalid configured CA PEM bundle"))?;
        if kind == SectionKind::Certificate {
            let der = if trusted {
                first_der_item(&der).context("invalid trusted CA certificate length")?
            } else {
                &der
            };
            certs.push(CertificateDer::from(der.to_vec()));
        }
    }
    anyhow::ensure!(
        !certs.is_empty(),
        "configured CA bundle contains no certificates"
    );
    // Match the WS path's strict registration policy for every root in the bundle.
    let mut roots = rustls::RootCertStore::empty();
    for cert in &certs {
        roots
            .add(cert.clone())
            .context("invalid configured CA certificate")?;
    }
    Ok(certs)
}

// OpenSSL may append X509_AUX trust metadata. Registration still validates the certificate.
fn first_der_item(der: &[u8]) -> Option<&[u8]> {
    let length = *der.get(1)?;
    let (header, content) = if length & 0x80 == 0 {
        (2usize, usize::from(length))
    } else {
        let count = usize::from(length & 0x7f);
        if count == 0 {
            return None;
        }
        let end = 2usize.checked_add(count)?;
        let content = der.get(2..end)?.iter().try_fold(0usize, |n, b| {
            n.checked_mul(256)?.checked_add(usize::from(*b))
        })?;
        (end, content)
    };
    der.get(..header.checked_add(content)?)
}

pub(crate) fn apply(mut builder: reqwest::ClientBuilder) -> Result<reqwest::ClientBuilder> {
    if let Some(certs) = certificates()? {
        crate::transport_registry::ensure_aws_lc_provider()?;
        builder = builder.use_rustls_tls();
        for cert in certs {
            builder = builder.add_root_certificate(
                reqwest::Certificate::from_der(&cert)
                    .context("invalid configured CA certificate")?,
            );
        }
    }
    Ok(builder)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auxiliary_trimming_checks_der_bounds_without_accepting_invalid_roots() {
        assert_eq!(
            first_der_item(&[0x30, 2, 1, 2, 0x30, 0]),
            Some([0x30, 2, 1, 2].as_slice())
        );
        assert_eq!(
            first_der_item(&[0x30, 0x81, 2, 1, 2, 0x30, 0]),
            Some([0x30, 0x81, 2, 1, 2].as_slice())
        );
        for bytes in [
            &[][..],
            &[0x30],
            &[0x30, 0x80, 0, 0],
            &[0x30, 0x82, 1],
            &[0x30, 2, 1],
            &[0x30, 0xff, 0],
        ] {
            assert!(first_der_item(bytes).is_none());
        }
        assert!(
            parse(
                b"-----BEGIN TRUSTED CERTIFICATE-----\nMAIBAQ==\n-----END TRUSTED CERTIFICATE-----"
            )
            .is_err()
        );
    }
    #[test]
    fn precedence_and_empty_values_do_not_mutate_process_environment() {
        for (codex, ssl, expected) in [
            ("first.pem", "second.pem", Some("first.pem")),
            ("", "second.pem", Some("second.pem")),
            ("", "", None),
        ] {
            assert_eq!(
                selected_path(|key| Some(
                    if key == "CODEX_CA_CERTIFICATE" {
                        codex
                    } else {
                        ssl
                    }
                    .into()
                )),
                expected.map(PathBuf::from)
            );
        }
        for pem in [
            b"".as_slice(),
            b"not a certificate",
            b"-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----",
        ] {
            assert!(parse(pem).is_err());
        }
    }
}
