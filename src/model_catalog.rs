use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::ReasoningEffort;

const CACHE_FILE: &str = "codex-models.json";
const REFRESH_INTERVAL_SECONDS: u64 = 24 * 60 * 60;
/// Codex publishes a fixed capability order for this family, so alc keeps the
/// catalog sorted from the most capable model to the cheapest one.
const TARGET_MODELS: [&str; 3] = ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCatalog {
    pub schema_version: u32,
    pub refreshed_at: u64,
    pub source: String,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub context_window: u64,
    pub default_effort: ReasoningEffort,
    pub supported_efforts: Vec<ReasoningEffort>,
}

impl ModelCatalog {
    pub fn built_in() -> Self {
        serde_json::from_str(include_str!("../models/codex.json"))
            .expect("the bundled Codex model catalog must be valid")
    }

    pub fn load(config_dir: &Path) -> Self {
        read_cache(config_dir).unwrap_or_else(Self::built_in)
    }

    pub fn load_and_refresh_if_due(config_dir: &Path) -> Self {
        let cached = Self::load(config_dir);
        if !is_due(&cached) {
            return cached;
        }
        Self::refresh(config_dir).unwrap_or(cached)
    }

    pub fn refresh(config_dir: &Path) -> Result<Self> {
        let codex = env::var_os("ALC_CODEX_BIN")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| which::which("codex").ok())
            .context("Codex CLI was not found; install or update Codex, then retry")?;
        let output = Command::new(&codex)
            .args(["debug", "models"])
            .output()
            .with_context(|| format!("failed to run {} debug models", codex.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Codex model discovery failed: {}", stderr.trim());
        }

        let raw: DebugCatalog = serde_json::from_slice(&output.stdout)
            .context("Codex returned an invalid model catalog")?;
        let mut models = Vec::new();
        for target in TARGET_MODELS {
            let Some(model) = raw.models.iter().find(|model| model.slug == target) else {
                continue;
            };
            let supported_efforts: Vec<_> = model
                .supported_reasoning_levels
                .iter()
                .filter_map(|level| level.effort.parse().ok())
                .collect();
            if supported_efforts.is_empty() || model.context_window == 0 {
                continue;
            }
            let default_effort = model
                .default_reasoning_level
                .parse()
                .ok()
                .filter(|effort| supported_efforts.contains(effort))
                .unwrap_or(ReasoningEffort::Medium);
            models.push(ModelInfo {
                id: model.slug.clone(),
                name: model.display_name.clone(),
                description: model.description.clone(),
                context_window: model.context_window,
                default_effort,
                supported_efforts,
            });
        }
        if models.len() != TARGET_MODELS.len() {
            bail!(
                "Codex did not report the complete GPT-5.6 Luna/Terra/Sol family; keeping the previous catalog"
            );
        }

        let catalog = Self {
            schema_version: 1,
            refreshed_at: now_unix(),
            source: "installed Codex CLI (`codex debug models`)".to_owned(),
            models,
        };
        write_cache(config_dir, &catalog)?;
        Ok(catalog)
    }

    pub fn find(&self, id: &str) -> Option<&ModelInfo> {
        self.models.iter().find(|model| model.id == id)
    }
}

#[derive(Debug, Deserialize)]
struct DebugCatalog {
    models: Vec<DebugModel>,
}

#[derive(Debug, Deserialize)]
struct DebugModel {
    slug: String,
    display_name: String,
    description: String,
    context_window: u64,
    default_reasoning_level: String,
    supported_reasoning_levels: Vec<DebugEffort>,
}

#[derive(Debug, Deserialize)]
struct DebugEffort {
    effort: String,
}

fn read_cache(config_dir: &Path) -> Option<ModelCatalog> {
    let text = fs::read_to_string(config_dir.join(CACHE_FILE)).ok()?;
    let catalog: ModelCatalog = serde_json::from_str(&text).ok()?;
    (catalog.schema_version == 1 && !catalog.models.is_empty()).then_some(catalog)
}

fn write_cache(config_dir: &Path, catalog: &ModelCatalog) -> Result<()> {
    fs::create_dir_all(config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;
    let path = config_dir.join(CACHE_FILE);
    let temporary = config_dir.join(format!(".{CACHE_FILE}.tmp"));
    fs::write(&temporary, serde_json::to_vec_pretty(catalog)?)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("failed to replace {}", path.display()))?;
    }
    fs::rename(&temporary, &path).with_context(|| {
        format!(
            "failed to move {} to {}",
            temporary.display(),
            path.display()
        )
    })?;
    Ok(())
}

fn is_due(catalog: &ModelCatalog) -> bool {
    now_unix().saturating_sub(catalog.refreshed_at) >= REFRESH_INTERVAL_SECONDS
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_lists_the_most_capable_model_first() {
        let ids: Vec<_> = ModelCatalog::built_in()
            .models
            .iter()
            .map(|model| model.id.clone())
            .collect();
        assert_eq!(ids, ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]);
    }

    #[test]
    fn bundled_catalog_has_requested_models_and_efforts() {
        let catalog = ModelCatalog::built_in();
        assert_eq!(catalog.models.len(), 3);
        for id in TARGET_MODELS {
            let model = catalog.find(id).expect("requested GPT-5.6 model");
            assert_eq!(
                model.supported_efforts,
                ReasoningEffort::ALL,
                "{id} should offer low through max"
            );
            assert!(
                model.context_window > 0,
                "{id} should include its context window"
            );
        }
    }

    #[test]
    fn a_fresh_catalog_is_not_due() {
        let mut catalog = ModelCatalog::built_in();
        catalog.refreshed_at = now_unix();
        assert!(!is_due(&catalog));
    }
}
