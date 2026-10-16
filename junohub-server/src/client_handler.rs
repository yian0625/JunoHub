use std::{env, fs, path::Path, process::Command};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Serialize;
use url::Url;

pub const LOCAL_HELPER_PORT: u16 = 17372;

pub fn pairing_url_from_deep_link(value: &str) -> Result<String> {
    let url = Url::parse(value).context("JunoHub 集成链接无效")?;
    if url.scheme() != "junohub" || url.host_str() != Some("connect") {
        bail!("不支持的 JunoHub 集成链接");
    }
    url.query_pairs()
        .find(|(key, _)| key == "pair_url")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty())
        .context("JunoHub 集成链接缺少一次性接入地址")
}

pub fn deep_link_host(value: &str) -> Result<String> {
    let url = validated_deep_link(value)?;
    Ok(url.host_str().unwrap_or_default().to_string())
}

pub fn deep_link_param(value: &str, name: &str) -> Result<String> {
    let url = validated_deep_link(value)?;
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty())
        .with_context(|| format!("JunoHub 集成链接缺少 {name}"))
}

fn validated_deep_link(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("JunoHub 集成链接无效")?;
    if url.scheme() != "junohub" || url.host_str().is_none() {
        bail!("不支持的 JunoHub 集成链接");
    }
    Ok(url)
}

pub fn open_result<T: Serialize>(return_url: &str, result: &T) -> Result<()> {
    let mut url = Url::parse(return_url).context("返回地址无效")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("返回地址必须使用 HTTP 或 HTTPS");
    }
    let json = serde_json::to_vec(result).context("无法编码本机检测结果")?;
    url.set_fragment(Some(&format!(
        "junohub-integration={}",
        URL_SAFE_NO_PAD.encode(json)
    )));
    open_url(url.as_str())
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> Result<()> {
    let output = Command::new("open").arg(url).output()?;
    ensure_success(output, "打开 JunoHub 结果页")
}

#[cfg(target_os = "linux")]
fn open_url(url: &str) -> Result<()> {
    let output = Command::new("xdg-open").arg(url).output()?;
    ensure_success(output, "打开 JunoHub 结果页")
}

#[cfg(target_os = "windows")]
fn open_url(url: &str) -> Result<()> {
    let output = Command::new("cmd")
        .args(["/C", "start", "", url])
        .output()?;
    ensure_success(output, "打开 JunoHub 结果页")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn open_url(_url: &str) -> Result<()> {
    bail!("当前操作系统无法打开结果页")
}

pub fn install(home: &Path) -> Result<String> {
    let current = env::current_exe().context("无法定位 JunoHub 本地组件")?;
    let target_dir = home.join(".junohub/bin");
    fs::create_dir_all(&target_dir)?;
    let target = target_dir.join(executable_name());
    if current != target {
        fs::copy(&current, &target)
            .with_context(|| format!("无法安装本地组件到 {}", target.display()))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
    }

    install_platform_handler(home, &target)?;
    install_local_helper(home, &target)?;
    Ok(target.display().to_string())
}

#[cfg(target_os = "macos")]
fn install_local_helper(home: &Path, executable: &Path) -> Result<()> {
    let launch_agents = home.join("Library/LaunchAgents");
    fs::create_dir_all(&launch_agents)?;
    let plist = launch_agents.join("com.junohub.connect-helper.plist");
    let log_dir = home.join(".junohub");
    fs::create_dir_all(&log_dir)?;
    let executable = xml_escape(&executable.to_string_lossy());
    let log = xml_escape(&log_dir.join("connect-helper.log").to_string_lossy());
    fs::write(
        &plist,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.junohub.connect-helper</string>
<key>ProgramArguments</key><array><string>{executable}</string><string>serve</string><string>--port</string><string>{LOCAL_HELPER_PORT}</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
<key>StandardOutPath</key><string>{log}</string>
<key>StandardErrorPath</key><string>{log}</string>
</dict></plist>
"#,
        ),
    )?;
    let uid = String::from_utf8(
        Command::new("id")
            .arg("-u")
            .output()
            .context("无法读取当前用户编号")?
            .stdout,
    )?
    .trim()
    .to_string();
    let domain = format!("gui/{uid}");
    let service = format!("{domain}/com.junohub.connect-helper");
    let _ = Command::new("launchctl")
        .args(["bootout", &service])
        .output();
    let output = Command::new("launchctl")
        .arg("bootstrap")
        .arg(&domain)
        .arg(&plist)
        .output()
        .context("无法注册本机检测服务")?;
    ensure_success(output, "注册本机检测服务")?;
    let output = Command::new("launchctl")
        .args(["kickstart", "-k", &service])
        .output()
        .context("无法启动本机检测服务")?;
    ensure_success(output, "启动本机检测服务")
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "linux")]
fn install_local_helper(home: &Path, executable: &Path) -> Result<()> {
    let directory = home.join(".config/systemd/user");
    fs::create_dir_all(&directory)?;
    let service = directory.join("junohub-connect-helper.service");
    fs::write(
        service,
        format!(
            "[Unit]\nDescription=JunoHub local inspection helper\n\n[Service]\nExecStart={} serve --port {}\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
            executable.display(),
            LOCAL_HELPER_PORT
        ),
    )?;
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();
    let output = Command::new("systemctl")
        .args([
            "--user",
            "enable",
            "--now",
            "junohub-connect-helper.service",
        ])
        .output()
        .context("无法启动本机检测服务")?;
    ensure_success(output, "启动本机检测服务")
}

