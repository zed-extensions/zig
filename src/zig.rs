use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use zed_extension_api::{self as zed, serde_json, settings::LspSettings, LanguageServerId, Result};

struct ZigExtension {
    cached_version_map: HashMap<Option<String>, String>,
}

#[derive(Deserialize)]
struct AssetInfo {
    pub tarball: String,
    pub shasum: String,
    pub size: String,
}

#[derive(Clone)]
struct ZlsBinary {
    path: String,
    args: Option<Vec<String>>,
    environment: Option<Vec<(String, String)>>,
}

fn download_zls(
    language_server_id: &LanguageServerId,
    binary_path: &str,
    version_dir: &str,
    download_url: &str,
) -> Result<()> {
    let (platform, _) = zed::current_platform();

    zed::set_language_server_installation_status(
        language_server_id,
        &zed::LanguageServerInstallationStatus::Downloading,
    );

    zed::download_file(
        download_url,
        version_dir,
        match platform {
            zed::Os::Mac | zed::Os::Linux => zed::DownloadedFileType::GzipTar,
            zed::Os::Windows => zed::DownloadedFileType::Zip,
        },
    )
    .map_err(|e| format!("failed to download file: {e}"))?;

    zed::make_file_executable(binary_path)?;

    Ok(())
}

impl ZigExtension {
    fn asset_from_github_latest(
        &mut self,
        target: &str,
        extension: &str,
    ) -> Result<(String, String)> {
        // Note that in github releases and on zlstools.org the tar.gz asset is not shown
        // but is available at https://builds.zigtools.org/zls-{os}-{arch}-{version}.tar.gz
        let release = zed::latest_github_release(
            "zigtools/zls",
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let asset_name: String = format!("zls-{}-{}.{}", target, release.version, extension);

        Ok((release.version, asset_name))
    }

    fn language_server_binary(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<ZlsBinary> {
        let mut args: Option<Vec<String>> = None;

        let (platform, arch) = zed::current_platform();
        let environment = match platform {
            zed::Os::Mac | zed::Os::Linux => Some(worktree.shell_env()),
            zed::Os::Windows => None,
        };

        if let Ok(lsp_settings) = LspSettings::for_worktree("zls", worktree) {
            if let Some(binary) = lsp_settings.binary {
                args = binary.arguments;
                if let Some(path) = binary.path {
                    return Ok(ZlsBinary {
                        path: path.clone(),
                        args,
                        environment,
                    });
                }
            }
        }

        if let Some(path) = worktree.which("zls") {
            return Ok(ZlsBinary {
                path,
                args,
                environment,
            });
        }

        let arch: &str = match arch {
            zed::Architecture::Aarch64 => "aarch64",
            zed::Architecture::X86 => "x86",
            zed::Architecture::X8664 => "x86_64",
        };

        let os: &str = match platform {
            zed::Os::Mac => "macos",
            zed::Os::Linux => "linux",
            zed::Os::Windows => "windows",
        };

        let extension = match platform {
            zed::Os::Windows => ".zip",
            _ => ".tar.gz",
        };

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let target = format!("{}-{}", arch, os);

        let zig_version = match worktree.which("zig") {
            Some(_zig_path) => {
                let version_output = zed::Command::new("zig").arg("version").output()?;
                if !matches!(version_output.status, Some(0)) {
                    None
                } else {
                    let zig_version = String::from_utf8(version_output.stdout).map_err(|e| {
                        format!("Failed to parse output of `zig version` command: {}", e)
                    })?;
                    Some(zig_version)
                }
            }
            None => None,
        };

        if let Some(path) = self.cached_version_map.get(&zig_version) {
            if fs::metadata(path).map_or(false, |stat| stat.is_file()) {
                return Ok(ZlsBinary {
                    path: path.clone(),
                    args,
                    environment,
                });
            }
        }

        let (version, download_url) = match zig_version {
            None => {
                let (version, asset_name) = self.asset_from_github_latest(&target, extension)?;

                let download_url = format!("https://builds.zigtools.org/{}", asset_name);

                (version, download_url)
            }
            Some(ref zig_version) => {
                let url = format!("https://releases.zigtools.org/v1/zls/select-version?zig_version={}&compatibility=only-runtime", urlencoding::Encoded(zig_version.trim()));
                let request = zed::http_client::HttpRequest::builder()
                    .url(url)
                    .method(zed::http_client::HttpMethod::Get)
                    .build()?;
                let resp = request.fetch()?;
                let select: HashMap<String, serde_json::Value> = serde_json::from_slice(&resp.body)
                    .map_err(|e| format!("failed to parse select version {e}"))?;

                let version: String =
                    serde_json::from_value(select.get("version").unwrap().clone())
                        .map_err(|e| format!("failed to parse version {e}"))?;
                let asset: AssetInfo = serde_json::from_value(
                    select
                        .get(&target)
                        .ok_or_else(|| format!("failed to find ZLS asset for {target}"))?
                        .clone(),
                )
                .map_err(|e| format!("failed to parse ZLS asset for {target} {e}"))?;

                // Note that in github releases and on zlstools.org the tar.gz asset is not shown
                // but is available at https://builds.zigtools.org/zls-{os}-{arch}-{version}.tar.gz
                let download_url = asset.tarball.replace(".tar.xz", ".tar.gz");

                (version, download_url)
            }
        };

        let version_dir = format!("zls-{}", version);

        let binary_path = match platform {
            zed::Os::Mac | zed::Os::Linux => format!("{version_dir}/zls"),
            zed::Os::Windows => format!("{version_dir}/zls.exe"),
        };

        if !fs::metadata(&binary_path).map_or(false, |stat| stat.is_file()) {
            download_zls(
                language_server_id,
                binary_path.as_str(),
                version_dir.as_str(),
                download_url.as_str(),
            )?;
        }

        self.cached_version_map
            .insert(zig_version, binary_path.clone());

        Ok(ZlsBinary {
            path: binary_path,
            args,
            environment,
        })
    }
}

impl zed::Extension for ZigExtension {
    fn new() -> Self {
        Self {
            cached_version_map: HashMap::new(),
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let zls_binary = self.language_server_binary(language_server_id, worktree)?;
        Ok(zed::Command {
            command: zls_binary.path,
            args: zls_binary.args.unwrap_or_default(),
            env: zls_binary.environment.unwrap_or_default(),
        })
    }

    fn language_server_workspace_configuration(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        let settings = LspSettings::for_worktree("zls", worktree)
            .ok()
            .and_then(|lsp_settings| lsp_settings.settings.clone())
            .unwrap_or_default();
        Ok(Some(settings))
    }
}

zed::register_extension!(ZigExtension);
