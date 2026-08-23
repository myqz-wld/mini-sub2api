use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use http::Uri;
use hyper_util::client::proxy::matcher::Intercept;
use hyper_util::client::proxy::matcher::Matcher;
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio::time::sleep_until;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::Connector;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::client_async_tls_with_config;
use tokio_tungstenite::proxy::connect_via_proxy;
use tokio_tungstenite::tungstenite::Error as WebSocketError;
use tokio_tungstenite::tungstenite::error::TlsError;
use tokio_tungstenite::tungstenite::error::UrlError;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::handshake::client::Response;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::proxy::ProxyAuth;
use tokio_tungstenite::tungstenite::proxy::ProxyConfig;
use tokio_tungstenite::tungstenite::proxy::ProxyScheme;

const HAPPY_EYEBALLS_DELAY: Duration = Duration::from_millis(250);

pub(crate) trait AsyncIo: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> AsyncIo for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

pub(crate) type WebSocketConnection = WebSocketStream<MaybeTlsStream<Box<dyn AsyncIo>>>;

pub(crate) enum WebSocketHandshake {
    Connected {
        socket: Box<WebSocketConnection>,
        response: Response,
    },
    Rejected(Response),
}

impl WebSocketHandshake {
    pub(crate) fn status(&self) -> http::StatusCode {
        match self {
            Self::Connected { response, .. } | Self::Rejected(response) => response.status(),
        }
    }

    pub(crate) fn headers(&self) -> &http::HeaderMap {
        match self {
            Self::Connected { response, .. } | Self::Rejected(response) => response.headers(),
        }
    }
}

pub(crate) struct WebSocketConnector {
    tls_config: Arc<ClientConfig>,
    proxy_matcher: Option<Matcher>,
    connect_timeout: Duration,
}

impl WebSocketConnector {
    pub(crate) fn system(tls_config: Arc<ClientConfig>, connect_timeout: Duration) -> Self {
        Self {
            tls_config,
            proxy_matcher: Some(Matcher::from_system()),
            connect_timeout,
        }
    }

    pub(crate) fn direct(tls_config: Arc<ClientConfig>, connect_timeout: Duration) -> Self {
        Self {
            tls_config,
            proxy_matcher: None,
            connect_timeout,
        }
    }

    pub(crate) fn with_proxy(
        tls_config: Arc<ClientConfig>,
        connect_timeout: Duration,
        proxy_url: &str,
    ) -> Self {
        Self {
            tls_config,
            proxy_matcher: Some(Matcher::builder().all(proxy_url).build()),
            connect_timeout,
        }
    }

    pub(crate) async fn connect(
        &self,
        request: Request,
        config: WebSocketConfig,
    ) -> Result<WebSocketHandshake, WebSocketError> {
        let proxy = self.resolve_proxy(&request)?;
        let result = tokio::time::timeout(
            self.connect_timeout,
            connect(request, config, Arc::clone(&self.tls_config), proxy),
        )
        .await
        .map_err(|_| {
            WebSocketError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "websocket connection timed out",
            ))
        })?;
        match result {
            Ok((socket, response)) => Ok(WebSocketHandshake::Connected {
                socket: Box::new(socket),
                response,
            }),
            Err(WebSocketError::Http(response)) => Ok(WebSocketHandshake::Rejected(*response)),
            Err(error) => Err(error),
        }
    }

    #[cfg(test)]
    pub(crate) fn shares_tls_state_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.tls_config, &other.tls_config)
    }

    fn resolve_proxy(&self, request: &Request) -> Result<Option<ProxyEndpoint>, WebSocketError> {
        let Some(matcher) = &self.proxy_matcher else {
            return Ok(None);
        };
        let uri = proxy_lookup_uri(request.uri())?;
        matcher
            .intercept(&uri)
            .map(ProxyEndpoint::from_intercept)
            .transpose()
    }
}

struct ProxyEndpoint {
    config: ProxyConfig,
    tls: bool,
}

