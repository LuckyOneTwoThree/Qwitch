use std::time::Duration;

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const REPO: &str = "LuckyOneTwoThree/Qwitch";
const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/LuckyOneTwoThree/Qwitch/releases/latest";
const CHECKSUMS_ASSET_NAME: &str = "SHA256SUMS.txt";
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const MAX_DOWNLOAD_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_url: String,
    pub release_notes: String,
    pub published_at: Option<String>,
    pub asset_name: Option<String>,
    pub asset_url: Option<String>,
    pub asset_size: Option<u64>,
    pub checksums_url: Option<String>,
    /// windows_installer：下载安装包并拉起安装程序；macos_dmg：引导手动替换应用
    pub strategy: String,
    pub platform_label: String,
    pub can_auto_install: bool,
    pub hint: String,
}

fn build_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent(format!("Qwitch/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("网络客户端初始化失败：{e}"))
}

fn build_client_no_redirect(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent(format!("Qwitch/{}", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("网络客户端初始化失败：{e}"))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn build_download_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent(format!("Qwitch/{}", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("更新下载重定向次数过多")
            } else if let Err(msg) = validate_github_url(attempt.url().as_str()) {
                attempt.error(msg)
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| format!("网络客户端初始化失败：{e}"))
}

fn validate_official_repo_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|e| format!("更新地址无效：{e}"))?;
    if url.scheme() != "https" {
        return Err("更新地址必须使用 HTTPS".into());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "更新地址缺少主机名".to_string())?
        .to_ascii_lowercase();
    if host != "github.com" {
        return Err("更新地址必须是官方 github.com 仓库地址".into());
    }
    let expected_prefix = format!("/{}/", REPO.to_ascii_lowercase());
    if !url
        .path()
        .to_ascii_lowercase()
        .starts_with(&expected_prefix)
    {
        return Err("更新地址不属于官方 Qwitch 仓库".into());
    }
    Ok(url)
}

fn validate_github_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|e| format!("更新地址无效：{e}"))?;
    if url.scheme() != "https" {
        return Err("更新地址必须使用 HTTPS".into());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "更新地址缺少主机名".to_string())?
        .to_ascii_lowercase();
    if host == "github.com" {
        return validate_official_repo_url(raw);
    }
    let allowed = matches!(
        host.as_str(),
        "objects.githubusercontent.com"
            | "release-assets.githubusercontent.com"
            | "github-releases.githubusercontent.com"
    );
    if !allowed {
        return Err("更新地址不在 GitHub Release CDN 白名单内".into());
    }
    Ok(url)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn sanitize_asset_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.chars().any(|c| c.is_control())
    {
        return Err("更新资产名称非法".into());
    }
    Ok(name.to_string())
}

fn platform_label() -> &'static str {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "macOS Apple Silicon"
    } else if cfg!(target_os = "macos") {
        "macOS Intel"
    } else if cfg!(target_os = "windows") {
        "Windows x64"
    } else {
        "当前平台"
    }
}

fn pick_asset_for<'a>(assets: &'a [GithubAsset], os: &str, arch: &str) -> Option<&'a GithubAsset> {
    match os {
        "macos" => {
            let want_arm = arch == "aarch64";
            assets.iter().find(|asset| {
                let name = asset.name.to_ascii_lowercase();
                name.ends_with(".dmg")
                    && if want_arm {
                        name.contains("aarch64")
                    } else {
                        name.contains("x64")
                    }
            })
        }
        "windows" => assets.iter().find(|asset| {
            let name = asset.name.to_ascii_lowercase();
            name.ends_with("x64-setup.exe")
        }),
        _ => None,
    }
}

fn find_checksums_asset(assets: &[GithubAsset]) -> Option<&GithubAsset> {
    assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case(CHECKSUMS_ASSET_NAME))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_checksum(text: &str, file_name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let name = parts.next()?.trim_start_matches('*');
        if name == file_name || name.ends_with(&format!("/{file_name}")) {
            Some(hash.to_ascii_lowercase())
        } else {
            None
        }
    })
}

