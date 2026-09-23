use super::*;
use http::StatusCode;
use http::header::{COOKIE, SET_COOKIE};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as ServerRequest, Response as ServerResponse,
};

const HOST: &str = "cookie-test.chatgpt.com";
const HOSTS: &[&str] = &[HOST, "other-cookie-test.chatgpt.com", "api.openai.com"];

#[derive(Clone)]
struct Step {
    http: bool,
    host: &'static str,
    path: &'static str,
    explicit: Option<&'static str>,
    expected: Option<&'static str>,
    status: StatusCode,
    cookies: &'static [&'static str],
}

impl Step {
    fn websocket(expected: Option<&'static str>, cookies: &'static [&'static str]) -> Self {
        Self {
            http: false,
            host: HOST,
            path: "/backend-api/responses",
            explicit: None,
            expected,
            status: StatusCode::SWITCHING_PROTOCOLS,
            cookies,
        }
    }
}

#[tokio::test]
// Tungstenite's handshake callback requires its unboxed HTTP error response.
#[allow(clippy::result_large_err)]
async fn http_and_websocket_handshakes_share_routing_cookies_with_scope_and_rejection_checks() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = proxy_listener.local_addr().unwrap();
    assert!(address.ip().is_loopback() && proxy_address.ip().is_loopback());
    let root = BASE64_STANDARD.decode(ROOT_CERTIFICATE).unwrap();
    let certificate = CertificateDer::from(BASE64_STANDARD.decode(LEAF_CERTIFICATE).unwrap());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        BASE64_STANDARD.decode(PRIVATE_KEY).unwrap(),
    ));
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(root.clone())).unwrap();
    crate::transport_registry::build_websocket_tls_config(roots.clone()).unwrap();
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certificate], key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let steps = vec![
        Step {
            http: true,
            status: StatusCode::OK,
            ..Step::websocket(
                None,
                &[
                    "__oailb=from-http; Path=/backend-api; Secure",
                    "session=never-store; Path=/; Secure",
                ],
            )
        },
        Step::websocket(
            Some("__oailb=from-http"),
            &["__oailb=from-ws; Path=/backend-api; Secure"],
        ),
        Step {
            http: true,
            status: StatusCode::OK,
            ..Step::websocket(Some("__oailb=from-ws"), &[])
        },
        Step {
            status: StatusCode::FORBIDDEN,
            ..Step::websocket(
                Some("__oailb=from-ws"),
                &["__oailb=from-rejection; Path=/backend-api; Secure"],
            )
        },
        Step::websocket(Some("__oailb=from-rejection"), &[]),
        Step {
            explicit: Some("explicit=synthetic"),
            ..Step::websocket(Some("explicit=synthetic"), &[])
        },
        Step {
            path: "/outside",
            ..Step::websocket(None, &[])
        },
        Step {
            host: HOSTS[1],
            ..Step::websocket(None, &[])
        },
        Step {
            host: HOSTS[2],
            ..Step::websocket(None, &["__oailb=untrusted; Path=/; Secure"])
        },
        Step {
            http: true,
            host: HOSTS[2],
            status: StatusCode::OK,
            ..Step::websocket(None, &[])
        },
        Step::websocket(
            Some("__oailb=from-rejection"),
            &["__oailb=; Path=/backend-api; Max-Age=0; Secure"],
        ),
        Step::websocket(None, &[]),
    ];
    let server_steps = steps.clone();
    let server = tokio::spawn(async move {
        for step in server_steps {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = acceptor.accept(stream).await.unwrap();
            if step.http {
                let raw = read_headers(&mut stream).await;
                let cookie = raw
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("cookie"))
                    .map(|(_, value)| value.trim());
                assert_eq!(cookie, step.expected);
                let mut reply =
                    String::from("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n");
                for cookie in step.cookies {
                    reply.push_str(&format!("Set-Cookie: {cookie}\r\n"));
                }
                reply.push_str("\r\n{}");
                stream.write_all(reply.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            } else {
                let upgraded = accept_hdr_async(
                    stream,
                    |request: &ServerRequest, mut response: ServerResponse| {
                        assert_eq!(request.uri().path(), step.path);
                        assert_eq!(
                            request.headers().get(COOKIE).map(|v| v.to_str().unwrap()),
                            step.expected
                        );
                        if step.status != StatusCode::SWITCHING_PROTOCOLS {
                            let mut rejected = http::Response::builder()
                                .status(step.status)
                                .body(Some(String::new()))
                                .unwrap();
                            for cookie in step.cookies {
                                rejected
                                    .headers_mut()
                                    .append(SET_COOKIE, cookie.parse().unwrap());
                            }
                            return Err(rejected);
                        }
                        for cookie in step.cookies {
                            response
                                .headers_mut()
                                .append(SET_COOKIE, cookie.parse().unwrap());
                        }
                        Ok(response)
                    },
                )
                .await;
                assert_eq!(
                    upgraded.is_ok(),
                    step.status == StatusCode::SWITCHING_PROTOCOLS
                );
            }
        }
    });
    let websocket_count = steps.iter().filter(|step| !step.http).count();
    let proxy = tokio::spawn(async move {
        for _ in 0..websocket_count {
            let (mut client, _) = proxy_listener.accept().await.unwrap();
            let request = read_headers(&mut client).await;
            assert!(HOSTS.iter().any(|host| {
                request.starts_with(&format!("CONNECT {host}:{} HTTP/1.1\r\n", address.port()))
            }));
            // This is the only tunnel destination: no target DNS lookup or external fallback.
            let mut target = TcpStream::connect(address).await.unwrap();
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            let _ = tokio::io::copy_bidirectional(&mut client, &mut target).await;
        }
    });
    let mut builder = reqwest::Client::builder()
        .use_native_tls()
        .no_proxy()
        .add_root_certificate(reqwest::Certificate::from_der(&root).unwrap());
    for host in HOSTS {
        builder = builder.resolve(host, address);
    }
    let client = crate::cloudflare_cookies::apply(builder).build().unwrap();
    let connector = WebSocketConnector::with_proxy(
        roots,
        Duration::from_secs(5),
        &format!("http://{proxy_address}"),
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        for step in steps {
            assert!(HOSTS.contains(&step.host), "unmapped test hostname");
            let scheme = if step.http { "https" } else { "wss" };
            let url = format!("{scheme}://{}:{}{}", step.host, address.port(), step.path);
            if step.http {
                let response = client.get(&url).send().await.unwrap();
                assert_eq!(response.status(), step.status);
                response.bytes().await.unwrap();
            } else {
                let mut request = url.into_client_request().unwrap();
                if let Some(value) = step.explicit {
                    request.headers_mut().insert(COOKIE, value.parse().unwrap());
                }
                assert!(
                    connector.resolve_proxy(&request).unwrap().is_some(),
                    "loopback proxy is mandatory"
                );
                let response = connector
                    .connect(request, WebSocketConfig::default())
                    .await
                    .unwrap();
                assert_eq!(response.status(), step.status);
            }
        }
        server.await.unwrap();
        proxy.await.unwrap();
    })
    .await
    .expect("bounded loopback cookie handshakes");
}

