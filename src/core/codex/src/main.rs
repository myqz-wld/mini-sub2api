mod ascii_json;
mod build_info;
mod cli;
mod cloudflare_cookies;
mod codex_auth_import;
mod codex_user_agent;
mod error;
mod fingerprint;
mod fingerprint_projection;
mod http_body;
mod http_client;
mod inference_fingerprint;
mod oauth;
mod oauth_login;
mod request_defaults;
mod request_identity;
mod request_normalizer;
mod request_profile;
mod request_pseudonym;
mod response_item_metadata;
mod response_stream;
mod responses_lite;
mod responses_websocket;
mod responses_websocket_deferred;
mod responses_websocket_emulation;
mod responses_websocket_http;
mod responses_websocket_inject;
mod responses_websocket_prewarm;
mod responses_websocket_projection;
mod responses_websocket_reuse;
mod responses_websocket_state;
mod server;
mod terminal_detection;
#[cfg(test)]
mod test_support;
mod transport_registry;
mod upstream_request;
mod vault;
mod vault_io;
mod websocket_connector;
mod websocket_delivery;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    cli::Cli::parse().run().await
}