fn strategy_hint() -> (String, bool, String) {
    if cfg!(target_os = "windows") {
        (
            "windows_installer".to_string(),
            true,
            "将下载官方安装包并启动安装程序，Qwitch 会退出以便覆盖安装。若出现 SmartScreen 提示，请选择「更多信息 → 仍要运行」。".to_string(),
        )
    } else if cfg!(target_os = "macos") {
        (
            "macos_dmg".to_string(),
            false,
            "当前 macOS 包未签名/公证，无法安全自替换。请下载对应架构的 DMG，退出 Qwitch 后将应用拖入 /Applications 覆盖，并执行 xattr -cr /Applications/Qwitch.app。".to_string(),
        )
    } else {
        (
            "manual".to_string(),
            false,
            "请前往 Release 页面手动下载对应平台的安装包。".to_string(),
        )
    }
}

fn expected_asset_name_for(version: &str, os: &str, arch: &str) -> Option<String> {
    match os {
        "macos" => {
            let suffix = if arch == "aarch64" { "aarch64" } else { "x64" };
            Some(format!("Qwitch_{version}_{suffix}.dmg"))
        }
        "windows" => Some(format!("Qwitch_{version}_x64-setup.exe")),
        _ => None,
    }
}

fn expected_asset_name(version: &str) -> Option<String> {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "other"
    };
    expected_asset_name_for(version, os, std::env::consts::ARCH)
}

fn parse_release_tag_version(tag: &str) -> Result<(String, Version), String> {
    let ver_str = tag.trim().trim_start_matches(['v', 'V']).to_string();
    let parsed = Version::parse(&ver_str).map_err(|e| format!("最新版本号无效：{e}"))?;
    Ok((ver_str, parsed))
}

fn current_os_tag() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "other"
    }
}

fn build_update_info_from_release(
    release: GithubRelease,
    current_version: &str,
    os: &str,
    arch: &str,
) -> Result<AppUpdateInfo, String> {
    let current = Version::parse(current_version).map_err(|e| format!("当前版本号无效：{e}"))?;
    let (latest_version, latest) = parse_release_tag_version(&release.tag_name)?;

    let asset = pick_asset_for(&release.assets, os, arch);
    let checksums = find_checksums_asset(&release.assets);
    let (strategy, can_auto_install, hint) = strategy_hint();

    Ok(AppUpdateInfo {
        update_available: latest > current,
        current_version: current_version.to_string(),
        latest_version,
        release_url: release.html_url,
        release_notes: release.body.unwrap_or_default(),
        published_at: release.published_at,
        asset_name: asset.map(|a| a.name.clone()),
        asset_url: asset.map(|a| a.browser_download_url.clone()),
        asset_size: asset.map(|a| a.size).filter(|size| *size > 0),
        checksums_url: checksums.map(|a| a.browser_download_url.clone()),
        strategy,
        platform_label: platform_label().to_string(),
        can_auto_install,
        hint,
    })
}

async fn fetch_release_via_api(client: &reqwest::Client) -> Result<AppUpdateInfo, String> {
    let response = client
        .get(LATEST_RELEASE_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("检查更新失败：{e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "检查更新失败：GitHub 返回 HTTP {}",
            response.status().as_u16()
        ));
    }

    let release: GithubRelease = response
        .json()
        .await
        .map_err(|e| format!("解析版本信息失败：{e}"))?;

    build_update_info_from_release(
        release,
        env!("CARGO_PKG_VERSION"),
        current_os_tag(),
        std::env::consts::ARCH,
    )
}

