use reqwest::cookie::CookieStore;
use reqwest::cookie::Jar;
use reqwest::header::HeaderValue;
use std::sync::Arc;
use std::sync::LazyLock;

// This process-wide store intentionally accepts only Cloudflare infrastructure
// cookies. Account, session, authentication, and arbitrary application cookies
// must never be added to this allowlist.
static STORE: LazyLock<Arc<ChatGptCloudflareCookieStore>> =
    LazyLock::new(|| Arc::new(ChatGptCloudflareCookieStore::default()));

#[derive(Debug, Default)]
struct ChatGptCloudflareCookieStore {
    jar: Jar,
}

impl CookieStore for ChatGptCloudflareCookieStore {
    fn set_cookies(
        &self,
        cookie_headers: &mut dyn Iterator<Item = &HeaderValue>,
        url: &reqwest::Url,
    ) {
        if !is_chatgpt_cookie_url(url) {
            return;
        }
        let mut allowed =
            cookie_headers.filter(|header| is_allowed_cloudflare_set_cookie_header(header));
        self.jar.set_cookies(&mut allowed, url);
    }

    fn cookies(&self, url: &reqwest::Url) -> Option<HeaderValue> {
        if !is_chatgpt_cookie_url(url) {
            return None;
        }
        self.jar.cookies(url).and_then(only_cloudflare_cookies)
    }
}

pub(crate) fn apply(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    builder.cookie_provider(Arc::clone(&STORE))
}

fn is_chatgpt_cookie_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https" && url.host_str().is_some_and(is_allowed_chatgpt_host)
}

fn is_allowed_chatgpt_host(host: &str) -> bool {
    const EXACT_HOSTS: &[&str] = &["chatgpt.com", "chat.openai.com", "chatgpt-staging.com"];
    const SUBDOMAIN_SUFFIXES: &[&str] = &[".chatgpt.com", ".chatgpt-staging.com"];
    EXACT_HOSTS.contains(&host)
        || SUBDOMAIN_SUFFIXES
            .iter()
            .any(|suffix| host.ends_with(suffix))
}

fn is_allowed_cloudflare_set_cookie_header(header: &HeaderValue) -> bool {
    header
        .to_str()
        .ok()
        .and_then(set_cookie_name)
        .is_some_and(is_allowed_cloudflare_cookie_name)
}

fn set_cookie_name(header: &str) -> Option<&str> {
    let (name, _) = header.split_once('=')?;
    let name = name.trim();
    (!name.is_empty()).then_some(name)
}

fn only_cloudflare_cookies(header: HeaderValue) -> Option<HeaderValue> {
    let cookies = header
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|cookie| {
            let cookie = cookie.trim();
            let name = cookie.split_once('=')?.0.trim();
            is_allowed_cloudflare_cookie_name(name).then_some(cookie)
        })
        .collect::<Vec<_>>()
        .join("; ");
    (!cookies.is_empty())
        .then(|| HeaderValue::from_str(&cookies).ok())
        .flatten()
}

fn is_allowed_cloudflare_cookie_name(name: &str) -> bool {
    matches!(
        name,
        "__cf_bm"
            | "__cflb"
            | "__cfruid"
            | "__cfseq"
            | "__cfwaitingroom"
            | "_cfuvid"
            | "cf_clearance"
            | "cf_ob_info"
            | "cf_use_ob"
    ) || name.starts_with("cf_chl_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn stores_only_cloudflare_cookies_for_https_chatgpt_hosts() {
        let store = ChatGptCloudflareCookieStore::default();
        let url =
            reqwest::Url::parse("https://chatgpt.com/backend-api/codex/responses").expect("URL");
        let cloudflare = HeaderValue::from_static("__cf_bm=beam; Path=/; Secure; HttpOnly");
        let account = HeaderValue::from_static("session=must-not-store; Path=/; Secure");
        store.set_cookies(&mut [&cloudflare, &account].into_iter(), &url);
        assert_eq!(
            store
                .cookies(&url)
                .and_then(|value| value.to_str().ok().map(str::to_string))
                .as_deref(),
            Some("__cf_bm=beam")
        );
        let api = reqwest::Url::parse("https://api.openai.com/v1/responses").expect("URL");
        assert_eq!(store.cookies(&api), None);
    }

    #[test]
    fn rejects_plain_http_and_suffix_trick_hosts() {
        for raw in [
            "http://chatgpt.com/backend-api/codex/responses",
            "https://chatgpt.com.evil.example/responses",
            "https://evilchatgpt.com/responses",
        ] {
            let url = reqwest::Url::parse(raw).expect("URL");
            assert!(!is_chatgpt_cookie_url(&url));
        }
    }
}
