use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoragePaths {
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub downloads_dir: PathBuf,
}

static CUSTOM_PATHS: OnceLock<StoragePaths> = OnceLock::new();

pub fn init_storage_paths(paths: StoragePaths) {
    let _ = CUSTOM_PATHS.set(paths);
}

pub fn app_config_root_dir() -> PathBuf {
    if let Some(p) = CUSTOM_PATHS.get() {
        return p.config_dir.clone();
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("squad-air")
}

pub fn app_cache_root_dir() -> PathBuf {
    if let Some(p) = CUSTOM_PATHS.get() {
        return p.cache_dir.clone();
    }
    app_config_root_dir().join("cache")
}

pub fn app_downloads_root_dir() -> PathBuf {
    if let Some(p) = CUSTOM_PATHS.get() {
        return p.downloads_dir.clone();
    }
    dirs::download_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("SquadAir")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_paths_are_non_empty() {
        assert!(!app_config_root_dir().as_os_str().is_empty());
        assert!(!app_cache_root_dir().as_os_str().is_empty());
        assert!(!app_downloads_root_dir().as_os_str().is_empty());
    }
}
