use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use futures_util::future::join_all;
use serde::Serialize;

use crate::client_connect::Client;

#[derive(Debug)]
pub struct InstallOutcome {
    pub client: Client,
    pub version: String,
    pub installed_now: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolStatus {
    pub client: String,
    pub label: String,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub installed_but_broken: bool,
    pub error: Option<String>,
    pub env_type: String,
    pub wsl_distro: Option<String>,
    pub installations: Vec<ToolInstallation>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolInstallation {
    pub source: String,
    pub path: String,
    pub version: Option<String>,
    pub runnable: bool,
    pub is_path_default: bool,
}

pub async fn inspect_clients(clients: &[Client], home: &Path) -> Vec<ToolStatus> {
    let search_path = effective_path(home);
    join_all(clients.iter().map(|&client| {
        let search_path = &search_path;
        async move {
            let (checked, latest_version) = tokio::join!(
                enumerate_installations(client, home, search_path),
                fetch_latest_version(client),
            );
            let (current_version, installed_but_broken, error) = match checked
                .iter()
                .find(|(installation, _)| installation.is_path_default)
            {
                Some((installation, error)) => (
                    installation.version.clone(),
                    !installation.runnable,
                    error.clone(),
                ),
                None => (None, false, Some("未安装".to_string())),
            };
            ToolStatus {
                client: client.command().to_string(),
                label: client.label().to_string(),
                current_version,
                latest_version,
                installed_but_broken,
                error,
                env_type: environment_type().to_string(),
                wsl_distro: None,
                installations: checked
                    .into_iter()
                    .map(|(installation, _)| installation)
                    .collect(),
            }
        }
    }))
    .await
}

pub async fn install_client(client: Client, home: &Path) -> Result<ToolStatus> {
    let search_path = effective_path(home);
    install(client, home, &search_path).await?;
    inspect_one(client, home).await
}

pub async fn update_client(client: Client, home: &Path) -> Result<ToolStatus> {
    let search_path = effective_path(home);
    let before = inspect_one(client, home).await?;
    let executable = find_executable(client.command(), home, &search_path)
        .ok_or_else(|| anyhow!("{} 尚未安装", client.label()))?;
    if let Some(arguments) = official_update_arguments(client) {
        let output = Command::new(&executable)
            .args(arguments)
            .env("HOME", home)
            .env("PATH", &search_path)
            .output()
            .with_context(|| format!("无法启动 {} 官方更新", client.label()))?;
        if output.status.success() {
            return verify_updated(client, before, inspect_one(client, home).await?);
        }
    }
    install(client, home, &search_path).await?;
    verify_updated(client, before, inspect_one(client, home).await?)
}

fn verify_updated(client: Client, before: ToolStatus, after: ToolStatus) -> Result<ToolStatus> {
    if before.current_version == after.current_version
        && before.latest_version.is_some()
        && before.latest_version != after.current_version
    {
        bail!(
            "{} 更新命令已完成，但实际版本仍为 {}（最新版本：{}）。可能存在多个安装位置",
            client.label(),
            after.current_version.as_deref().unwrap_or("未知"),
            before.latest_version.as_deref().unwrap_or("未知")
        );
    }
    Ok(after)
}

async fn inspect_one(client: Client, home: &Path) -> Result<ToolStatus> {
    let status = inspect_clients(&[client], home)
        .await
        .into_iter()
        .next()
        .context("客户端检测没有返回结果")?;
    if status.current_version.is_none() {
        bail!(
            "{} 操作已经结束，但仍无法运行：{}",
            client.label(),
            status.error.as_deref().unwrap_or("未知错误")
        );
    }
    Ok(status)
}

pub async fn ensure_clients(clients: &[Client], home: &Path) -> Vec<Result<InstallOutcome>> {
    let mut outcomes = Vec::with_capacity(clients.len());
    for client in clients {
        outcomes.push(ensure_client(*client, home).await);
    }
    outcomes
}

async fn ensure_client(client: Client, home: &Path) -> Result<InstallOutcome> {
    let search_path = effective_path(home);
    if let Some((_, version)) = probe(client, home, &search_path) {
        return Ok(InstallOutcome {
            client,
            version,
            installed_now: false,
        });
    }

    install(client, home, &search_path).await?;
    let refreshed_path = effective_path(home);
    let (_, version) = probe(client, home, &refreshed_path).ok_or_else(|| {
        anyhow!(
            "{} 安装命令已完成，但仍无法运行。请检查 Node.js 版本与 PATH 后重试",
            client.label()
        )
    })?;
    Ok(InstallOutcome {
        client,
        version,
        installed_now: true,
    })
}

async fn install(client: Client, home: &Path, search_path: &str) -> Result<()> {
    #[cfg(not(target_os = "windows"))]
    if let Some(url) = official_installer_url(client) {
        if install_shell_script(client, url, home, search_path)
            .await
            .is_ok()
        {
            return Ok(());
        }
    }
    #[cfg(target_os = "windows")]
    if matches!(client, Client::Grok | Client::Hermes) {
        return install_windows_script(client, home, search_path);
    }
    install_with_npm(client, home, search_path)
}

#[cfg(target_os = "windows")]
fn install_windows_script(client: Client, home: &Path, search_path: &str) -> Result<()> {
    let url = match client {
        Client::Grok => "https://x.ai/cli/install.ps1",
        Client::Hermes => {
            "https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.ps1"
        }
        _ => return Err(anyhow!("{} 没有 PowerShell 安装器", client.label())),
    };
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!("irm {url} | iex"),
        ])
        .env("HOME", home)
        .env("PATH", search_path)
        .output()
        .with_context(|| format!("无法启动 {} PowerShell 安装器", client.label()))?;
    ensure_success(output, &format!("{} 官方安装", client.label()))
}

