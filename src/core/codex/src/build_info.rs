use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use std::io::Read;
use std::path::Path;
use std::process::Command;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildInfo {
    package_name: String,
    version: String,
    full_commit: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckResult {
    status: String,
    metadata_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    installed_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    installed_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin_main_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_dirty: Option<bool>,
    message: String,
}

pub fn check_installed() -> Result<bool> {
    let executable = std::env::current_exe().context("locating installed executable")?;
    let metadata_path = executable
        .parent()
        .context("installed executable has no parent directory")?
        .join("build-info.json");
    let metadata = match read_metadata(&metadata_path) {
        Ok(value) => value,
        Err(error) => {
            let result = CheckResult {
                status: if metadata_path.exists() {
                    "metadata_invalid".to_string()
                } else {
                    "metadata_missing".to_string()
                },
                metadata_path: metadata_path.display().to_string(),
                installed_version: None,
                installed_commit: None,
                source_commit: None,
                origin_main_commit: None,
                source_dirty: None,
                message: error.to_string(),
            };
            println!("{}", serde_json::to_string(&result)?);
            return Ok(false);
        }
    };
    let compiled_commit = option_env!("MINI_SUB2API_BUILD_COMMIT").unwrap_or("unknown");
    let mut result = CheckResult {
        status: String::new(),
        metadata_path: metadata_path.display().to_string(),
        installed_version: Some(metadata.version.clone()),
        installed_commit: Some(metadata.full_commit.clone()),
        source_commit: None,
        origin_main_commit: None,
        source_dirty: None,
        message: String::new(),
    };
    if metadata.package_name != "mini-sub2api"
        || metadata.version != env!("CARGO_PKG_VERSION")
        || metadata.full_commit != compiled_commit
    {
        result.status = "artifact_mismatch".to_string();
        result.message = "embedded build identity does not match build-info.json".to_string();
        println!("{}", serde_json::to_string(&result)?);
        return Ok(false);
    }
    let source = match read_git(&std::env::current_dir()?) {
        Ok(value) => value,
        Err(error) => {
            result.status = "source_unavailable".to_string();
            result.message = error.to_string();
            println!("{}", serde_json::to_string(&result)?);
            return Ok(false);
        }
    };
    result.source_commit = Some(source.commit.clone());
    result.origin_main_commit = source.origin_main_commit;
    result.source_dirty = Some(source.dirty);
    if source.commit != metadata.full_commit {
        result.status = "source_mismatch".to_string();
        result.message = "installed commit differs from the current source checkout".to_string();
        println!("{}", serde_json::to_string(&result)?);
        return Ok(false);
    }
    result.status = "ok".to_string();
    result.message = "installed metadata matches the current source checkout".to_string();
    println!("{}", serde_json::to_string(&result)?);
    Ok(true)
}

fn read_metadata(path: &Path) -> Result<BuildInfo> {
    const MAX_METADATA_BYTES: u64 = 64 * 1024;
    let file = std::fs::File::open(path).context("opening build-info.json")?;
    let mut bytes = Vec::new();
    std::io::Read::take(file, MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("reading build-info.json")?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_METADATA_BYTES,
        "build-info.json is too large"
    );
    serde_json::from_slice(&bytes).context("decoding build-info.json")
}

struct GitState {
    commit: String,
    origin_main_commit: Option<String>,
    dirty: bool,
}

fn read_git(directory: &Path) -> Result<GitState> {
    anyhow::ensure!(
        git_output(directory, &["rev-parse", "--is-inside-work-tree"])? == "true",
        "source directory is not a Git checkout"
    );
    let commit =
        git_output(directory, &["rev-parse", "HEAD"]).unwrap_or_else(|_| "unborn".to_string());
    let dirty = !git_output(directory, &["status", "--porcelain"])?.is_empty();
    let origin_main_commit = git_output(directory, &["rev-parse", "--verify", "origin/main"]).ok();
    Ok(GitState {
        commit,
        origin_main_commit,
        dirty,
    })
}

fn git_output(directory: &Path, arguments: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .context("running local git command")?;
    anyhow::ensure!(output.status.success(), "local git command failed");
    String::from_utf8(output.stdout)
        .context("decoding local git output")
        .map(|value| value.trim().to_string())
}
