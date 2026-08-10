use std::{collections::HashMap, fs, path::Path};
use zed_extension_api::{self as zed, serde_json, settings::LspSettings, LanguageServerId, Result};

const ZIG_TEST_EXE_BASENAME: &str = "zig_test";

struct ZigExtension {
    // Keyed by the detected `zig version` string (or "latest" when no zig
    // binary is on the worktree's PATH), since different worktrees may use
    // different Zig toolchains and therefore need different ZLS builds.
    cached_zls_paths: HashMap<String, String>,
}

#[derive(Clone)]
struct ZlsBinary {
    path: String,
    args: Option<Vec<String>>,
    environment: Option<Vec<(String, String)>>,
}

impl ZigExtension {
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

        let extension: &str = match platform {
            zed::Os::Mac | zed::Os::Linux => "tar.gz",
            zed::Os::Windows => "zip",
        };

        let target = zls_target(arch, os);
        let zig_version = detect_zig_version(worktree);
        let cache_key = zig_version.clone().unwrap_or_else(|| "latest".to_string());

        if let Some(path) = self.cached_zls_paths.get(&cache_key) {
            if fs::metadata(path).is_ok_and(|stat| stat.is_file()) {
                return Ok(ZlsBinary {
                    path: path.clone(),
                    args,
                    environment,
                });
            }
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        // ZLS enforces that its own version matches the Zig toolchain it runs
        // against, so we must pick the ZLS build released for the Zig version
        // that's actually installed rather than always grabbing the latest
        // ZLS release (see zigtools/zls select-version API).
        let (version, download_url) = match &zig_version {
            Some(zig_version) => matching_zls_release(zig_version, &target)?,
            None => latest_zls_release(&target, extension)?,
        };

        let version_dir = format!("zls-{}", version);
        let binary_path = match platform {
            zed::Os::Mac | zed::Os::Linux => format!("{version_dir}/zls"),
            zed::Os::Windows => format!("{version_dir}/zls.exe"),
        };

        if !fs::metadata(&binary_path).is_ok_and(|stat| stat.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );

            zed::download_file(
                &download_url,
                &version_dir,
                match platform {
                    zed::Os::Mac | zed::Os::Linux => zed::DownloadedFileType::GzipTar,
                    zed::Os::Windows => zed::DownloadedFileType::Zip,
                },
            )
            .map_err(|e| format!("failed to download file: {e}"))?;

            zed::make_file_executable(&binary_path)?;
        }

        self.cached_zls_paths.insert(cache_key, binary_path.clone());
        prune_stale_zls_dirs(self.cached_zls_paths.values());

        Ok(ZlsBinary {
            path: binary_path,
            args,
            environment,
        })
    }
}

// Removes `zls-*` directories that are no longer referenced by any cached
// binary path for this session. Different worktrees may use different Zig
// versions and thus need different ZLS builds concurrently, so we can't just
// wipe everything but the version we most recently downloaded.
fn prune_stale_zls_dirs<'a>(keep_paths: impl Iterator<Item = &'a String>) {
    let keep: std::collections::HashSet<&str> = keep_paths
        .filter_map(|path| path.split('/').next())
        .collect();

    let Ok(entries) = fs::read_dir(".") else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.starts_with("zls-") && !keep.contains(name.as_str()) {
            fs::remove_dir_all(entry.path()).ok();
        }
    }
}

fn zls_target(arch: &str, os: &str) -> String {
    format!("{arch}-{os}")
}

fn detect_zig_version(worktree: &zed::Worktree) -> Option<String> {
    // Use the path `which` actually resolved (respecting the worktree's shell
    // env, e.g. version managers like mise/asdf) rather than a bare "zig",
    // so the version we detect matches the binary that will actually run.
    let zig_path = worktree.which("zig")?;

    let mut command = zed::Command::new(zig_path).arg("version");
    if matches!(zed::current_platform().0, zed::Os::Mac | zed::Os::Linux) {
        command = command.envs(worktree.shell_env());
    }

    let output = command.output().ok()?;
    if !matches!(output.status, Some(0)) {
        return None;
    }

    parse_zig_version_output(&output.stdout)
}

fn parse_zig_version_output(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let version = text.lines().map(str::trim).rfind(|line| !line.is_empty())?;

    version
        .as_bytes()
        .first()
        .filter(|b| b.is_ascii_digit())
        .map(|_| version.to_string())
}

fn latest_asset_url(target: &str, version: &str, extension: &str) -> String {
    format!("https://builds.zigtools.org/zls-{target}-{version}.{extension}")
}

fn latest_zls_release(target: &str, extension: &str) -> Result<(String, String), String> {
    let release = zed::latest_github_release(
        "zigtools/zls",
        zed::GithubReleaseOptions {
            require_assets: true,
            pre_release: false,
        },
    )?;

    let download_url = latest_asset_url(target, &release.version, extension);

    Ok((release.version, download_url))
}

