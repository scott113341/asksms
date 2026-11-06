//! Loads the tracked eval files: `eval.toml`, `providers.toml` and `scenarios/*.toml`.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, bail};
use asksms::llm::AnswerConfig;
use rig_core::completion::Effort;
use serde::Deserialize;
use serde_json::Value;

/// `eval.toml`: settings for the whole eval.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalConfig {
    /// How many times each question is asked of each provider.
    #[serde(default = "one")]
    pub samples: u32,
    pub judge: JudgeConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JudgeConfig {
    pub model: String,
    pub reasoning: Option<Effort>,
}

fn one() -> u32 {
    1
}

/// A named model plus the settings it answers with.
#[derive(Debug)]
pub struct Provider {
    pub name: String,
    pub model: String,
    pub answer: AnswerConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvidersFile {
    #[serde(default)]
    provider: Vec<ProviderEntry>,
}

/// A `[[provider]]` entry. Any setting left out uses production's value.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderEntry {
    name: String,
    model: String,
    plugins: Option<Vec<Value>>,
    reasoning: Option<Effort>,
    instructions: Option<String>,
    max_output_tokens: Option<u64>,
}

/// A question to ask, and what the judge should expect of the answer.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// The file name without `.toml`.
    #[serde(skip)]
    pub id: String,
    pub question: String,
    pub criteria: String,
}

pub fn load_eval_config(path: &Path) -> anyhow::Result<EvalConfig> {
    let config: EvalConfig = parse_toml(path)?;
    if config.samples == 0 {
        bail!("{}: samples must be at least 1", path.display());
    }
    Ok(config)
}

pub fn load_providers(path: &Path) -> anyhow::Result<Vec<Provider>> {
    let file: ProvidersFile = parse_toml(path)?;
    let mut seen = HashSet::new();
    let mut providers = Vec::new();
    for entry in file.provider {
        if !seen.insert(entry.name.clone()) {
            bail!(
                "{}: provider {:?} is listed twice",
                path.display(),
                entry.name
            );
        }
        let defaults = AnswerConfig::default();
        providers.push(Provider {
            name: entry.name,
            model: entry.model,
            answer: AnswerConfig {
                instructions: entry.instructions.unwrap_or(defaults.instructions),
                max_output_tokens: entry
                    .max_output_tokens
                    .unwrap_or(defaults.max_output_tokens),
                plugins: entry.plugins.unwrap_or(defaults.plugins),
                reasoning: entry.reasoning.or(defaults.reasoning),
            },
        });
    }
    if providers.is_empty() {
        bail!("{}: no [[provider]] entries", path.display());
    }
    Ok(providers)
}

/// Loads every `*.toml` in `dir`, sorted by id.
pub fn load_scenarios(dir: &Path) -> anyhow::Result<Vec<Scenario>> {
    let mut scenarios = Vec::new();
    let entries = fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let mut scenario: Scenario = parse_toml(&path)?;
        scenario.id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .context("scenario file name is not UTF-8")?
            .to_owned();
        scenarios.push(scenario);
    }
    scenarios.sort_by(|a, b| a.id.cmp(&b.id));
    if scenarios.is_empty() {
        bail!("no scenarios in {}", dir.display());
    }
    Ok(scenarios)
}

fn parse_toml<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn providers_fall_back_to_production_settings() {
        let dir = crate::test_dir("providers");
        let path = dir.join("providers.toml");
        fs::write(
            &path,
            r#"
            [[provider]]
            name = "default"
            model = "a/b"

            [[provider]]
            name = "custom"
            model = "c/d"
            plugins = [{ id = "web", max_results = 2 }]
            reasoning = "low"
            max_output_tokens = 300
            "#,
        )
        .unwrap();

        let providers = load_providers(&path).unwrap();
        assert_eq!(providers[0].answer, AnswerConfig::default());
        assert_eq!(
            providers[1].answer.plugins,
            vec![json!({ "id": "web", "max_results": 2 })]
        );
        assert_eq!(providers[1].answer.reasoning, Some(Effort::Low));
        assert_eq!(providers[1].answer.max_output_tokens, 300);
        assert_eq!(
            providers[1].answer.instructions,
            AnswerConfig::default().instructions
        );
    }

    #[test]
    fn rejects_duplicate_provider_names() {
        let dir = crate::test_dir("duplicate-providers");
        let path = dir.join("providers.toml");
        fs::write(
            &path,
            "[[provider]]\nname = \"x\"\nmodel = \"a\"\n[[provider]]\nname = \"x\"\nmodel = \"b\"\n",
        )
        .unwrap();
        let err = load_providers(&path).unwrap_err();
        assert!(err.to_string().contains("listed twice"), "{err:#}");
    }
}
