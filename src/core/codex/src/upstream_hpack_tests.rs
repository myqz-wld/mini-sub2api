//! Inspect bytes from the actual production request builder, not an independently built header.
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn subscription_authorization_uses_native_without_indexing_on_http2() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/responses", listener.local_addr().unwrap());
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut preface = [0; 24];
        socket.read_exact(&mut preface).await.unwrap();
        assert_eq!(&preface, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
        socket
            .write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0])
            .await
            .unwrap();
        let mut headers = Vec::new();
        loop {
            let mut header = [0; 9];
            socket.read_exact(&mut header).await.unwrap();
            let length =
                ((header[0] as usize) << 16) | ((header[1] as usize) << 8) | header[2] as usize;
            assert!(length < 65536);
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            if header[3] == 4 && header[4] & 1 == 0 {
                socket
                    .write_all(&[0, 0, 0, 4, 1, 0, 0, 0, 0])
                    .await
                    .unwrap();
            }
            if header[3] == 1 {
                assert_ne!(header[4] & 4, 0);
                headers = body;
            }
            if matches!(header[3], 0 | 1) && header[4] & 1 != 0 {
                let mut response = vec![0, 0, 1, 1, 5];
                response.extend_from_slice(&header[5..9]);
                response.push(0x88);
                socket.write_all(&response).await.unwrap();
                break;
            }
        }
        authorization_prefix(&headers)
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .http2_prior_knowledge()
        .build()
        .unwrap();
    let request = build(
        &client,
        &HeaderMap::new(),
        &url,
        &ResolvedAuth::CodexOAuth {
            token: "synthetic-only".into(),
            account_id: "synthetic".into(),
        },
        UpstreamProfile::CodexSubscription1560,
        bytes::Bytes::from_static(b"{}"),
    )
    .unwrap();
    assert!(!request.headers()[http::header::AUTHORIZATION].is_sensitive());
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), client.execute(request))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), 200);
    // Static HPACK name index 23 with a four-bit zero prefix, as in native 0.156.0.
    assert_eq!(peer.await.unwrap(), vec![0x0f, 0x08]);
}

fn integer(data: &[u8], offset: &mut usize, bits: u8) -> usize {
    let mask = (1u8 << bits) - 1;
    let mut value = (data[*offset] & mask) as usize;
    *offset += 1;
    if value == mask as usize {
        let mut shift = 0;
        loop {
            let byte = data[*offset];
            *offset += 1;
            value += ((byte & 127) as usize) << shift;
            if byte & 128 == 0 {
                break;
            }
            shift += 7;
        }
    }
    value
}
fn skip_string(data: &[u8], offset: &mut usize) {
    let size = integer(data, offset, 7);
    *offset += size;
}
fn authorization_prefix(data: &[u8]) -> Vec<u8> {
    let mut offset = 0;
    while offset < data.len() {
        let start = offset;
        let byte = data[offset];
        if byte & 128 != 0 {
            integer(data, &mut offset, 7);
            continue;
        }
        if byte & 0xe0 == 0x20 {
            integer(data, &mut offset, 5);
            continue;
        }
        let name = integer(data, &mut offset, if byte & 64 != 0 { 6 } else { 4 });
        if name == 23 {
            return data[start..offset].to_vec();
        }
        if name == 0 {
            skip_string(data, &mut offset);
        }
        skip_string(data, &mut offset);
    }
    panic!("authorization name missing from HPACK block");
}
