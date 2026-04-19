use std::path::PathBuf;

use zed_extension_api::{
    self as zed, LanguageServerId, LanguageServerInstallationStatus, Result,
    set_language_server_installation_status,
};

const REPO: &str = "llogick/zigscient-next";
/// Prefix used in release tag names, e.g. "zigscient-next-0.16.0".
const TAG_PREFIX: &str = "zigscient-next-";

fn arch_str(arch: zed::Architecture) -> &'static str {
    match arch {
        zed::Architecture::Aarch64 => "aarch64",
        zed::Architecture::X86 => "x86",
        zed::Architecture::X8664 => "x86_64",
    }
}

fn os_str(os: zed::Os) -> &'static str {
    match os {
        zed::Os::Mac => "macos",
        zed::Os::Linux => "linux",
        zed::Os::Windows => "windows",
    }
}

/// Returns the name of the zigscient binary as it appears inside the release zip.
pub fn binary_name(os: zed::Os, arch: zed::Architecture) -> &'static str {
    match os {
        zed::Os::Windows => "zigscient.exe",
        _ => match (os, arch) {
            (zed::Os::Mac, zed::Architecture::Aarch64) => "zigscient-aarch64-macos",
            (zed::Os::Mac, zed::Architecture::X8664) => "zigscient-x86_64-macos",
            (zed::Os::Linux, zed::Architecture::Aarch64) => "zigscient-aarch64-linux",
            (zed::Os::Linux, zed::Architecture::X8664) => "zigscient-x86_64-linux",
            _ => "zigscient",
        },
    }
}

struct Release {
    version: String,
    asset_url: String,
}

fn fetch_latest_release() -> Result<Release> {
    let release = zed::latest_github_release(
        REPO,
        zed::GithubReleaseOptions {
            require_assets: true,
            pre_release: false,
        },
    )?;

    let (os, arch) = zed::current_platform();
    let expected_asset = format!("zigscient-{}-{}.zip", arch_str(arch), os_str(os));

    let asset_url = release
        .assets
        .iter()
        .find(|a| a.name == expected_asset)
        .map(|a| a.download_url.clone())
        .ok_or_else(|| {
            format!(
                "No asset '{expected_asset}' found in latest release. \
                 Check https://github.com/{REPO}/releases"
            )
        })?;

    // Tags look like "zigscient-next-0.16.0"; strip the prefix so the
    // install directory is just the semver string.
    let version = release
        .version
        .strip_prefix(TAG_PREFIX)
        .unwrap_or(&release.version)
        .to_string();

    Ok(Release { version, asset_url })
}

/// Returns `true` if the binary at `path` responds successfully to `--version`.
pub fn is_runnable(path: &PathBuf) -> bool {
    zed::Command::new(path.to_string_lossy().as_ref())
        .arg("--version")
        .output()
        .map(|o| o.status == Some(0))
        .unwrap_or(false)
}

/// Resolves the zigscient binary, downloading it from GitHub if necessary.
pub fn resolve_binary(
    language_server_id: &LanguageServerId,
    check_updates: crate::config::CheckUpdates,
) -> Result<PathBuf> {
    set_language_server_installation_status(
        language_server_id,
        &LanguageServerInstallationStatus::CheckingForUpdate,
    );

    let release = fetch_latest_release()?;
    let (os, arch) = zed::current_platform();
    let install_dir = PathBuf::from("zigscient").join(&release.version);
    let binary = install_dir.join(binary_name(os, arch));

    if check_updates != crate::config::CheckUpdates::Always && is_runnable(&binary) {
        return Ok(binary);
    }

    set_language_server_installation_status(
        language_server_id,
        &LanguageServerInstallationStatus::Downloading,
    );

    zed::download_file(
        &release.asset_url,
        &install_dir.to_string_lossy(),
        zed::DownloadedFileType::Zip,
    )
    .map_err(|e| format!("Failed to download zigscient: {e}"))?;

    zed::make_file_executable(&binary.to_string_lossy())
        .map_err(|e| format!("Failed to mark zigscient executable: {e}"))?;

    Ok(binary)
}