fn matching_zls_release(zig_version: &str, target: &str) -> Result<(String, String), String> {
    let request = zed::http_client::HttpRequest::builder()
        .url(select_version_request_url(zig_version))
        .method(zed::http_client::HttpMethod::Get)
        .build()?;
    let response = request.fetch()?;

    parse_zls_release(&response.body, target)
}

fn percent_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn select_version_request_url(zig_version: &str) -> String {
    format!(
        "https://releases.zigtools.org/v1/zls/select-version?zig_version={}&compatibility=only-runtime",
        percent_encode(zig_version)
    )
}

// Parses the response of https://releases.zigtools.org/v1/zls/select-version,
// which reports the newest ZLS release whose runtime is compatible with the
// given Zig version, and returns its version and the tarball URL for `target`
// (e.g. "x86_64-linux").
fn parse_zls_release(body: &[u8], target: &str) -> Result<(String, String), String> {
    let payload: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| format!("failed to parse ZLS release info: {e}"))?;

    // The select-version endpoint reports errors (e.g. "no ZLS release for
    // this Zig version yet") as a 200 response with a `message`/`error`
    // field instead of an HTTP error status, so check for those first.
    if let Some(message) = payload
        .get("message")
        .or_else(|| payload.get("error"))
        .and_then(|v| v.as_str())
    {
        return Err(format!("zigtools/zls select-version: {message}"));
    }

    let version = payload
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "ZLS release info is missing a `version` field".to_string())?
        .to_string();

    let tarball = payload
        .get(target)
        .and_then(|asset| asset.get("tarball"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| format!("no ZLS release available for target `{target}`"))?;

    Ok((version, tarball.replace(".tar.xz", ".tar.gz")))
}