impl ProxyEndpoint {
    fn from_intercept(intercept: Intercept) -> Result<Self, WebSocketError> {
        let uri = intercept.uri();
        let scheme = uri.scheme_str().unwrap_or("http");
        let (scheme, tls, default_port) = match scheme {
            "http" => (ProxyScheme::Http, false, 80),
            "https" => (ProxyScheme::Http, true, 443),
            "socks5" => (ProxyScheme::Socks5, false, 1080),
            "socks5h" => (ProxyScheme::Socks5h, false, 1080),
            _ => return Err(WebSocketError::Url(UrlError::UnsupportedProxyScheme)),
        };
        let host = uri.host().ok_or_else(invalid_proxy_config)?.to_string();
        let port = uri.port_u16().unwrap_or(default_port);
        let auth = match scheme {
            ProxyScheme::Http => decode_basic_proxy_auth(&intercept)?,
            ProxyScheme::Socks5 | ProxyScheme::Socks5h => {
                intercept.raw_auth().map(|(username, password)| ProxyAuth {
                    username: username.to_string(),
                    password: password.to_string(),
                })
            }
        };
        Ok(Self {
            config: ProxyConfig {
                scheme,
                host,
                port,
                auth,
            },
            tls,
        })
    }
}

fn decode_basic_proxy_auth(intercept: &Intercept) -> Result<Option<ProxyAuth>, WebSocketError> {
    let Some(header) = intercept.basic_auth() else {
        return Ok(None);
    };
    let raw = header.to_str().map_err(|_| invalid_proxy_config())?;
    let encoded = raw
        .strip_prefix("Basic ")
        .ok_or_else(invalid_proxy_config)?;
    let decoded = BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| invalid_proxy_config())?;
    let decoded = String::from_utf8(decoded).map_err(|_| invalid_proxy_config())?;
    let (username, password) = decoded.split_once(':').unwrap_or((&decoded, ""));
    Ok(Some(ProxyAuth {
        username: username.to_string(),
        password: password.to_string(),
    }))
}

async fn connect(
    request: Request,
    config: WebSocketConfig,
    tls_config: Arc<ClientConfig>,
    proxy: Option<ProxyEndpoint>,
) -> Result<(WebSocketConnection, Response), WebSocketError> {
    let host = websocket_host(&request)?;
    let port = websocket_port(&request)?;
    let stream: Box<dyn AsyncIo> = match proxy {
        None => Box::new(
            connect_tcp(host_port(host, port))
                .await
                .map_err(WebSocketError::Io)?,
        ),
        Some(proxy) => {
            let stream = connect_tcp(proxy.config.authority())
                .await
                .map_err(WebSocketError::Io)?;
            let stream: Box<dyn AsyncIo> = if proxy.tls {
                let server_name = ServerName::try_from(proxy.config.host.clone())
                    .map_err(|_| WebSocketError::Tls(TlsError::InvalidDnsName))?;
                let stream = TlsConnector::from(Arc::clone(&tls_config))
                    .connect(server_name, stream)
                    .await
                    .map_err(WebSocketError::Io)?;
                Box::new(stream)
            } else {
                Box::new(stream)
            };
            Box::new(connect_via_proxy(stream, &proxy.config, host, port).await?)
        }
    };

    client_async_tls_with_config(
        request,
        stream,
        Some(config),
        Some(Connector::Rustls(tls_config)),
    )
    .await
}

fn proxy_lookup_uri(uri: &Uri) -> Result<Uri, WebSocketError> {
    let scheme = match uri.scheme_str() {
        Some("ws") => "http",
        Some("wss") => "https",
        _ => return Err(WebSocketError::Url(UrlError::UnsupportedUrlScheme)),
    };
    let authority = uri
        .authority()
        .cloned()
        .ok_or(WebSocketError::Url(UrlError::NoHostName))?;
    let path_and_query = uri
        .path_and_query()
        .cloned()
        .unwrap_or_else(|| http::uri::PathAndQuery::from_static("/"));
    Uri::builder()
        .scheme(scheme)
        .authority(authority)
        .path_and_query(path_and_query)
        .build()
        .map_err(|_| invalid_proxy_config())
}

