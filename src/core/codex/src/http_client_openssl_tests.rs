use super::native_tls_builder;
use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::ssl::{SslAcceptor, SslMethod};
use openssl::x509::extension::{BasicConstraints, KeyUsage, SubjectAlternativeName};
use openssl::x509::{X509, X509NameBuilder};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

fn certificate(key: &PKey<Private>, matching_host: bool) -> X509 {
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", "synthetic.invalid")
        .unwrap();
    let name = name.build();
    let mut certificate = X509::builder().unwrap();
    certificate.set_version(2).unwrap();
    certificate
        .set_serial_number(&BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap())
        .unwrap();
    certificate.set_subject_name(&name).unwrap();
    certificate.set_issuer_name(&name).unwrap();
    certificate.set_pubkey(key).unwrap();
    certificate
        .set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    certificate
        .set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    certificate
        .append_extension(BasicConstraints::new().critical().ca().build().unwrap())
        .unwrap();
    certificate
        .append_extension(
            KeyUsage::new()
                .digital_signature()
                .key_encipherment()
                .key_cert_sign()
                .build()
                .unwrap(),
        )
        .unwrap();
    let alternative = SubjectAlternativeName::new()
        .ip(if matching_host {
            "127.0.0.1"
        } else {
            "127.0.0.2"
        })
        .build(&certificate.x509v3_context(None, None))
        .unwrap();
    certificate.append_extension(alternative).unwrap();
    certificate.sign(key, MessageDigest::sha256()).unwrap();
    certificate.build()
}

#[tokio::test]
async fn linux_native_tls_requires_trust_and_hostname_before_sending_http() {
    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    for (trusted, matching_host) in [(false, true), (true, false), (true, true)] {
        let certificate = certificate(&key, matching_host);
        let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        acceptor.set_private_key(&key).unwrap();
        acceptor.set_certificate(&certificate).unwrap();
        acceptor.check_private_key().unwrap();
        let acceptor = acceptor.build();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("https://{}/tls-probe", listener.local_addr().unwrap());
        crate::test_support::assert_loopback_url(&url);
        listener.set_nonblocking(true).unwrap();
        let server = tokio::task::spawn_blocking(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "TLS test accept deadline");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => panic!("TLS test accept failed"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let Ok(mut stream) = acceptor.accept(stream) else {
                return false;
            };
            let mut request = [0; 4096];
            let Ok(size) = stream.read(&mut request) else {
                return false;
            };
            if size == 0 {
                return false;
            }
            assert!(request[..size].starts_with(b"GET /tls-probe HTTP/1.1\r\n"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
            true
        });
        let mut builder = native_tls_builder()
            .no_proxy()
            .timeout(Duration::from_secs(3));
        if trusted {
            builder = builder.add_root_certificate(
                reqwest::Certificate::from_der(&certificate.to_der().unwrap()).unwrap(),
            );
        }
        let result = builder.build().unwrap().get(url).send().await;
        let should_succeed = trusted && matching_host;
        if should_succeed {
            let response = result.expect("trusted matching loopback TLS");
            assert_eq!(response.status(), http::StatusCode::OK);
            assert_eq!(response.text().await.unwrap(), "ok");
        } else {
            assert!(result.expect_err("invalid TLS peer accepted").is_connect());
        }
        assert_eq!(server.await.unwrap(), should_succeed);
    }
}

#[test]
fn linux_links_the_pinned_native_openssl_release() {
    assert_eq!(openssl::version::number(), 0x3060_0040);
}
