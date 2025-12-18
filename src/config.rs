use crate::file_detector::default_extensions;
use directories::ProjectDirs;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub detect_extensions: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            detect_extensions: default_extensions(),
        }
    }
}

#[derive(Deserialize, Default)]
struct FilesConfig {
    detect_extensions: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
struct RawConfig {
    files: Option<FilesConfig>,
}

impl AppConfig {
    pub fn load() -> Self {
        let mut config = AppConfig::default();

        if let Some(path) = config_path() {
            match fs::read_to_string(&path) {
                Ok(content) => {
                    match toml::from_str::<RawConfig>(&content) {
                        Ok(raw) => {
                            if let Some(files) = raw.files {
                                if let Some(exts) = normalize_exts(files.detect_extensions) {
                                    config.detect_extensions = exts;
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "[Config] Failed to parse config file '{}': {} (falling back to defaults)",
                                path.display(),
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    if std::env::var("RKLLM_DEBUG_CONFIG").is_ok() {
                        eprintln!(
                            "[Config] Could not read config file '{}': {} (using defaults)",
                            path.display(),
                            e
                        );
                    }
                }
            }

            if std::env::var("RKLLM_DEBUG_CONFIG").is_ok() {
                eprintln!(
                    "[Config] Loaded detect_extensions: {:?}",
                    config.detect_extensions
                );
            }
        }

        config
    }
}

fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "rkllm-cli").map(|dirs| dirs.config_dir().join("config.toml"))
}

fn normalize_exts(exts: Option<Vec<String>>) -> Option<Vec<String>> {
    let list = exts?;
    if list.is_empty() {
        return Some(Vec::new()); // 明示的に無効化
    }

    let mut seen = std::collections::HashSet::new();
    let mut filtered = Vec::new();

    for ext in list {
        let lower = ext.trim().to_ascii_lowercase();
        if lower.is_empty() {
            continue;
        }
        // 既定の拡張子と同じ制約：英数字のみ
        if !lower.chars().all(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        if seen.insert(lower.clone()) {
            filtered.push(lower);
        }
    }

    if filtered.is_empty() {
        Some(default_extensions())
    } else {
        Some(filtered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_detector::DEFAULT_EXTENSIONS;

    #[test]
    fn default_contains_defaults() {
        let cfg = AppConfig::default();
        for ext in DEFAULT_EXTENSIONS {
            assert!(cfg.detect_extensions.contains(&ext.to_string()));
        }
    }

    #[test]
    fn normalize_accepts_custom_and_dedupes() {
        let exts = Some(vec!["RS".into(), "toml".into(), "rs".into(), "invalid-ext".into()]);
        let normalized = normalize_exts(exts).unwrap();
        assert_eq!(normalized.len(), 2);
        assert!(normalized.contains(&"rs".to_string()));
        assert!(normalized.contains(&"toml".to_string()));
    }

    #[test]
    fn normalize_empty_disables() {
        let normalized = normalize_exts(Some(vec![])).unwrap();
        assert!(normalized.is_empty());
    }
}