async fn read_headers(stream: &mut (impl AsyncRead + Unpin)) -> String {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        bytes.push(stream.read_u8().await.unwrap());
        assert!(bytes.len() <= 8192, "bounded test handshake");
    }
    String::from_utf8(bytes).unwrap()
}

// Public synthetic TLS fixture for loopback tests only; never a production credential.
const ROOT_CERTIFICATE: &str = "MIIBozCCAUigAwIBAgIBATAKBggqhkjOPQQDAjAtMSswKQYDVQQDEyJtaW5pLXN1YjJhcGkgc3ludGhldGljIGNvb2tpZSB0ZXN0MCAXDTAwMDEwMTAwMDAwMFoYDzIxMDAwMTAxMDAwMDAwWjAtMSswKQYDVQQDEyJtaW5pLXN1YjJhcGkgc3ludGhldGljIGNvb2tpZSB0ZXN0MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEdlM8wy/Byl27IFJjUHChlJMhVsVd/oJA9rga/UQFk6MpRHzJrk42qiu+rzitTHcdux3yfrO5xduHAaVk9otvkqNXMFUwDgYDVR0PAQH/BAQDAgKEMBMGA1UdJQQMMAoGCCsGAQUFBwMBMA8GA1UdEwEB/wQFMAMBAf8wHQYDVR0OBBYEFGBCy+LAUjKxnRLIL9AX0bR43DQJMAoGCCqGSM49BAMCA0kAMEYCIQDVNRDLyIgzpHfzr9HPs9qW5WPzpX2HERRNTgsYOF5MXwIhAKHrw8tdF2C02H8cgdszWtiPx/Ay2YQ2M9zBHX4Dp7hO";
const LEAF_CERTIFICATE: &str = "MIIB2DCCAX+gAwIBAgIBAjAKBggqhkjOPQQDAjAtMSswKQYDVQQDEyJtaW5pLXN1YjJhcGkgc3ludGhldGljIGNvb2tpZSB0ZXN0MCAXDTAwMDEwMTAwMDAwMFoYDzIxMDAwMTAxMDAwMDAwWjAxMS8wLQYDVQQDEyZtaW5pLXN1YjJhcGkgc3ludGhldGljIGNvb2tpZSBlbmRwb2ludDBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABCNRVaVDtZqB0Q61PgwTXUEhLMkL1GrITAqtvY1mrXsNUosgpCsILpiasIrpst1DdQ27/WM1lWpft7Q/HViThI2jgYkwgYYwDgYDVR0PAQH/BAQDAgeAMBMGA1UdJQQMMAoGCCsGAQUFBwMBMAwGA1UdEwEB/wQCMAAwUQYDVR0RBEowSIIXY29va2llLXRlc3QuY2hhdGdwdC5jb22CHW90aGVyLWNvb2tpZS10ZXN0LmNoYXRncHQuY29tgg5hcGkub3BlbmFpLmNvbTAKBggqhkjOPQQDAgNHADBEAiB4q9IWXOBLl7L2sn/TH0gBXolG8EIA5r494A8iAdy4JQIgV1XtalBWonhY3YW0UDYxXkMpMaRCDFKbSEnswVLV2nM=";
const PRIVATE_KEY: &str = "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgyfDKjsAAhTktgypaRbhJzPbbuqZcYlxBvNGNt+V/K4yhRANCAAQjUVWlQ7WagdEOtT4ME11BISzJC9RqyEwKrb2NZq17DVKLIKQrCC6YmrCK6bLdQ3UNu/1jNZVqX7e0Px1Yk4SN";