/// 校验「即将安装的更新」确实是从当前版本升级，而非降级/跨版本替换。
///
/// 背景：`download_and_install_update` 的 `asset_url` / `checksums_url` 由前端传入，
/// 而 `validate_official_repo_url` 只校验「属于本仓库」，因此任意历史 tag 的安装包
/// 都能通过 URL 校验，且其 SHA256SUMS.txt 同样能通过校验和验证 —— 即存在降级安装
/// （downgrade）面。此函数在真正执行安装前重新拉取 latest release 并要求：
///   1. 目标资产名与 latest release 中该平台的资产名逐字一致；
///   2. 目标资产 URL 与 latest release 中该资产的下载地址逐字一致；
///   3. checksums URL（若提供）与 latest release 的 SHA256SUMS.txt 地址一致；
///   4. latest 版本严格大于当前版本（禁止降级与同版本重装）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn validate_update_target(
    latest: &AppUpdateInfo,
    current_version: &str,
    asset_name: &str,
    asset_url: &str,
    checksums_url: Option<&str>,
) -> Result<(), String> {
    let current = Version::parse(current_version).map_err(|e| format!("当前版本号无效：{e}"))?;
    let latest_ver =
        Version::parse(&latest.latest_version).map_err(|e| format!("最新版本号无效：{e}"))?;

    if latest_ver <= current {
        return Err(format!(
            "已中止：目标版本 {} 不高于当前版本 {}（禁止降级或同版本重装）",
            latest.latest_version, current_version
        ));
    }

    let expected_name = latest.asset_name.as_deref().ok_or_else(|| {
        format!(
            "最新版本 {} 没有适用于本平台的安装包",
            latest.latest_version
        )
    })?;
    if expected_name != asset_name {
        return Err(format!(
            "已中止：目标安装包 {asset_name} 与最新版本资产 {expected_name} 不一致"
        ));
    }

    let expected_url = latest
        .asset_url
        .as_deref()
        .ok_or_else(|| "最新版本缺少安装包下载地址".to_string())?;
    if expected_url != asset_url {
        return Err("已中止：目标下载地址与最新版本资产地址不一致".into());
    }

    match (latest.checksums_url.as_deref(), checksums_url) {
        (Some(expected), Some(given)) if expected != given => {
            return Err("已中止：校验和文件地址与最新版本不一致".into());
        }
        (Some(_), None) => {
            return Err("已中止：最新版本提供校验和文件，但请求未提供".into());
        }
        (None, Some(_)) => {
            return Err("已中止：最新版本未提供校验和文件，但请求声明了该文件".into());
        }
        _ => {}
    }

    Ok(())
}

/// GitHub API 有匿名限流（共享出口 IP 很容易 403）。
/// 备用方案走 releases/latest 的 302 跳转获取版本号，再按发布命名规则拼接资产地址。
async fn fetch_release_via_redirect(client: &reqwest::Client) -> Result<AppUpdateInfo, String> {
    let latest_url = format!("https://github.com/{REPO}/releases/latest");
    let response = client
        .get(&latest_url)
        .send()
        .await
        .map_err(|e| format!("备用检查失败：{e}"))?;
    let status = response.status();
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            format!(
                "备用检查失败：GitHub 返回 HTTP {} 且缺少跳转地址",
                status.as_u16()
            )
        })?;

    let release_url = validate_official_repo_url(location)?;
    if !release_url.path().contains("/releases/tag/") {
        return Err("备用检查失败：版本跳转地址不在 GitHub Release 白名单内".into());
    }
    let tag = release_url
        .path()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    if tag.is_empty() {
        return Err("备用检查失败：无法从跳转地址解析版本标签".into());
    }

    let (latest_version, latest) = parse_release_tag_version(&tag)?;
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let current = Version::parse(&current_version).map_err(|e| format!("当前版本号无效：{e}"))?;
    let asset_name = expected_asset_name(&latest_version);
    let asset_url = asset_name
        .as_ref()
        .map(|name| format!("https://github.com/{REPO}/releases/download/{tag}/{name}"));
    let checksums_url = Some(format!(
        "https://github.com/{REPO}/releases/download/{tag}/{CHECKSUMS_ASSET_NAME}"
    ));
    let (strategy, can_auto_install, hint) = strategy_hint();

    Ok(AppUpdateInfo {
        update_available: latest > current,
        current_version,
        latest_version,
        release_url: release_url.to_string(),
        release_notes: String::new(),
        published_at: None,
        asset_name,
        asset_url,
        asset_size: None,
        checksums_url,
        strategy,
        platform_label: platform_label().to_string(),
        can_auto_install,
        hint,
    })
}

