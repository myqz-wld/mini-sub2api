mod build_info;
mod cli;
mod codex_auth_import;
mod error;
mod http_body;
mod http_client;
mod oauth;
mod oauth_login;
mod request_normalizer;
mod server;
#[cfg(test)]
mod test_support;
mod upstream_request;
mod vault;

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