fn invalid_proxy_config() -> WebSocketError {
    WebSocketError::Url(UrlError::InvalidProxyConfig("<redacted>".to_string()))
}

fn websocket_host(request: &Request) -> Result<&str, WebSocketError> {
    request
        .uri()
        .host()
        .ok_or(WebSocketError::Url(UrlError::NoHostName))
}

fn websocket_port(request: &Request) -> Result<u16, WebSocketError> {
    request
        .uri()
        .port_u16()
        .or_else(|| match request.uri().scheme_str() {
            Some("ws") => Some(80),
            Some("wss") => Some(443),
            _ => None,
        })
        .ok_or(WebSocketError::Url(UrlError::UnsupportedUrlScheme))
}

fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

async fn connect_tcp(address: String) -> io::Result<TcpStream> {
    let addresses = tokio::net::lookup_host(address).await?.collect::<Vec<_>>();
    connect_happy_eyeballs(addresses, TcpStream::connect).await
}

async fn connect_happy_eyeballs<T, F, Fut>(
    addresses: Vec<SocketAddr>,
    mut connect: F,
) -> io::Result<T>
where
    F: FnMut(SocketAddr) -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    let mut addresses = addresses.into_iter();
    let Some(first_address) = addresses.next() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "could not resolve to any address",
        ));
    };
    let first_is_ipv4 = first_address.is_ipv4();
    let (mut preferred, mut alternate) = (VecDeque::new(), VecDeque::new());
    for address in addresses {
        if address.is_ipv4() == first_is_ipv4 {
            preferred.push_back(address);
        } else {
            alternate.push_back(address);
        }
    }
    let mut addresses = VecDeque::new();
    while !preferred.is_empty() || !alternate.is_empty() {
        if let Some(address) = alternate.pop_front() {
            addresses.push_back(address);
        }
        if let Some(address) = preferred.pop_front() {
            addresses.push_back(address);
        }
    }

    let mut attempts = FuturesUnordered::new();
    attempts.push(connect(first_address));
    let mut next_attempt_at = Instant::now() + HAPPY_EYEBALLS_DELAY;
    let mut last_error = None;
    loop {
        if addresses.is_empty() {
            match attempts.next().await {
                Some(Ok(stream)) => return Ok(stream),
                Some(Err(error)) if attempts.is_empty() => return Err(error),
                Some(Err(error)) => last_error = Some(error),
                None => {
                    return Err(last_error.unwrap_or_else(|| {
                        io::Error::other("connection attempts ended without an error")
                    }));
                }
            }
            continue;
        }
        tokio::select! {
            result = attempts.next() => {
                match result {
                    Some(Ok(stream)) => return Ok(stream),
                    Some(Err(error)) => {
                        last_error = Some(error);
                        attempts.push(connect(take_next_address(&mut addresses)?));
                        next_attempt_at = Instant::now() + HAPPY_EYEBALLS_DELAY;
                    }
                    None => {
                        attempts.push(connect(take_next_address(&mut addresses)?));
                        next_attempt_at = Instant::now() + HAPPY_EYEBALLS_DELAY;
                    }
                }
            }
            _ = sleep_until(next_attempt_at) => {
                attempts.push(connect(take_next_address(&mut addresses)?));
                next_attempt_at = Instant::now() + HAPPY_EYEBALLS_DELAY;
            }
        }
    }
}

fn take_next_address(addresses: &mut VecDeque<SocketAddr>) -> io::Result<SocketAddr> {
    addresses
        .pop_front()
        .ok_or_else(|| io::Error::other("connection address queue unexpectedly empty"))
}

#[cfg(test)]
#[path = "websocket_connector_tests.rs"]
mod tests;