#[tauri::command]
pub async fn check_app_update() -> Result<AppUpdateInfo, String> {
    let client = build_client(20)?;
    match fetch_release_via_api(&client).await {
        Ok(info) => Ok(info),
        Err(api_error) => {
            let fallback_client = build_client_no_redirect(20)?;
            fetch_release_via_redirect(&fallback_client)
                .await
                .map_err(|fallback_error| format!("{api_error}；{fallback_error}"))
        }
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
async fn verify_checksum(
    client: &reqwest::Client,
    checksums_url: &str,
    file_name: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let url = validate_official_repo_url(checksums_url)?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载校验文件失败：{e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "下载校验文件失败：HTTP {}",
            response.status().as_u16()
        ));
    }
    validate_github_url(response.url().as_str())?;
    let text = response
        .text()
        .await
        .map_err(|e| format!("读取校验文件失败：{e}"))?;
    let expected = parse_checksum(&text, file_name)
        .ok_or_else(|| "校验文件中缺少该安装包记录，已中止自动安装".to_string())?;
    let actual = hex::encode(Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err("安装包 SHA256 校验失败，已中止自动安装".into());
    }
    Ok(())
}

/// Windows：下载官方安装包、校验 SHA256 后拉起安装程序，并退出 Qwitch 以便覆盖安装。
#[tauri::command]
pub async fn download_and_install_update(
    app: tauri::AppHandle,
    asset_url: String,
    asset_name: String,
    checksums_url: Option<String>,
) -> Result<String, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (&app, &asset_url, &asset_name, &checksums_url);
        Err("当前平台不支持应用内自动安装，请使用手动下载安装包".into())
    }

    #[cfg(target_os = "windows")]
    {
        let file_name = sanitize_asset_name(&asset_name)?;
        if !file_name.to_ascii_lowercase().ends_with(".exe") {
            return Err("更新资产不是 Windows 安装程序".into());
        }
        let checksums_url = checksums_url
            .ok_or_else(|| "发布资产缺少 SHA256SUMS.txt，已中止自动安装".to_string())?;

        let client = build_download_client(300)?;
        let url = validate_official_repo_url(&asset_url)?;

        // 安装前重新确认「目标确实是当前最新版本」：阻断降级安装与跨版本替换。
        // 复用与 check_app_update 相同的取最新版本逻辑（API 优先，限流时降级跳转）。
        let latest = match fetch_release_via_api(&client).await {
            Ok(info) => info,
            Err(api_err) => {
                let no_redirect_client = build_client_no_redirect(20)?;
                fetch_release_via_redirect(&no_redirect_client)
                    .await
                    .map_err(|redirect_err| {
                        format!("无法确认最新版本，已中止自动安装：{api_err}；{redirect_err}")
                    })?
            }
        };
        validate_update_target(
            &latest,
            env!("CARGO_PKG_VERSION"),
            &file_name,
            &asset_url,
            Some(&checksums_url),
        )?;

        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("下载安装包失败：{e}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "下载安装包失败：HTTP {}",
                response.status().as_u16()
            ));
        }
        if let Some(len) = response.content_length() {
            if len > MAX_DOWNLOAD_BYTES {
                return Err("安装包体积异常，已中止下载".into());
            }
        }
        validate_github_url(response.url().as_str())?;
        let cap = response
            .content_length()
            .and_then(|len| usize::try_from(len).ok())
            .unwrap_or(0)
            .min(MAX_DOWNLOAD_BYTES as usize);
        let mut bytes = Vec::with_capacity(cap);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("读取安装包失败：{e}"))?
        {
            if (bytes.len() as u64).saturating_add(chunk.len() as u64) > MAX_DOWNLOAD_BYTES {
                return Err("安装包体积异常，已中止下载".into());
            }
            bytes.extend_from_slice(&chunk);
        }

        verify_checksum(&client, &checksums_url, &file_name, &bytes).await?;

        let path = std::env::temp_dir().join(format!(
            "qwitch-update-{}-{file_name}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, &bytes).map_err(|e| format!("写入临时安装包失败：{e}"))?;
        tokio::process::Command::new(&path)
            .spawn()
            .map_err(|e| format!("启动安装程序失败：{e}"))?;

        let app_exit = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(900)).await;
            app_exit.exit(0);
        });

        Ok("安装程序已启动，Qwitch 即将退出以完成更新".to_string())
    }
}

