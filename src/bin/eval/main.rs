//! Evaluates how well different OpenRouter models and settings answer questions.
//!
//! Asks every scenario in `evals/scenarios/` of every provider in
//! `evals/providers.toml`, checks each answer's format and has an LLM judge
//! score it against the scenario's criteria. Answers and judgments are cached in
//! `evals/.cache/`, so only new or changed combinations call a model. Results go
//! to the terminal and to `evals/results/`.
//!
//! Run it with `cargo eval`; see `cargo eval --help`.

mod cache;
mod checks;
mod config;
mod judge;
mod report;
mod runner;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::Parser;

use crate::cache::Cache;
use crate::judge::Judge;
use crate::report::Report;
use crate::runner::{Job, Runner};

/// Asks each provider each scenario's question and has an LLM judge score the answers.
/// Cached answers and judgments are reused, so only new or changed work calls a model.
#[derive(Parser)]
struct Args {
    /// Only run this provider (repeatable).
    #[arg(long = "provider", short = 'p', value_name = "NAME")]
    providers: Vec<String>,

    /// Only run this scenario (repeatable).
    #[arg(long = "scenario", short = 's', value_name = "ID")]
    scenarios: Vec<String>,

    /// How many times to ask each question, overriding eval.toml.
    #[arg(long, short = 'n')]
    samples: Option<u32>,

    /// Show what isn't cached and roughly what it would cost, without calling any model.
    #[arg(long)]
    dry_run: bool,

    /// Ask the selected questions again even if their answers are cached.
    #[arg(long)]
    refresh_answers: bool,

    /// Judge the selected answers again even if their judgments are cached.
    #[arg(long)]
    refresh_judgments: bool,
}

fn evals_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("evals")
}

/// The current local time, for timestamps in the cache and results.
pub fn now() -> String {
    jiff::Zoned::now()
        .strftime("%Y-%m-%d %H:%M:%S %Z")
        .to_string()
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Returns whether every job succeeded.
async fn run(args: Args) -> anyhow::Result<bool> {
    let dir = evals_dir();
    dotenvy::from_path(Path::new(env!("CARGO_MANIFEST_DIR")).join(".env")).ok();

    let eval_config = config::load_eval_config(&dir.join("eval.toml"))?;
    let providers = select(
        config::load_providers(&dir.join("providers.toml"))?,
        &args.providers,
        |p| &p.name,
        "provider",
    )?;
    let scenarios = select(
        config::load_scenarios(&dir.join("scenarios"))?,
        &args.scenarios,
        |s| &s.id,
        "scenario",
    )?;
    let samples = args.samples.unwrap_or(eval_config.samples);
    if samples == 0 {
        bail!("--samples must be at least 1");
    }

    let api_key = match std::env::var("OPENROUTER_API_KEY") {
        Ok(key) if !key.is_empty() => key,
        _ if args.dry_run => String::new(),
        _ => bail!("OPENROUTER_API_KEY must be set (in the environment or .env)"),
    };
    let template =
        fs::read_to_string(dir.join("judge-prompt.md")).context("reading evals/judge-prompt.md")?;

    let runner = Runner {
        cache: Cache::new(dir.join(".cache")),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?,
        base_url: asksms::env_or("OPENROUTER_BASE_URL", "https://openrouter.ai/api/v1"),
        api_key,
        judge: Judge {
            model: eval_config.judge.model.clone(),
            reasoning: eval_config.judge.reasoning,
            template,
        },
        dry_run: args.dry_run,
        refresh_answers: args.refresh_answers,
        refresh_judgments: args.refresh_judgments,
    };

    let mut jobs = Vec::new();
    for provider in &providers {
        for scenario in &scenarios {
            for sample in 1..=samples {
                jobs.push(Job {
                    provider,
                    scenario,
                    sample,
                });
            }
        }
    }
    eprintln!(
        "{} providers × {} scenarios × {samples} sample{} = {} answers",
        providers.len(),
        scenarios.len(),
        if samples == 1 { "" } else { "s" },
        jobs.len()
    );

    let outcomes = runner.run(jobs).await;
    let report = Report::new(
        &providers,
        &scenarios,
        &outcomes,
        eval_config.judge.model,
        eval_config.judge.reasoning,
        samples,
    );

    if args.dry_run {
        report.print_dry_run();
        return Ok(true);
    }

    report.print_terminal();

    let results = dir.join("results");
    fs::create_dir_all(&results).context("creating evals/results")?;
    let stem = jiff::Zoned::now().strftime("%Y-%m-%dT%H-%M-%S").to_string();
    let json_path = results.join(format!("{stem}.json"));
    let html_path = results.join(format!("{stem}.html"));
    fs::write(&json_path, serde_json::to_vec_pretty(&report)?)?;
    fs::write(&html_path, report.html())?;
    println!("Wrote {} and {}", json_path.display(), html_path.display());

    if report.run.failures > 0 {
        eprintln!(
            "{} answers failed; run again to retry them (failures aren't cached).",
            report.run.failures
        );
    }
    Ok(report.run.failures == 0)
}

/// Keeps the items named in `wanted`, or all of them if it's empty.
fn select<T>(
    items: Vec<T>,
    wanted: &[String],
    name: impl Fn(&T) -> &String,
    kind: &str,
) -> anyhow::Result<Vec<T>> {
    if wanted.is_empty() {
        return Ok(items);
    }
    let known: HashSet<&String> = items.iter().map(&name).collect();
    for w in wanted {
        if !known.contains(w) {
            let mut names: Vec<&&String> = known.iter().collect();
            names.sort();
            bail!("unknown {kind} {w:?}; choose from {names:?}");
        }
    }
    Ok(items
        .into_iter()
        .filter(|item| wanted.contains(name(item)))
        .collect())
}

/// A fresh, empty directory for a test.
#[cfg(test)]
pub fn test_dir(name: &str) -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("asksms-eval-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}