#[cfg(not(target_os = "windows"))]
fn official_installer_url(client: Client) -> Option<&'static str> {
    match client {
        Client::Claude => Some("https://claude.ai/install.sh"),
        Client::Grok => Some("https://x.ai/cli/install.sh"),
        Client::OpenCode => Some("https://opencode.ai/install"),
        Client::Hermes => Some(
            "https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.sh",
        ),
        _ => None,
    }
}

fn official_update_arguments(client: Client) -> Option<&'static [&'static str]> {
    match client {
        Client::Claude | Client::Codex | Client::Grok | Client::Hermes => Some(&["update"]),
        Client::OpenCode => Some(&["upgrade"]),
        Client::OpenClaw => Some(&["update", "--yes"]),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
async fn install_shell_script(
    client: Client,
    url: &str,
    home: &Path,
    search_path: &str,
) -> Result<()> {
    let response = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .with_context(|| format!("无法下载 {} 官方安装器", client.label()))?
        .error_for_status()
        .with_context(|| format!("{} 官方安装器下载失败", client.label()))?;
    let script = response
        .bytes()
        .await
        .context("无法读取 Claude Code 安装器")?;
    let temp_dir = home.join(".junohub/tmp");
    fs::create_dir_all(&temp_dir)?;
    let script_path = temp_dir.join(format!(
        "{}-install-{}.sh",
        client.command(),
        uuid::Uuid::new_v4()
    ));
    fs::write(&script_path, script).context("无法保存 Claude Code 安装器")?;
    let output = Command::new("bash")
        .arg(&script_path)
        .env("HOME", home)
        .env("PATH", search_path)
        .output()
        .with_context(|| format!("无法启动 {} 官方安装器", client.label()));
    let _ = fs::remove_file(&script_path);
    ensure_success(output?, &format!("{} 官方安装", client.label()))
}

fn install_with_npm(client: Client, home: &Path, search_path: &str) -> Result<()> {
    let package = client
        .npm_package()
        .ok_or_else(|| anyhow!("{} 需要使用官方安装器", client.label()))?;
    let npm = find_executable("npm", home, search_path).ok_or_else(|| {
        anyhow!(
            "安装 {} 需要 Node.js 与 npm。请先安装 Node.js 18 或更高版本",
            client.label()
        )
    })?;
    let output = Command::new(&npm)
        .args(["install", "-g", package])
        .env("HOME", home)
        .env("PATH", search_path)
        .output()
        .with_context(|| format!("无法启动 npm 安装 {}", client.label()))?;
    ensure_success(output, &format!("{} 安装", client.label()))
}

fn probe(client: Client, home: &Path, search_path: &str) -> Option<(PathBuf, String)> {
    let executable = find_executable(client.command(), home, search_path)?;
    let version = version_at(client, &executable, home, search_path).ok()?;
    Some((executable, version))
}

fn version_at(client: Client, executable: &Path, home: &Path, search_path: &str) -> Result<String> {
    let output = Command::new(executable)
        .arg("--version")
        .env("HOME", home)
        .env("PATH", search_path)
        .output()
        .with_context(|| format!("无法运行 {}", client.label()))?;
    version_from_output(output)
}

async fn inspect_version_at(
    client: Client,
    executable: &Path,
    home: &Path,
    search_path: &str,
    timeout: Duration,
) -> Result<String> {
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("--version")
        .env("HOME", home)
        .env("PATH", search_path)
        .kill_on_drop(true);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .with_context(|| format!("{} 版本检测超时", client.label()))?
        .with_context(|| format!("无法运行 {}", client.label()))?;
    version_from_output(output)
}

fn version_from_output(output: Output) -> Result<String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        if detail.is_empty() {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(signal) = output.status.signal() {
                    bail!("版本进程被系统终止（信号 {signal}）");
                }
            }
            bail!("版本检测失败（{}）", output.status);
        }
        bail!("{detail}");
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let raw = if stdout.is_empty() { stderr } else { stdout };
    if raw.is_empty() {
        bail!("版本检测没有返回内容");
    }
    Ok(extract_version(&raw))
}