/// macOS：用系统默认浏览器打开架构对应的 DMG 或 Release 页面。
#[tauri::command]
pub async fn open_update_url(url: String) -> Result<(), String> {
    let parsed = validate_official_repo_url(&url)?;
    #[cfg(target_os = "macos")]
    {
        tokio::process::Command::new("open")
            .arg(parsed.as_str())
            .spawn()
            .map_err(|e| format!("打开下载页失败：{e}"))?;
    }
    #[cfg(target_os = "windows")]
    {
        tokio::process::Command::new("explorer")
            .arg(parsed.as_str())
            .spawn()
            .map_err(|e| format!("打开下载页失败：{e}"))?;
    }
    #[cfg(target_os = "linux")]
    {
        tokio::process::Command::new("xdg-open")
            .arg(parsed.as_str())
            .spawn()
            .map_err(|e| format!("打开下载页失败：{e}"))?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        return Err("当前平台不支持打开下载页".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> GithubAsset {
        GithubAsset {
            name: name.to_string(),
            browser_download_url: format!("https://github.com/x/y/releases/download/v1/{name}"),
            size: 1024,
        }
    }

    #[test]
    fn update_asset_selection_is_arch_specific() {
        let assets = vec![
            asset("Qwitch_0.2.1_aarch64.dmg"),
            asset("Qwitch_0.2.1_x64.dmg"),
            asset("Qwitch_0.2.1_x64-setup.exe"),
            asset("SHA256SUMS.txt"),
        ];
        assert_eq!(
            pick_asset_for(&assets, "macos", "aarch64").map(|a| a.name.as_str()),
            Some("Qwitch_0.2.1_aarch64.dmg")
        );
        assert_eq!(
            pick_asset_for(&assets, "macos", "x86_64").map(|a| a.name.as_str()),
            Some("Qwitch_0.2.1_x64.dmg")
        );
        assert_eq!(
            pick_asset_for(&assets, "windows", "x86_64").map(|a| a.name.as_str()),
            Some("Qwitch_0.2.1_x64-setup.exe")
        );
        assert!(find_checksums_asset(&assets).is_some());

        assert_eq!(
            expected_asset_name_for("0.2.1", "macos", "aarch64").as_deref(),
            Some("Qwitch_0.2.1_aarch64.dmg")
        );
        assert_eq!(
            expected_asset_name_for("0.2.1", "macos", "x86_64").as_deref(),
            Some("Qwitch_0.2.1_x64.dmg")
        );
        assert_eq!(
            expected_asset_name_for("0.2.1", "windows", "x86_64").as_deref(),
            Some("Qwitch_0.2.1_x64-setup.exe")
        );
    }

    #[test]
    fn checksum_parser_matches_file_name_only() {
        let h1 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let h2 = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
        let text = format!(
            "\
{h1}  Qwitch_0.2.1_x64-setup.exe
{h2}  *Qwitch_0.2.1_aarch64.dmg
short123  Qwitch_0.2.1_short.dmg
zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz  Qwitch_0.2.1_badhex.dmg
"
        );
        assert_eq!(
            parse_checksum(&text, "Qwitch_0.2.1_x64-setup.exe").as_deref(),
            Some(h1)
        );
        assert_eq!(
            parse_checksum(&text, "Qwitch_0.2.1_aarch64.dmg").as_deref(),
            Some(h2)
        );
        assert!(parse_checksum(&text, "Qwitch_0.2.1_short.dmg").is_none());
        assert!(parse_checksum(&text, "Qwitch_0.2.1_badhex.dmg").is_none());
        assert!(parse_checksum(&text, "Qwitch_0.2.0_x64.dmg").is_none());
    }

    fn sample_latest(version: &str) -> AppUpdateInfo {
        AppUpdateInfo {
            update_available: true,
            current_version: "0.3.0".to_string(),
            latest_version: version.to_string(),
            release_url: "https://github.com/LuckyOneTwoThree/Qwitch/releases".to_string(),
            release_notes: String::new(),
            published_at: None,
            asset_name: Some("Qwitch_0.4.0_x64-setup.exe".to_string()),
            asset_url: Some(
                "https://github.com/LuckyOneTwoThree/Qwitch/releases/download/v0.4.0/Qwitch_0.4.0_x64-setup.exe"
                    .to_string(),
            ),
            asset_size: Some(1024),
            checksums_url: Some(
                "https://github.com/LuckyOneTwoThree/Qwitch/releases/download/v0.4.0/SHA256SUMS.txt"
                    .to_string(),
            ),
            strategy: "auto_install".to_string(),
            platform_label: "Windows".to_string(),
            can_auto_install: true,
            hint: String::new(),
        }
    }

    #[test]
    fn update_target_rejects_downgrade_and_same_version() {
        let latest = sample_latest("0.4.0");
        let name = "Qwitch_0.4.0_x64-setup.exe";
        let url = latest.asset_url.as_deref().unwrap();
        let ck = latest.checksums_url.as_deref();

        // 正常升级：通过
        assert!(validate_update_target(&latest, "0.3.0", name, url, ck).is_ok());

        // 降级：拒绝（这是 anti-rollback 的核心目标）
        assert!(validate_update_target(&latest, "0.5.0", name, url, ck).is_err());

        // 同版本重装：拒绝
        assert!(validate_update_target(&latest, "0.4.0", name, url, ck).is_err());
    }

    #[test]
    fn update_target_rejects_asset_and_checksum_mismatch() {
        let latest = sample_latest("0.4.0");
        let name = "Qwitch_0.4.0_x64-setup.exe";
        let url = latest.asset_url.as_deref().unwrap();
        let ck = latest.checksums_url.as_deref();

        // 资产名不一致（例如指向旧版本安装包）
        assert!(
            validate_update_target(&latest, "0.3.0", "Qwitch_0.1.0_x64-setup.exe", url, ck)
                .is_err()
        );
        // 下载地址不一致
        assert!(validate_update_target(
            &latest,
            "0.3.0",
            name,
            "https://github.com/LuckyOneTwoThree/Qwitch/releases/download/v0.1.0/Qwitch_0.4.0_x64-setup.exe",
            ck
        )
        .is_err());
        // 校验和地址不一致
        assert!(validate_update_target(
            &latest,
            "0.3.0",
            name,
            url,
            Some("https://github.com/LuckyOneTwoThree/Qwitch/releases/download/v0.1.0/SHA256SUMS.txt")
        )
        .is_err());
        // 请求未提供校验和
        assert!(validate_update_target(&latest, "0.3.0", name, url, None).is_err());
    }

    #[test]
    fn update_url_allowlist_rejects_non_github_and_http() {
        assert!(validate_github_url(
            "https://github.com/LuckyOneTwoThree/Qwitch/releases/download/v0.2.1/a.dmg"
        )
        .is_ok());
        assert!(validate_github_url(
            "https://objects.githubusercontent.com/github-production-release-asset-123/a.exe"
        )
        .is_ok());
        assert!(
            validate_github_url("https://raw.githubusercontent.com/evil/repo/main/a.exe").is_err()
        );
        assert!(validate_official_repo_url(
            "https://github.com/LuckyOneTwoThree/Qwitch/releases/download/v0.2.4/a.dmg"
        )
        .is_ok());
        assert!(validate_official_repo_url("https://objects.githubusercontent.com/x").is_err());
        assert!(
            validate_github_url("https://github.com/evil/repo/releases/download/v1/a.exe").is_err()
        );
        assert!(validate_github_url("http://github.com/LuckyOneTwoThree/Qwitch/x").is_err());
        assert!(validate_github_url("https://example.com/x").is_err());
        assert!(validate_github_url("https://evilgithub.com/x").is_err());
        assert!(sanitize_asset_name("../evil.exe").is_err());
        assert!(sanitize_asset_name("Qwitch_0.2.1_x64-setup.exe").is_ok());
    }

    #[test]
    fn release_api_parses_v_prefixed_tags_and_preserves_metadata() {
        for tag in ["v0.2.4", "V0.2.4"] {
            let release = GithubRelease {
                tag_name: tag.to_string(),
                body: Some("## Release Notes\n- Bug fixes".to_string()),
                html_url: format!("https://github.com/{REPO}/releases/tag/{tag}"),
                published_at: Some("2026-11-20T01:00:00Z".to_string()),
                assets: vec![
                    GithubAsset {
                        name: "Qwitch_0.2.4_aarch64.dmg".to_string(),
                        browser_download_url: format!(
                            "https://github.com/{REPO}/releases/download/{tag}/Qwitch_0.2.4_aarch64.dmg"
                        ),
                        size: 12_345_678,
                    },
                    asset("SHA256SUMS.txt"),
                ],
            };
            let info = build_update_info_from_release(release, "0.2.3", "macos", "aarch64")
                .expect("api release parse");
            assert!(info.update_available);
            assert_eq!(info.latest_version, "0.2.4");
            assert_eq!(info.release_notes, "## Release Notes\n- Bug fixes");
            assert_eq!(info.published_at.as_deref(), Some("2026-11-20T01:00:00Z"));
            assert_eq!(info.asset_size, Some(12_345_678));
            assert_eq!(info.asset_name.as_deref(), Some("Qwitch_0.2.4_aarch64.dmg"));
            assert!(info.checksums_url.is_some());
        }
    }

    #[test]
    fn official_repo_url_and_missing_checksums_metadata() {
        assert!(validate_official_repo_url(
            "https://github.com/LuckyOneTwoThree/Qwitch/releases/latest"
        )
        .is_ok());
        assert!(validate_official_repo_url(
            "http://github.com/LuckyOneTwoThree/Qwitch/releases/latest"
        )
        .is_err());
        assert!(validate_official_repo_url("https://example.com/Qwitch/releases").is_err());
        assert!(
            validate_official_repo_url("https://github.com/OtherUser/Qwitch/releases/latest")
                .is_err()
        );

        let release_no_sum = GithubRelease {
            tag_name: "v0.3.0".to_string(),
            body: None,
            html_url: format!("https://github.com/{REPO}/releases/tag/v0.3.0"),
            published_at: None,
            assets: vec![GithubAsset {
                name: "Qwitch_0.3.0_x64-setup.exe".to_string(),
                browser_download_url: format!(
                    "https://github.com/{REPO}/releases/download/v0.3.0/Qwitch_0.3.0_x64-setup.exe"
                ),
                size: 1024,
            }],
        };
        let info = build_update_info_from_release(release_no_sum, "0.2.3", "windows", "x86_64")
            .expect("parse release without checksums");
        assert!(info.update_available);
        assert!(info.checksums_url.is_none());
        let expected_can_auto_install = cfg!(target_os = "windows");
        assert_eq!(info.can_auto_install, expected_can_auto_install);
    }
}
