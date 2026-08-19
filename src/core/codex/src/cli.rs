use crate::http_client::apply_loopback_proxy_policy;
use crate::oauth;
use crate::oauth::LoginFlow;
use crate::oauth::OAuthConfig;
use crate::server;
use crate::vault::DEFAULT_CODEX_RESPONSES_URL;
use crate::vault::DEFAULT_OPENAI_RESPONSES_URL;
use crate::vault::RemovalKind;
use crate::vault::Vault;
use anyhow::Context;
use anyhow::Result;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use reqwest::Client;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

#[derive(Debug, Parser)]
#[command(name = "mini-sub2api-core-codex", version)]
pub struct Cli {
    #[arg(long)]
    check_installed: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve(ServeArgs),
    Credential(CredentialArgs),
}

#[derive(Debug, Args)]
struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1:0")]
    listen: String,
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct CredentialArgs {
    #[command(subcommand)]
    command: CredentialCommand,
}

#[derive(Debug, Subcommand)]
enum CredentialCommand {
    Login(LoginArgs),
    AddApiKey(AddApiKeyArgs),
    Inspect(AccountArgs),
    Revoke(AccountArgs),
    Remove(AccountArgs),
}

#[derive(Debug, Args)]
struct LoginArgs {
    #[arg(long)]
    state_dir: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = LoginFlow::Device)]
    flow: LoginFlow,
    #[arg(long, default_value = oauth::DEFAULT_ISSUER)]
    issuer: String,
    #[arg(long, default_value = oauth::DEFAULT_CLIENT_ID)]
    client_id: String,
    #[arg(long, default_value = DEFAULT_CODEX_RESPONSES_URL)]
    upstream_url: String,
}

#[derive(Debug, Args)]
struct AddApiKeyArgs {
    #[arg(long)]
    state_dir: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_OPENAI_RESPONSES_URL)]
    upstream_url: String,
    #[arg(long, default_value_t = false)]
    secret_stdin: bool,
}

#[derive(Debug, Args)]
struct AccountArgs {
    #[arg(long)]
    state_dir: Option<PathBuf>,
    account_ref: String,
}

impl Cli {
    pub async fn run(self) -> Result<()> {
        if self.check_installed {
            anyhow::ensure!(
                self.command.is_none(),
                "--check-installed cannot be combined with a command"
            );
            anyhow::ensure!(
                crate::build_info::check_installed()?,
                "installed artifact check failed"
            );
            return Ok(());
        }
        let command = self.command.context("a command is required")?;
        match command {
            Command::Serve(args) => {
                let listen = server::parse_internal_listen(&args.listen)?;
                server::run(listen, state_dir(args.state_dir)).await
            }
            Command::Credential(args) => args.command.run().await,
        }
    }
}

impl CredentialCommand {
    async fn run(self) -> Result<()> {
        match self {
            Self::Login(args) => {
                let vault = Vault::open(state_dir(args.state_dir))?;
                let metadata = oauth::login(
                    &vault,
                    args.flow,
                    OAuthConfig {
                        issuer: args.issuer,
                        client_id: args.client_id,
                        upstream_url: args.upstream_url,
                    },
                )
                .await?;
                print_json(&metadata)
            }
            Self::AddApiKey(args) => {
                anyhow::ensure!(args.secret_stdin, "--secret-stdin is required");
                validate_url(&args.upstream_url)?;
                let api_key = read_secret_stdin().await?;
                let vault = Vault::open(state_dir(args.state_dir))?;
                let metadata = vault.create_api_key(api_key, args.upstream_url).await?;
                print_json(&metadata)
            }
            Self::Inspect(args) => {
                let vault = Vault::open(state_dir(args.state_dir))?;
                let locked = vault.lock_record(&args.account_ref).await?;
                print_json(&locked.record.metadata())
            }
            Self::Revoke(args) => {
                let vault = Vault::open(state_dir(args.state_dir))?;
                if let Some(receipt) = vault.removal_receipt(&args.account_ref).await? {
                    anyhow::ensure!(
                        receipt.kind == RemovalKind::OAuthRevoked,
                        "credential was removed without an upstream OAuth revoke"
                    );
                    vault
                        .remove(&args.account_ref, RemovalKind::OAuthRevoked)
                        .await?;
                    return print_json(&serde_json::json!({
                        "accountRef": args.account_ref,
                        "revoked": true,
                        "recovered": true
                    }));
                }
                let mut locked = match vault.lock_record(&args.account_ref).await {
                    Ok(locked) => locked,
                    Err(lock_error) => {
                        if let Some(receipt) = vault.removal_receipt(&args.account_ref).await?
                            && receipt.kind == RemovalKind::OAuthRevoked
                        {
                            vault
                                .remove(&args.account_ref, RemovalKind::OAuthRevoked)
                                .await?;
                            return print_json(&serde_json::json!({
                                "accountRef": args.account_ref,
                                "revoked": true,
                                "recovered": true
                            }));
                        }
                        return Err(lock_error);
                    }
                };
                let issuer = match &locked.record.material {
                    crate::vault::CredentialMaterial::CodexOAuth { issuer, .. } => issuer.clone(),
                    crate::vault::CredentialMaterial::OpenAiApiKey { .. } => {
                        anyhow::bail!("credential is not OAuth-backed")
                    }
                };
                let client = apply_loopback_proxy_policy(
                    Client::builder()
                        .connect_timeout(Duration::from_secs(15))
                        .redirect(reqwest::redirect::Policy::none()),
                    &issuer,
                )
                .build()
                .context("building revoke client")?;
                oauth::revoke(&mut locked, &client).await?;
                locked.complete_removal(RemovalKind::OAuthRevoked).await?;
                print_json(&serde_json::json!({
                    "accountRef": args.account_ref,
                    "revoked": true
                }))
            }
            Self::Remove(args) => {
                let vault = Vault::open(state_dir(args.state_dir))?;
                vault
                    .remove(&args.account_ref, RemovalKind::ServiceOnly)
                    .await?;
                print_json(&serde_json::json!({
                    "accountRef": args.account_ref,
                    "removed": true
                }))
            }
        }
    }
}

fn state_dir(configured: Option<PathBuf>) -> PathBuf {
    configured.unwrap_or_else(oauth::default_state_dir)
}

async fn read_secret_stdin() -> Result<String> {
    tokio::task::spawn_blocking(|| {
        const MAX_SECRET_BYTES: u64 = 16 * 1024;
        let mut secret = String::new();
        std::io::stdin()
            .lock()
            .take(MAX_SECRET_BYTES + 1)
            .read_to_string(&mut secret)
            .context("reading secret from stdin")?;
        anyhow::ensure!(
            secret.len() as u64 <= MAX_SECRET_BYTES,
            "secret is too large"
        );
        let secret = secret.trim().to_string();
        anyhow::ensure!(!secret.is_empty(), "secret is empty");
        Ok(secret)
    })
    .await
    .context("secret read task failed")?
}

fn validate_url(raw: &str) -> Result<()> {
    let url = Url::parse(raw).context("parsing upstream URL")?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https"),
        "unsupported URL scheme"
    );
    anyhow::ensure!(url.host_str().is_some(), "upstream URL has no host");
    let loopback = match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(_)) | None => false,
    };
    anyhow::ensure!(
        url.scheme() == "https" || loopback,
        "plain HTTP is allowed only for loopback compatibility endpoints"
    );
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "upstream URL must not contain user info"
    );
    Ok(())
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}
