use std::path::Path;

use zed_extension_api as zed;

const PATH_ERR: &str = "Failed to convert path to string";

pub fn path_to_string<P: AsRef<Path>>(path: P) -> zed::Result<String> {
    path.as_ref()
        .to_path_buf()
        .into_os_string()
        .into_string()
        .map_err(|_| PATH_ERR.to_string())
}
