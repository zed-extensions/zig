use zed_extension_api::{Worktree, serde_json::Value};

#[derive(Debug, Clone, PartialEq, Default)]
pub enum CheckUpdates {
    #[default]
    Always,
    Once,
    Never,
}

pub fn get_check_updates(configuration: &Option<Value>) -> CheckUpdates {
    let mode = configuration
        .as_ref()
        .and_then(|cfg| cfg.pointer("/check_updates"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase());

    match mode.as_deref() {
        Some("once") => CheckUpdates::Once,
        Some("never") => CheckUpdates::Never,
        _ => CheckUpdates::default(),
    }
}

/// Reads an explicit `zigscient_path` from the workspace configuration,
/// expanding a leading `~` against the worktree's `$HOME`.
pub fn get_binary_path(configuration: &Option<Value>, worktree: &Worktree) -> Option<String> {
    let path = configuration
        .as_ref()?
        .pointer("/zigscient_path")
        .and_then(|v| v.as_str())?
        .to_string();

    Some(expand_tilde(worktree, path))
}

fn expand_tilde(worktree: &Worktree, path: String) -> String {
    if !path.starts_with('~') {
        return path;
    }
    let home = worktree
        .shell_env()
        .into_iter()
        .find(|(k, _)| k == "HOME")
        .map(|(_, v)| v)
        .unwrap_or_default();

    if home.is_empty() {
        path
    } else {
        path.replacen('~', &home, 1)
    }
}