#[cfg(target_os = "windows")]
fn install_local_helper(_home: &Path, executable: &Path) -> Result<()> {
    let command = format!(
        "\"{}\" serve --port {}",
        executable.display(),
        LOCAL_HELPER_PORT
    );
    run_reg(&[
        "add",
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
        "/v",
        "JunoHubConnectHelper",
        "/d",
        &command,
        "/f",
    ])?;
    let _ = Command::new(executable)
        .args(["serve", "--port", &LOCAL_HELPER_PORT.to_string()])
        .spawn();
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn install_local_helper(_home: &Path, _executable: &Path) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_platform_handler(home: &Path, executable: &Path) -> Result<()> {
    let applications = home.join("Applications");
    fs::create_dir_all(&applications)?;
    let app = applications.join("JunoHub Connect.app");
    if app.exists() {
        fs::remove_dir_all(&app).context("无法更新旧版 JunoHub Connect.app")?;
    }
    let path = executable
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let script = format!(
        "on open location targetUrl\n  do shell script quoted form of \"{path}\" & \" \" & quoted form of targetUrl & \" >>\" & quoted form of \"{path}.log\" & \" 2>&1 &\"\nend open location"
    );
    let output = Command::new("osacompile")
        .args(["-o"])
        .arg(&app)
        .args(["-e", &script])
        .output()
        .context("无法创建 JunoHub URL 处理器")?;
    ensure_success(output, "注册 JunoHub URL 处理器")?;
    let plist = app.join("Contents/Info.plist");
    for command in [
        "Add :CFBundleURLTypes array",
        "Add :CFBundleURLTypes:0 dict",
        "Add :CFBundleURLTypes:0:CFBundleURLName string com.junohub.connect",
        "Add :CFBundleURLTypes:0:CFBundleURLSchemes array",
        "Add :CFBundleURLTypes:0:CFBundleURLSchemes:0 string junohub",
    ] {
        let output = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", command])
            .arg(&plist)
            .output()
            .context("无法写入 JunoHub URL Scheme")?;
        ensure_success(output, "写入 JunoHub URL Scheme")?;
    }
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    if Path::new(lsregister).exists() {
        let output = Command::new(lsregister).arg("-f").arg(&app).output()?;
        ensure_success(output, "刷新 macOS URL 处理器")?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_platform_handler(home: &Path, executable: &Path) -> Result<()> {
    let directory = home.join(".local/share/applications");
    fs::create_dir_all(&directory)?;
    let desktop = directory.join("junohub-connect.desktop");
    let escaped = executable.to_string_lossy().replace(' ', "\\ ");
    fs::write(
        &desktop,
        format!(
            "[Desktop Entry]\nType=Application\nName=JunoHub Connect\nExec={escaped} %u\nTerminal=false\nNoDisplay=true\nMimeType=x-scheme-handler/junohub;\n"
        ),
    )?;
    let output = Command::new("xdg-mime")
        .args([
            "default",
            "junohub-connect.desktop",
            "x-scheme-handler/junohub",
        ])
        .output()
        .context("需要 xdg-mime 注册 JunoHub URL 处理器")?;
    ensure_success(output, "注册 JunoHub URL 处理器")
}

#[cfg(target_os = "windows")]
fn install_platform_handler(_home: &Path, executable: &Path) -> Result<()> {
    let key = r"HKCU\Software\Classes\junohub";
    run_reg(&["add", key, "/ve", "/d", "URL:JunoHub Connect", "/f"])?;
    run_reg(&["add", key, "/v", "URL Protocol", "/d", "", "/f"])?;
    let command_key = format!(r"{key}\shell\open\command");
    let command = format!("\"{}\" \"%1\"", executable.display());
    run_reg(&["add", &command_key, "/ve", "/d", &command, "/f"])
}

#[cfg(target_os = "windows")]
fn run_reg(args: &[&str]) -> Result<()> {
    let output = Command::new("reg.exe").args(args).output()?;
    ensure_success(output, "注册 JunoHub URL 处理器")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn install_platform_handler(_home: &Path, _executable: &Path) -> Result<()> {
    bail!("当前操作系统不支持一键打开 JunoHub 本地组件")
}

fn executable_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "junohub-connect.exe"
    } else {
        "junohub-connect"
    }
}

fn ensure_success(output: std::process::Output, label: &str) -> Result<()> {
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
    bail!("{label}失败：{detail}")
}

pub fn notify(message: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "display notification (item 1 of argv) with title \"JunoHub\"",
                "-e",
                "end run",
                "--",
                message,
            ])
            .status();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("notify-send")
            .args(["JunoHub", message])
            .status();
    }
    #[cfg(target_os = "windows")]
    {
        let safe = message.replace('\'', "''");
        let script = format!(
            "Add-Type -AssemblyName PresentationFramework; [System.Windows.MessageBox]::Show('{safe}', 'JunoHub')"
        );
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pair_url_from_deep_link() {
        let value = pairing_url_from_deep_link(
            "junohub://connect?pair_url=https%3A%2F%2Fhub.example%2Fclient-connect%2Fredeem%3Ftoken%3Dabc",
        )
        .unwrap();
        assert_eq!(value, "https://hub.example/client-connect/redeem?token=abc");
    }

    #[test]
    fn rejects_other_deep_links() {
        assert!(pairing_url_from_deep_link("junohub://other?pair_url=x").is_err());
    }
}
