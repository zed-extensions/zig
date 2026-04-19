mod config;
mod labels;
mod util;
mod zigscient;

use std::path::PathBuf;

use zed_extension_api::{
    self as zed, CodeLabel, LanguageServerId, Result,
    lsp::{Completion, Symbol},
    register_extension, serde_json,
    settings::LspSettings,
};

use crate::{
    config::{get_binary_path, get_check_updates},
    labels::{label_for_completion as zig_label_completion, label_for_symbol as zig_label_symbol},
    util::path_to_string,
};

struct ZigExtension {
    cached_binary_path: Option<PathBuf>,
}

impl ZigExtension {
    fn language_server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<PathBuf> {
        let configuration = LspSettings::for_worktree("zigscient", worktree)
            .ok()
            .and_then(|s| s.settings);

        // 1. Explicit path from workspace settings (`zigscient_path`).
        if let Some(user_path) = get_binary_path(&configuration, worktree) {
            let p = PathBuf::from(user_path);
            self.cached_binary_path = Some(p.clone());
            return Ok(p);
        }

        // 2. Explicit binary path via `lsp.zigscient.binary.path`.
        if let Ok(lsp_settings) = LspSettings::for_worktree("zigscient", worktree) {
            if let Some(binary) = lsp_settings.binary
                && let Some(path) = binary.path
            {
                let p = PathBuf::from(&path);
                self.cached_binary_path = Some(p.clone());
                return Ok(p);
            }
        }

        // 3. zigscient already on PATH.
        if let Some(path) = worktree.which("zigscient") {
            let p = PathBuf::from(path);
            self.cached_binary_path = Some(p.clone());
            return Ok(p);
        }

        // 4. Reuse a previously resolved path if it is still runnable.
        if let Some(cached) = self.cached_binary_path.clone() {
            if zigscient::is_runnable(&cached) {
                return Ok(cached);
            }
        }

        // 5. Download from GitHub releases.
        let check_updates = get_check_updates(&configuration);
        let path = zigscient::resolve_binary(language_server_id, check_updates)?;
        self.cached_binary_path = Some(path.clone());
        Ok(path)
    }
}

impl zed::Extension for ZigExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let binary_path = self.language_server_binary_path(language_server_id, worktree)?;

        let args = LspSettings::for_worktree("zigscient", worktree)
            .ok()
            .and_then(|s| s.binary)
            .and_then(|b| b.arguments)
            .unwrap_or_default();

        let env = match zed::current_platform().0 {
            zed::Os::Mac | zed::Os::Linux => worktree.shell_env(),
            zed::Os::Windows => vec![],
        };

        Ok(zed::Command {
            command: path_to_string(&binary_path)?,
            args,
            env,
        })
    }

    fn language_server_workspace_configuration(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        let settings = LspSettings::for_worktree("zigscient", worktree)
            .ok()
            .and_then(|s| s.settings)
            .unwrap_or_default();
        Ok(Some(settings))
    }

    fn label_for_completion(
        &self,
        language_server_id: &LanguageServerId,
        completion: Completion,
    ) -> Option<CodeLabel> {
        zig_label_completion(language_server_id, completion)
    }

    fn label_for_symbol(
        &self,
        language_server_id: &LanguageServerId,
        symbol: Symbol,
    ) -> Option<CodeLabel> {
        zig_label_symbol(language_server_id, symbol)
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
        let env: Vec<(String, String)> = build_task.env.clone().into_iter().collect();

        let mut args_it = build_task.args.iter();
        let template = match args_it.next().map(String::as_str) {
            Some("build") => match args_it.next().map(String::as_str) {
                Some("run") => zed::BuildTaskTemplate {
                    label: "zig build run".into(),
                    command: "zig".into(),
                    args: vec!["build".into(), "run".into()],
                    env,
                    cwd,
                },
                _ => return None,
            },

            Some("test") => {
                let test_exe_path = make_test_exe_path()?;
                let mut args: Vec<String> = build_task
                    .args
                    .into_iter()
                    .map(|s| s.replace('"', "'"))
                    .collect();
                args.push("--test-no-exec".into());
                args.push(format!("-femit-bin={test_exe_path}"));

                zed::BuildTaskTemplate {
                    label: "zig test (no-exec)".into(),
                    command: "zig".into(),
                    args,
                    env,
                    cwd,
                }
            }

            Some("run") => zed::BuildTaskTemplate {
                label: "zig run".into(),
                command: "zig".into(),
                args: vec!["run".into()],
                env,
                cwd,
            },

            _ => return None,
        };

        let config = serde_json::to_string(&serde_json::Value::Null).ok()?;

        Some(zed::DebugScenario {
            adapter: debug_adapter_name,
            label: resolved_label,
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
        match build_task.args.first().map(String::as_str) {
            Some("build") => {
                let exec = project_name_from_task(&build_task)
                    .ok_or("Failed to determine project name from cwd")?;

                Ok(zed::DebugRequest::Launch(zed::LaunchRequest {
                    program: format!("zig-out/bin/{exec}"),
                    cwd: build_task.cwd,
                    args: vec![],
                    envs: build_task.env.into_iter().collect(),
                }))
            }

            Some("test") => {
                // The build step emits the test binary via `-femit-bin=<path>`;
                // extract that path and strip any trailing ".exe".
                let program = build_task
                    .args
                    .iter()
                    .find_map(|arg| arg.strip_prefix("-femit-bin="))
                    .map(|path| path.trim_end_matches(".exe").to_string())
                    .ok_or("Could not extract test binary path from -femit-bin= argument")?;

                Ok(zed::DebugRequest::Launch(zed::LaunchRequest {
                    program,
                    cwd: build_task.cwd,
                    args: vec![],
                    envs: build_task.env.into_iter().collect(),
                }))
            }

            _ => Err("Unsupported zig sub-command for DAP locator".into()),
        }
    }
}

fn project_name_from_task(task: &zed::TaskTemplate) -> Option<String> {
    use std::path::Path;
    task.cwd
        .as_deref()
        .and_then(|cwd| Path::new(cwd).file_name())
        .map(|n| n.to_string_lossy().into_owned())
}

/// Generates a unique path for the test binary relative to the extension's
/// working directory (avoids needing `std::env::current_dir`).
fn make_test_exe_path() -> Option<String> {
    let mut name = format!("zig_test_{}", uuid::Uuid::new_v4());
    if zed::current_platform().0 == zed::Os::Windows {
        name.push_str(".exe");
    }
    Some(name)
}

register_extension!(ZigExtension);