impl zed::Extension for ZigExtension {
    fn new() -> Self {
        Self {
            cached_zls_paths: HashMap::new(),
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

    fn dap_locator_create_scenario(
        &mut self,
        locator_name: String,
        build_task: zed::TaskTemplate,
        resolved_label: String,
        debug_adapter_name: String,
    ) -> Option<zed::DebugScenario> {
        if build_task.command != "zig" {
            return None;
        }

        let cwd = build_task.cwd.clone();
        let env = build_task.env.clone().into_iter().collect();

        let mut args_it = build_task.args.iter();
        let template = match args_it.next() {
            Some(arg) if arg == "build" => match args_it.next() {
                Some(arg) if arg == "run" => zed::BuildTaskTemplate {
                    label: "zig build".into(),
                    command: "zig".into(),
                    args: vec!["build".into()],
                    env,
                    cwd,
                },
                _ => return None,
            },
            Some(arg) if arg == "test" => {
                let test_exe_path = get_test_exe_path().unwrap();
                let mut args: Vec<String> = build_task
                    .args
                    .into_iter()
                    // TODO verify if this is required on non-Windows platforms
                    .map(|s| s.replace("\"", "'"))
                    .collect();
                args.push("--test-no-exec".into());
                args.push(format!("-femit-bin={test_exe_path}"));

                zed::BuildTaskTemplate {
                    label: "zig test --test-no-exec".into(),
                    command: "zig".into(),
                    args,
                    env,
                    cwd,
                }
            }
            Some(arg) if arg == "run" => zed::BuildTaskTemplate {
                label: "zig run".into(),
                command: "zig".into(),
                args: vec!["run".into()],
                env,
                cwd,
            },
            _ => return None,
        };

        let config = serde_json::Value::Null;
        let Ok(config) = serde_json::to_string(&config) else {
            return None;
        };

        Some(zed::DebugScenario {
            adapter: debug_adapter_name,
            label: resolved_label.clone(),
            config,
            tcp_connection: None,
            build: Some(zed::BuildTaskDefinition::Template(
                zed::BuildTaskDefinitionTemplatePayload {
                    template,
                    locator_name: Some(locator_name),
                },
            )),
        })
    }

    fn run_dap_locator(
        &mut self,
        _locator_name: String,
        build_task: zed::TaskTemplate,
    ) -> Result<zed::DebugRequest, String> {
        let mut args_it = build_task.args.iter();
        match args_it.next() {
            Some(arg) if arg == "build" => {
                // We only handle the default case where the binary name matches the project name.
                // This is valid for projects created with `zig init`.
                // In other cases, the user should provide a custom debug configuration.
                let exec = get_project_name(&build_task).ok_or("Failed to get project name")?;

                let request = zed::LaunchRequest {
                    program: format!("zig-out/bin/{exec}"),
                    cwd: build_task.cwd,
                    args: vec![],
                    envs: build_task.env.into_iter().collect(),
                };

                Ok(zed::DebugRequest::Launch(request))
            }
            Some(arg) if arg == "test" => {
                let program = build_task
                    .args
                    .iter()
                    .find_map(|arg| {
                        arg.strip_prefix("-femit-bin=").map(|arg| {
                            arg.split("=")
                                .nth(1)
                                .ok_or("Expected binary path in -femit-bin=")
                                .map(|path| path.trim_end_matches(".exe"))
                        })
                    })
                    .ok_or("Failed to extract binary path from command args")
                    .flatten()?
                    .to_string();
                let request = zed::LaunchRequest {
                    program,
                    cwd: build_task.cwd,
                    args: vec![],
                    envs: build_task.env.into_iter().collect(),
                };
                Ok(zed::DebugRequest::Launch(request))
            }
            _ => Err("Unsupported build task".into()),
        }
    }
}

fn get_project_name(task: &zed::TaskTemplate) -> Option<String> {
    task.cwd
        .as_ref()
        .and_then(|cwd| Some(Path::new(&cwd).file_name()?.to_string_lossy().into_owned()))
}

fn get_test_exe_path() -> Option<String> {
    let test_exe_dir = std::env::current_dir().ok()?;
    let mut name = format!("{}_{}", ZIG_TEST_EXE_BASENAME, uuid::Uuid::new_v4());
    if zed::current_platform().0 == zed::Os::Windows {
        name.push_str(".exe");
    }
    Some(test_exe_dir.join(name).to_string_lossy().into_owned())
}

zed::register_extension!(ZigExtension);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zls_target_combines_arch_and_os() {
        assert_eq!(zls_target("x86_64", "linux"), "x86_64-linux");
    }

    #[test]
    fn percent_encode_escapes_reserved_characters() {
        assert_eq!(
            percent_encode("0.16.0-dev.74+e0d840bf"),
            "0.16.0-dev.74%2Be0d840bf"
        );
    }

    #[test]
    fn select_version_request_url_encodes_the_zig_version() {
        let url = select_version_request_url("0.16.0-dev.74+e0d840bf");
        assert_eq!(
            url,
            "https://releases.zigtools.org/v1/zls/select-version?zig_version=0.16.0-dev.74%2Be0d840bf&compatibility=only-runtime"
        );
    }

    #[test]
    fn parse_zig_version_output_trims_whitespace() {
        assert_eq!(
            parse_zig_version_output(b"0.16.0\n"),
            Some("0.16.0".to_string())
        );
        assert_eq!(
            parse_zig_version_output(b"  0.16.0-dev.1458+755a3d957  \n"),
            Some("0.16.0-dev.1458+755a3d957".to_string())
        );
        assert_eq!(parse_zig_version_output(b""), None);
        assert_eq!(parse_zig_version_output(b"   \n"), None);
    }

    #[test]
    fn parse_zig_version_output_takes_the_last_non_empty_line() {
        // Some shims (mise, asdf, etc.) print a banner before the version.
        assert_eq!(
            parse_zig_version_output(b"warning: using shim\n0.16.0\n"),
            Some("0.16.0".to_string())
        );
    }

    #[test]
    fn parse_zig_version_output_rejects_non_version_text() {
        assert_eq!(parse_zig_version_output(b"command not found"), None);
    }

    #[test]
    fn latest_asset_url_matches_the_zigtools_naming_scheme() {
        assert_eq!(
            latest_asset_url("x86_64-linux", "0.16.0", "tar.gz"),
            "https://builds.zigtools.org/zls-x86_64-linux-0.16.0.tar.gz"
        );
    }

    #[test]
    fn parse_zls_release_extracts_version_and_tarball_for_target() {
        let body = br#"{"version":"0.16.0","date":"2026-04-16","x86_64-linux":{"tarball":"https://builds.zigtools.org/zls-x86_64-linux-0.16.0.tar.xz","shasum":"abc","size":"123"}}"#;

        let (version, tarball) = parse_zls_release(body, "x86_64-linux").unwrap();

        assert_eq!(version, "0.16.0");
        assert_eq!(
            tarball,
            "https://builds.zigtools.org/zls-x86_64-linux-0.16.0.tar.gz"
        );
    }

    #[test]
    fn parse_zls_release_errors_when_target_is_missing() {
        let body = br#"{"version":"0.16.0","aarch64-macos":{"tarball":"foo"}}"#;

        let err = parse_zls_release(body, "x86_64-linux").unwrap_err();

        assert!(err.contains("x86_64-linux"));
    }

    #[test]
    fn parse_zls_release_errors_on_invalid_json() {
        assert!(parse_zls_release(b"not json", "x86_64-linux").is_err());
    }

    #[test]
    fn parse_zls_release_surfaces_api_reported_errors() {
        let body = br#"{"code":3,"message":"ZLS 9.9 has not been released yet"}"#;
        let err = parse_zls_release(body, "x86_64-linux").unwrap_err();
        assert!(err.contains("ZLS 9.9 has not been released yet"));

        let body = br#"{"error":"'9.9' is not a valid version!"}"#;
        let err = parse_zls_release(body, "x86_64-linux").unwrap_err();
        assert!(err.contains("is not a valid version"));
    }

    #[test]
    fn parse_zls_release_preserves_windows_zip_extension() {
        let body = br#"{"version":"0.16.0","x86_64-windows":{"tarball":"https://builds.zigtools.org/zls-x86_64-windows-0.16.0.zip"}}"#;

        let (_, tarball) = parse_zls_release(body, "x86_64-windows").unwrap();

        assert_eq!(
            tarball,
            "https://builds.zigtools.org/zls-x86_64-windows-0.16.0.zip"
        );
    }
}
