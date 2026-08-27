use crate::cloudflare_cookies;
use crate::http_client::has_literal_loopback_host;
use crate::http_client::native_tls_builder;
use crate::websocket_connector::WebSocketConnector;
use anyhow::Context;
use anyhow::Result;
use reqwest::Client;
use reqwest::ClientBuilder;
use rustls::ClientConfig;
use rustls::RootCertStore;
use rustls::SignatureScheme;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(300);
const REQUIRED_AWS_LC_SIGNATURE_SCHEME: SignatureScheme = SignatureScheme::ECDSA_NISTP521_SHA512;

/// Current transport-policy input. A later core-owned `EgressRoute` can advance this revision and
/// construct a replacement context without changing the coordinator/core request protocol.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct CredentialTransportPolicy {
    egress_revision: u64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TransportKey {
    account_ref: String,
    egress_revision: u64,
}

pub(crate) struct CredentialTransportContext {
    http: Client,
    direct_http: Client,
    websocket: WebSocketConnector,
    direct_websocket: WebSocketConnector,
}

impl CredentialTransportContext {
    pub(crate) fn http_client_for_url(&self, url: &str) -> &Client {
        if has_literal_loopback_host(url) {
            &self.direct_http
        } else {
            &self.http
        }
    }

    pub(crate) fn websocket_connector_for_url(&self, url: &str) -> &WebSocketConnector {
        if has_literal_loopback_host(url) {
            &self.direct_websocket
        } else {
            &self.websocket
        }
    }
}

pub(crate) struct TransportRegistry {
    contexts: Mutex<HashMap<TransportKey, Arc<CredentialTransportContext>>>,
    factory: TransportFactory,
}

impl TransportRegistry {
    pub(crate) fn new() -> Result<Self> {
        Self::new_inner(None)
    }

    #[cfg(test)]
    pub(crate) fn new_with_proxy_url(proxy_url: &str) -> Result<Self> {
        Self::new_inner(Some(proxy_url))
    }

    fn new_inner(explicit_proxy_url: Option<&str>) -> Result<Self> {
        Ok(Self {
            contexts: Mutex::new(HashMap::new()),
            factory: TransportFactory::new(explicit_proxy_url)?,
        })
    }

    pub(crate) fn context(
        &self,
        account_ref: &str,
        policy: CredentialTransportPolicy,
    ) -> Result<Arc<CredentialTransportContext>> {
        anyhow::ensure!(!account_ref.is_empty(), "empty credential transport key");
        let key = TransportKey {
            account_ref: account_ref.to_string(),
            egress_revision: policy.egress_revision,
        };
        let mut contexts = self
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("credential transport registry lock poisoned"))?;
        if let Some(context) = contexts.get(&key) {
            return Ok(Arc::clone(context));
        }
        let context = Arc::new(self.factory.build()?);
        contexts.insert(key, Arc::clone(&context));
        Ok(context)
    }
}

struct TransportFactory {
    websocket_roots: RootCertStore,
    explicit_proxy: Option<reqwest::Proxy>,
    explicit_proxy_url: Option<String>,
}

impl TransportFactory {
    fn new(explicit_proxy_url: Option<&str>) -> Result<Self> {
        let (websocket_roots, _) = load_websocket_roots()?;
        let explicit_proxy = explicit_proxy_url
            .map(reqwest::Proxy::all)
            .transpose()
            .context("building explicit test proxy")?;
        Ok(Self {
            websocket_roots,
            explicit_proxy,
            explicit_proxy_url: explicit_proxy_url.map(str::to_string),
        })
    }

    fn build(&self) -> Result<CredentialTransportContext> {
        let http = self
            .http_builder()
            .build()
            .context("building credential HTTP client")?;
        let direct_http = self
            .http_builder()
            .no_proxy()
            .build()
            .context("building direct credential HTTP client")?;
        let websocket = match self.explicit_proxy_url.as_deref() {
            Some(proxy_url) => WebSocketConnector::with_proxy(
                self.websocket_roots.clone(),
                CONNECT_TIMEOUT,
                proxy_url,
            ),
            None => WebSocketConnector::system(self.websocket_roots.clone(), CONNECT_TIMEOUT),
        };
        let direct_websocket =
            WebSocketConnector::direct(self.websocket_roots.clone(), CONNECT_TIMEOUT);
        Ok(CredentialTransportContext {
            http,
            direct_http,
            websocket,
            direct_websocket,
        })
    }

    /// Match Codex HTTP with the deployment platform's native TLS backend. WebSocket TLS remains
    /// on its separate explicit AWS-LC rustls configuration below.
    fn http_builder(&self) -> ClientBuilder {
        cloudflare_cookies::apply(
            self.apply_explicit_test_proxy(
                native_tls_builder()
                    .connect_timeout(CONNECT_TIMEOUT)
                    .read_timeout(HTTP_READ_TIMEOUT)
                    .redirect(reqwest::redirect::Policy::none()),
            ),
        )
    }

    fn apply_explicit_test_proxy(&self, builder: ClientBuilder) -> ClientBuilder {
        match &self.explicit_proxy {
            Some(proxy) => builder.proxy(proxy.clone()),
            None => builder,
        }
    }
}

fn load_websocket_roots() -> Result<(RootCertStore, usize)> {
    ensure_aws_lc_provider()?;
    let mut roots = RootCertStore::empty();
    let rustls_native_certs::CertificateResult { certs, errors, .. } =
        rustls_native_certs::load_native_certs();
    if !errors.is_empty() {
        tracing::warn!(
            native_root_error_count = errors.len(),
            "encountered errors while loading native root certificates"
        );
    }
    let (accepted, _) = roots.add_parsable_certificates(certs);
    anyhow::ensure!(accepted > 0, "no platform-native TLS roots were available");
    Ok((roots, accepted))
}

pub(crate) fn build_websocket_tls_config(roots: RootCertStore) -> Result<ClientConfig> {
    ensure_aws_lc_provider()?;
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(config)
}

fn ensure_aws_lc_provider() -> Result<()> {
    static RESULT: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    RESULT
        .get_or_init(|| {
            if rustls::crypto::aws_lc_rs::default_provider()
                .install_default()
                .is_ok()
            {
                return Ok(());
            }
            let Some(provider) = rustls::crypto::CryptoProvider::get_default() else {
                return Err("AWS-LC rustls provider was not installed".to_string());
            };
            if provider
                .signature_verification_algorithms
                .supported_schemes()
                .contains(&REQUIRED_AWS_LC_SIGNATURE_SCHEME)
            {
                Ok(())
            } else {
                Err("installed rustls provider lacks the required AWS-LC signature scheme".into())
            }
        })
        .clone()
        .map_err(anyhow::Error::msg)
}

#[cfg(test)]
#[path = "transport_registry_tests.rs"]
mod tests;
