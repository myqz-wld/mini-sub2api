use http::HeaderMap;
use http::HeaderValue;
use uuid::Uuid;

const INFERENCE_CALL_ID_HEADER: &str = "x-codex-inference-call-id";

pub(crate) fn headers_for_retry(headers: &HeaderMap) -> HeaderMap {
    let mut retry = headers.clone();
    if retry.contains_key(INFERENCE_CALL_ID_HEADER) {
        let value = HeaderValue::from_str(&Uuid::new_v4().to_string())
            .expect("UUID is a valid header value");
        retry.insert(INFERENCE_CALL_ID_HEADER, value);
    }
    retry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_only_present_inference_call_ids() {
        let empty = HeaderMap::new();
        assert!(!headers_for_retry(&empty).contains_key(INFERENCE_CALL_ID_HEADER));

        let mut headers = HeaderMap::new();
        headers.insert(INFERENCE_CALL_ID_HEADER, HeaderValue::from_static("first"));
        let retry = headers_for_retry(&headers);
        let value = retry[INFERENCE_CALL_ID_HEADER].to_str().expect("header");
        assert_ne!(value, "first");
        assert_eq!(Uuid::parse_str(value).expect("UUID").get_version_num(), 4);
    }
}