fn extract_version(raw: &str) -> String {
    raw.split_whitespace()
        .map(|part| {
            part.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric() && !matches!(character, '.' | '-' | '+')
            })
        })
        .find(|part| {
            let candidate = part.trim_start_matches('v');
            candidate
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_digit())
                && candidate.contains('.')
        })
        .unwrap_or(raw.trim())
        .trim_start_matches('v')
        .to_string()
}

async fn fetch_latest_version(client: Client) -> Option<String> {
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(4))
        .build()
        .ok()?;
    if client == Client::Hermes {
        let response = http
            .get("https://api.github.com/repos/NousResearch/hermes-agent/releases/latest")
            .header("User-Agent", "JunoHub")
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        return response
            .json::<serde_json::Value>()
            .await
            .ok()?
            .get("tag_name")
            .and_then(serde_json::Value::as_str)
            .map(|value| value.trim_start_matches('v').to_string());
    }
    let package = client.npm_package()?.trim_end_matches("@latest");
    let encoded = package.replace('/', "%2F");
    let response = http
        .get(format!("https://registry.npmjs.org/{encoded}/latest"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    response
        .json::<serde_json::Value>()
        .await
        .ok()?
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn ensure_success(output: Output, label: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    let tail = detail
        .lines()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    if tail.is_empty() {
        bail!("{label}失败，退出码：{:?}", output.status.code());
    }
    bail!("{label}失败：{tail}")
}

fn effective_path(home: &Path) -> String {
    let mut paths = env::var_os("PATH")
        .map(|value| env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();

    #[cfg(not(target_os = "windows"))]
    {
        let shell = env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
        if let Ok(output) = Command::new(shell)
            .args(["-lc", "printf %s \"$PATH\""])
            .output()
        {
            if output.status.success() {
                let login_path = String::from_utf8_lossy(&output.stdout).into_owned();
                paths.extend(env::split_paths(&login_path));
            }
        }
    }

    paths.extend(common_bin_dirs(home));
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    env::join_paths(paths)
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn find_executable(command: &str, home: &Path, search_path: &str) -> Option<PathBuf> {
    #[cfg(not(target_os = "windows"))]
    let names = vec![command.to_string()];
    #[cfg(target_os = "windows")]
    let names = vec![
        command.to_string(),
        format!("{command}.cmd"),
        format!("{command}.exe"),
    ];

    let mut directories = env::split_paths(search_path).collect::<Vec<_>>();
    directories.extend(common_bin_dirs(home));
    for directory in directories {
        for name in &names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

async fn enumerate_installations(
    client: Client,
    home: &Path,
    search_path: &str,
) -> Vec<(ToolInstallation, Option<String>)> {
    #[cfg(not(target_os = "windows"))]
    let names = vec![client.command().to_string()];
    #[cfg(target_os = "windows")]
    let names = vec![
        client.command().to_string(),
        format!("{}.cmd", client.command()),
        format!("{}.exe", client.command()),
    ];
    let default = find_executable(client.command(), home, search_path)
        .and_then(|path| path.canonicalize().ok().or(Some(path)));
    let mut directories = env::split_paths(search_path).collect::<Vec<_>>();
    directories.extend(common_bin_dirs(home));
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for directory in directories {
        for name in &names {
            let candidate = directory.join(name);
            if !candidate.is_file() {
                continue;
            }
            let canonical = candidate.canonicalize().unwrap_or(candidate);
            if !seen.insert(canonical.clone()) {
                continue;
            }
            candidates.push(canonical);
        }
    }
    let default = default.as_ref();
    join_all(candidates.iter().map(|canonical| async move {
        let result =
            inspect_version_at(client, canonical, home, search_path, Duration::from_secs(5)).await;
        let version = result.as_ref().ok().cloned();
        (
            ToolInstallation {
                source: installation_source(canonical),
                path: canonical.display().to_string(),
                runnable: version.is_some(),
                version,
                is_path_default: default == Some(canonical),
            },
            result.err().map(|error| error.to_string()),
        )
    }))
    .await
}

fn installation_source(path: &Path) -> String {
    let value = path.to_string_lossy().to_ascii_lowercase();
    if value.contains(".volta") {
        "Volta"
    } else if value.contains(".nvm") {
        "nvm"
    } else if value.contains("pnpm") {
        "pnpm"
    } else if value.contains("homebrew") || value.starts_with("/opt/homebrew") {
        "Homebrew"
    } else if value.contains("npm")
        || value.ends_with("/bin/claude")
        || value.ends_with("/bin/codex")
    {
        "npm"
    } else {
        "PATH"
    }
    .to_string()
}

fn environment_type() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

fn common_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let mut directories = vec![
        home.join(".local/bin"),
        home.join(".claude/local"),
        home.join(".npm-global/bin"),
        home.join(".npm-packages/bin"),
        home.join(".volta/bin"),
        home.join(".local/share/pnpm"),
        home.join("Library/pnpm"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ];
    let nvm_versions = home.join(".nvm/versions/node");
    if let Ok(entries) = fs::read_dir(nvm_versions) {
        let mut bins = entries
            .flatten()
            .filter_map(|entry| {
                entry
                    .file_type()
                    .ok()
                    .filter(|kind| kind.is_dir())
                    .map(|_| entry.path().join("bin"))
            })
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        bins.sort_by_key(|path| std::cmp::Reverse(node_version_key(path)));
        directories.extend(bins);
    }
    #[cfg(not(target_os = "windows"))]
    return directories;
    #[cfg(target_os = "windows")]
    {
        let mut directories = directories;
        if let Some(app_data) = env::var_os("APPDATA") {
            directories.push(PathBuf::from(app_data).join("npm"));
        }
        directories
    }
}

fn node_version_key(path: &Path) -> Vec<u64> {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse().unwrap_or_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn fake_client(home: &Path, name: &str, version: &str) {
        use std::os::unix::fs::PermissionsExt;

        let bin = home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\necho '{version}'\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn executable_search_includes_user_local_bin() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let name = if cfg!(target_os = "windows") {
            "codex.cmd"
        } else {
            "codex"
        };
        fs::write(bin.join(name), b"").unwrap();
        assert_eq!(
            find_executable("codex", home.path(), ""),
            Some(bin.join(name))
        );
    }

    #[test]
    fn executable_search_includes_newest_nvm_node() {
        let home = tempfile::tempdir().unwrap();
        let older = home.path().join(".nvm/versions/node/v9.9.0/bin");
        let newer = home.path().join(".nvm/versions/node/v24.1.0/bin");
        fs::create_dir_all(&older).unwrap();
        fs::create_dir_all(&newer).unwrap();
        fs::write(older.join("pi"), b"").unwrap();
        fs::write(newer.join("pi"), b"").unwrap();
        assert_eq!(
            find_executable("pi", home.path(), ""),
            Some(newer.join("pi"))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installed_clients_are_verified_without_running_installers() {
        let home = tempfile::tempdir().unwrap();
        fake_client(home.path(), "claude", "2.1.0");
        fake_client(home.path(), "codex", "0.99.0");
        fake_client(home.path(), "gemini", "1.2.3");
        let clients = [Client::Claude, Client::Codex, Client::Gemini];
        let outcomes = ensure_clients(&clients, home.path()).await;
        assert!(outcomes.iter().all(|outcome| outcome.is_ok()));
        assert!(outcomes
            .into_iter()
            .all(|outcome| !outcome.unwrap().installed_now));
    }

    #[test]
    fn extracts_semver_from_cli_output() {
        assert_eq!(extract_version("claude 2.1.156 (native)"), "2.1.156");
        assert_eq!(extract_version("codex-cli 0.182.0"), "0.182.0");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn version_inspection_times_out_for_a_stuck_executable() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let executable = home.path().join("stuck-client");
        fs::write(&executable, "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let started = std::time::Instant::now();
        let result = inspect_version_at(
            Client::Codex,
            &executable,
            home.path(),
            "/usr/bin:/bin",
            Duration::from_millis(100),
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("版本检测超时"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn version_inspection_reports_a_failing_executable_without_hiding_the_error() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let executable = home.path().join("broken-client");
        fs::write(
            &executable,
            "#!/bin/sh\necho 'missing runtime' >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        // 这条断言的是“失败原因不被吞掉”，不是超时。
        // 超时给足 10 秒：跑全量测试时机器负载高，1 秒会在 fork/exec 阶段偶发超时，
        // 让整套测试无意义地变红。
        let result = inspect_version_at(
            Client::Codex,
            &executable,
            home.path(),
            "/usr/bin:/bin",
            Duration::from_secs(10),
        )
        .await;
        assert_eq!(result.unwrap_err().to_string(), "missing runtime");
    }

    #[test]
    fn unchanged_outdated_version_is_not_reported_as_updated() {
        let status = ToolStatus {
            client: "codex".into(),
            label: "Codex".into(),
            current_version: Some("0.144.0".into()),
            latest_version: Some("0.154.0".into()),
            installed_but_broken: false,
            error: None,
            env_type: environment_type().into(),
            wsl_distro: None,
            installations: Vec::new(),
        };
        assert!(verify_updated(Client::Codex, status.clone(), status).is_err());
    }
}
