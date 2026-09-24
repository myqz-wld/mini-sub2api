//! Native custom-CA precedence. Errors deliberately omit local paths and certificate bytes.
use anyhow::{Context, Result};
use rustls_pki_types::{CertificateDer, pem::PemObject};
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
    let certs = CertificateDer::pem_slice_iter(pem)
        .map(|cert| cert.map(|cert| cert.into_owned()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| anyhow::anyhow!("invalid configured CA PEM bundle"))?;
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
