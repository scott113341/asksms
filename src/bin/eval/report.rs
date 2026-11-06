//! Summarizes outcomes and writes them to the terminal, a JSON file and an HTML page.

use std::fmt::Write as _;

use asksms::split::{SMS_LENGTH, split_into_messages};
use rig_core::completion::Effort;
use serde::Serialize;
use serde_json::Value;

use crate::config::{Provider, Scenario};
use crate::runner::{Outcome, Source};

#[derive(Serialize)]
pub struct Report<'a> {
    pub created_at: String,
    pub judge_model: String,
    pub judge_reasoning: Option<Effort>,
    pub samples: u32,
    pub run: RunStats,
    pub providers: Vec<ProviderSummary>,
    pub scenarios: Vec<ScenarioInfo>,
    pub outcomes: &'a [Outcome],
}

/// What this run had to fetch, as opposed to what came from the cache.
#[derive(Serialize)]
pub struct RunStats {
    pub answers_fetched: usize,
    pub judgments_fetched: usize,
    pub cost: f64,
    pub failures: usize,
}

#[derive(Serialize)]
pub struct ProviderSummary {
    pub name: String,
    pub model: String,
    pub plugins: Vec<Value>,
    pub reasoning: Option<Effort>,
    pub max_output_tokens: u64,
    pub instructions: String,
    pub runs: usize,
    pub answered: usize,
    pub judged: usize,
    pub mean_score: Option<f64>,
    /// Answers that passed every format check.
    pub format_passed: usize,
    /// Answers the model had to be asked to shorten.
    pub shortened: usize,
    /// Answers cut off by the output-token limit.
    pub truncated: usize,
    pub mean_chars: Option<f64>,
    pub mean_sms: Option<f64>,
    pub mean_cost: Option<f64>,
    pub mean_latency_ms: Option<f64>,
    pub failures: usize,
}

#[derive(Serialize)]
pub struct ScenarioInfo {
    pub id: String,
    pub question: String,
    pub criteria: String,
}

/// One scenario × provider cell of the score matrix, across its samples.
struct Cell {
    mean_score: Option<f64>,
    format_failed: bool,
    failed: bool,
    missing: bool,
}

impl<'a> Report<'a> {
    pub fn new(
        providers: &[Provider],
        scenarios: &[Scenario],
        outcomes: &'a [Outcome],
        judge_model: String,
        judge_reasoning: Option<Effort>,
        samples: u32,
    ) -> Self {
        let fetched = |o: &&Outcome| o.answer_source == Source::Fetched;
        let judged = |o: &&Outcome| o.judgment_source == Source::Fetched;
        let run = RunStats {
            answers_fetched: outcomes.iter().filter(fetched).count(),
            judgments_fetched: outcomes.iter().filter(judged).count(),
            // A fold from 0.0 rather than sum(), which gives -0.0 for no items.
            cost: outcomes.iter().fold(0.0, |total, o| {
                let answer = o
                    .answer
                    .as_ref()
                    .filter(|_| o.answer_source == Source::Fetched);
                let judgment = o
                    .judgment
                    .as_ref()
                    .filter(|_| o.judgment_source == Source::Fetched);
                total + answer.map_or(0.0, |a| a.cost) + judgment.map_or(0.0, |j| j.cost)
            }),
            failures: outcomes.iter().filter(|o| o.failed()).count(),
        };

        Self {
            created_at: crate::now(),
            judge_model,
            judge_reasoning,
            samples,
            run,
            providers: providers.iter().map(|p| summarize(p, outcomes)).collect(),
            scenarios: scenarios
                .iter()
                .map(|s| ScenarioInfo {
                    id: s.id.clone(),
                    question: s.question.clone(),
                    criteria: s.criteria.clone(),
                })
                .collect(),
            outcomes,
        }
    }

    fn outcomes_for<'b>(
        &'b self,
        provider: &'b str,
        scenario: &'b str,
    ) -> impl Iterator<Item = &'b Outcome> + 'b {
        self.outcomes
            .iter()
            .filter(move |o| o.provider == provider && o.scenario == scenario)
    }

    fn cell(&self, provider: &str, scenario: &str) -> Cell {
        let outcomes: Vec<&Outcome> = self.outcomes_for(provider, scenario).collect();
        let scores: Vec<f64> = outcomes
            .iter()
            .filter_map(|o| o.judgment.as_ref())
            .map(|j| f64::from(j.score))
            .collect();
        Cell {
            mean_score: mean(&scores),
            format_failed: outcomes
                .iter()
                .any(|o| o.answer.is_some() && !o.checks_passed()),
            failed: outcomes.iter().any(|o| o.failed()),
            missing: outcomes.iter().any(|o| {
                o.answer_source == Source::Missing || o.judgment_source == Source::Missing
            }),
        }
    }

    pub fn print_terminal(&self) {
        let names: Vec<&str> = self.providers.iter().map(|p| p.name.as_str()).collect();
        let first = self
            .scenarios
            .iter()
            .map(|s| s.id.chars().count())
            .chain([14])
            .max()
            .unwrap_or(14);
        let widths: Vec<usize> = names.iter().map(|n| n.chars().count().max(8)).collect();

        let row = |label: &str, cells: Vec<String>| {
            let mut line = format!("{label:<first$}");
            for (cell, width) in cells.iter().zip(&widths) {
                let _ = write!(line, "  {cell:>width$}");
            }
            println!("{line}");
        };

        println!();
        println!("Mean judge score, 1-5 (! = failed a format check, E = error, ? = not run yet)");
        println!();
        row("", names.iter().map(|n| (*n).to_owned()).collect());
        for scenario in &self.scenarios {
            let cells = names
                .iter()
                .map(|provider| {
                    let cell = self.cell(provider, &scenario.id);
                    let mut text = cell
                        .mean_score
                        .map_or("-".to_owned(), |score| format!("{score:.1}"));
                    if cell.format_failed {
                        text.push('!');
                    }
                    if cell.failed {
                        text.push('E');
                    }
                    if cell.missing {
                        text.push('?');
                    }
                    text
                })
                .collect();
            row(&scenario.id, cells);
        }
        println!();
        let stat = |f: &dyn Fn(&ProviderSummary) -> String| -> Vec<String> {
            self.providers.iter().map(f).collect()
        };
        row(
            "mean score",
            stat(&|p| p.mean_score.map_or("-".to_owned(), |s| format!("{s:.2}"))),
        );
        row(
            "format pass",
            stat(&|p| format!("{}/{}", p.format_passed, p.answered)),
        );
        row(
            "cost/answer",
            stat(&|p| p.mean_cost.map_or("-".to_owned(), |c| format!("${c:.4}"))),
        );
        row(
            "latency",
            stat(&|p| {
                p.mean_latency_ms
                    .map_or("-".to_owned(), |ms| format!("{:.1}s", ms / 1000.0))
            }),
        );
        row(
            "chars",
            stat(&|p| p.mean_chars.map_or("-".to_owned(), |c| format!("{c:.0}"))),
        );
        row(
            "SMS",
            stat(&|p| p.mean_sms.map_or("-".to_owned(), |n| format!("{n:.1}"))),
        );
        row("cut off", stat(&|p| p.truncated.to_string()));
        row("shortened", stat(&|p| p.shortened.to_string()));
        row("errors", stat(&|p| p.failures.to_string()));
        println!();
        println!(
            "This run fetched {} answers and {} judgments for ${:.4}.",
            self.run.answers_fetched, self.run.judgments_fetched, self.run.cost
        );
    }

    /// For a dry run: what's missing from the cache and roughly what it'll cost,
    /// estimated from the provider's (and judge's) cached costs.
    pub fn print_dry_run(&self) {
        println!();
        let judge_costs: Vec<f64> = self
            .outcomes
            .iter()
            .filter_map(|o| o.judgment.as_ref())
            .map(|j| j.cost)
            .collect();
        let judge_mean = mean(&judge_costs);
        let mut total = 0.0;
        let mut unknown = false;

        for provider in &self.providers {
            let mine = || self.outcomes.iter().filter(|o| o.provider == provider.name);
            let answers = mine()
                .filter(|o| o.answer_source == Source::Missing)
                .count();
            let judgments = mine()
                .filter(|o| {
                    o.judgment_source == Source::Missing || o.answer_source == Source::Missing
                })
                .count();
            let mut estimate = String::new();
            if answers > 0 {
                match provider.mean_cost {
                    Some(cost) => total += cost * answers as f64,
                    None => unknown = true,
                }
            }
            if judgments > 0 {
                match judge_mean {
                    Some(cost) => total += cost * judgments as f64,
                    None => unknown = true,
                }
            }
            if answers > 0 && provider.mean_cost.is_none() {
                estimate = " (no cached answers to estimate cost from)".to_owned();
            }
            println!(
                "{}: {answers} answers and {judgments} judgments to fetch{estimate}",
                provider.name
            );
        }
        println!();
        if unknown && total == 0.0 {
            println!("Estimated cost: unknown, since nothing is cached to estimate from");
        } else if unknown {
            println!("Estimated cost: more than ${total:.4}");
        } else {
            println!("Estimated cost: ${total:.4}");
        }
    }

    pub fn html(&self) -> String {
        let mut html = String::new();
        let h = &mut html;
        let _ = write!(
            h,
            r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>askSMS eval results</title>
<style>{STYLE}</style>
</head>
<body>
<main>
<h1>askSMS eval results</h1>
<p class="meta">{created} · judged by <code>{judge}</code>{judge_reasoning} · {samples} sample{plural} per question · {providers} providers × {scenarios} scenarios</p>
"#,
            created = esc(&self.created_at),
            judge = esc(&self.judge_model),
            judge_reasoning = self
                .judge_reasoning
                .map(|e| format!(" ({} reasoning)", e.as_str()))
                .unwrap_or_default(),
            samples = self.samples,
            plural = if self.samples == 1 { "" } else { "s" },
            providers = self.providers.len(),
            scenarios = self.scenarios.len(),
        );

        // Provider summary.
        let best = self
            .providers
            .iter()
            .filter_map(|p| p.mean_score)
            .fold(None, |best: Option<f64>, s| {
                Some(best.map_or(s, |b| b.max(s)))
            });
        h.push_str(
            r#"<h2>Providers</h2>
<div class="scroll"><table class="summary">
<thead><tr><th>Provider</th><th>Settings</th><th class="num">Mean score</th><th class="num">Format pass</th><th class="num">Cost / answer</th><th class="num">Latency</th><th class="num">Chars</th><th class="num">SMS</th><th class="num">Cut off</th><th class="num">Shortened</th><th class="num">Errors</th></tr></thead>
<tbody>
"#,
        );
        let default_instructions = asksms::llm::INSTRUCTIONS;
        for p in &self.providers {
            let mut settings = vec![format!("<code>{}</code>", esc(&p.model))];
            let plugins: Vec<String> = p.plugins.iter().map(|v| v.to_string()).collect();
            settings.push(if plugins.is_empty() {
                "no plugins".to_owned()
            } else {
                format!("plugins <code>{}</code>", esc(&plugins.join(", ")))
            });
            if let Some(effort) = p.reasoning {
                settings.push(format!("{} reasoning", effort.as_str()));
            }
            settings.push(format!("{} max tokens", p.max_output_tokens));
            if p.instructions != default_instructions {
                settings.push(format!(
                    r#"<span title="{}">custom system prompt</span>"#,
                    esc(&p.instructions)
                ));
            }
            let is_best = p.mean_score.is_some() && p.mean_score == best;
            let _ = writeln!(
                h,
                r#"<tr><th scope="row">{name}</th><td class="settings">{settings}</td><td class="num{best}">{score}</td><td class="num">{passed}/{answered}</td><td class="num">{cost}</td><td class="num">{latency}</td><td class="num">{chars}</td><td class="num">{sms}</td><td class="num">{truncated}</td><td class="num">{shortened}</td><td class="num">{failures}</td></tr>"#,
                name = esc(&p.name),
                settings = settings.join(" · "),
                best = if is_best { " best" } else { "" },
                score = p.mean_score.map_or("–".to_owned(), |s| format!("{s:.2}")),
                passed = p.format_passed,
                answered = p.answered,
                cost = p.mean_cost.map_or("–".to_owned(), |c| format!("${c:.4}")),
                latency = p
                    .mean_latency_ms
                    .map_or("–".to_owned(), |ms| format!("{:.1}s", ms / 1000.0)),
                chars = p.mean_chars.map_or("–".to_owned(), |c| format!("{c:.0}")),
                sms = p.mean_sms.map_or("–".to_owned(), |n| format!("{n:.1}")),
                truncated = p.truncated,
                shortened = p.shortened,
                failures = p.failures,
            );
        }
        h.push_str("</tbody></table></div>\n");

        // Score matrix.
        h.push_str(
            r#"<h2>Scores</h2>
<p class="legend">Mean judge score per scenario.
<span class="swatches">1 <i class="s1"></i><i class="s2"></i><i class="s3"></i><i class="s4"></i><i class="s5"></i> 5</span>
<span><b class="flag">!</b> failed a format check</span>
<span><b class="flag">E</b> error</span></p>
<div class="scroll"><table class="matrix">
<thead><tr><th>Scenario</th>"#,
        );
        for p in &self.providers {
            let _ = write!(h, "<th>{}</th>", esc(&p.name));
        }
        h.push_str("</tr></thead>\n<tbody>\n");
        for s in &self.scenarios {
            let _ = write!(
                h,
                r##"<tr><th scope="row"><a href="#{id}">{id_text}</a></th>"##,
                id = esc(&anchor(&s.id)),
                id_text = esc(&s.id),
            );
            for p in &self.providers {
                let cell = self.cell(&p.name, &s.id);
                let reasons: Vec<String> = self
                    .outcomes_for(&p.name, &s.id)
                    .map(
                        |o| match (&o.judgment, &o.answer_error, &o.judgment_error) {
                            (Some(j), _, _) => format!("{}/5: {}", j.score, j.reason),
                            (_, Some(e), _) | (_, _, Some(e)) => format!("error: {e}"),
                            _ => "not run".to_owned(),
                        },
                    )
                    .collect();
                let class = cell
                    .mean_score
                    .map_or("none".to_owned(), |s| format!("s{}", s.round() as u8));
                let mut flags = String::new();
                if cell.format_failed {
                    flags.push_str(r#"<b class="flag">!</b>"#);
                }
                if cell.failed {
                    flags.push_str(r#"<b class="flag">E</b>"#);
                }
                let _ = write!(
                    h,
                    r#"<td class="{class}" title="{title}">{score}{flags}</td>"#,
                    title = esc(&reasons.join("\n")),
                    score = cell
                        .mean_score
                        .map_or("–".to_owned(), |s| format!("{s:.1}")),
                );
            }
            h.push_str("</tr>\n");
        }
        h.push_str("</tbody></table></div>\n");

        // Every answer, by scenario.
        h.push_str("<h2>Answers</h2>\n");
        for s in &self.scenarios {
            let _ = write!(
                h,
                r#"<details id="{anchor}"><summary><span class="id">{id}</span> {question}</summary>
<div class="scenario">
<p class="criteria"><b>Criteria:</b> {criteria}</p>
"#,
                anchor = esc(&anchor(&s.id)),
                id = esc(&s.id),
                question = esc(&s.question),
                criteria = esc(&s.criteria),
            );
            for p in &self.providers {
                let _ = write!(h, r#"<section class="provider"><h3>{}</h3>"#, esc(&p.name));
                for o in self.outcomes_for(&p.name, &s.id) {
                    h.push_str(&answer_html(o, self.samples > 1));
                }
                h.push_str("</section>\n");
            }
            h.push_str("</div></details>\n");
        }

        h.push_str("</main>\n</body>\n</html>\n");
        html
    }
}

fn answer_html(o: &Outcome, show_sample: bool) -> String {
    let mut h = String::from(r#"<div class="answer">"#);
    let sample = if show_sample {
        format!(r#"<span class="sample">#{}</span>"#, o.sample)
    } else {
        String::new()
    };

    let Some(answer) = &o.answer else {
        let text = o
            .answer_error
            .as_deref()
            .map_or("Not run yet.".to_owned(), |e| format!("Error: {e}"));
        let _ = write!(h, r#"{sample}<p class="error">{}</p></div>"#, esc(&text));
        return h;
    };

    let badge = match (&o.judgment, &o.judgment_error) {
        (Some(j), _) => format!(
            r#"<span class="badge s{score}">{score}/5</span> <span class="reason">{reason}</span>"#,
            score = j.score,
            reason = esc(&j.reason),
        ),
        (None, Some(e)) => format!(r#"<span class="error">Judging failed: {}</span>"#, esc(e)),
        (None, None) => r#"<span class="reason">Not judged yet.</span>"#.to_owned(),
    };
    let _ = write!(
        h,
        r#"<div class="verdict">{sample}{badge}</div><p class="text">{}</p>"#,
        esc(&answer.text)
    );

    let mut meta = vec![
        format!("{} chars", answer.text.chars().count()),
        format!(
            "{} SMS",
            split_into_messages(&answer.text, SMS_LENGTH).len()
        ),
        format!("${:.4}", answer.cost),
        format!("{:.1}s", answer.latency_ms as f64 / 1000.0),
    ];
    if answer.asked_for_concise {
        meta.push("asked to shorten".to_owned());
    }
    if answer.truncated {
        meta.push("cut off by token limit".to_owned());
    }
    let _ = write!(h, r#"<p class="meta">{}</p>"#, meta.join(" · "));

    let failed: Vec<String> = o
        .checks
        .iter()
        .filter(|c| !c.passed)
        .map(|c| format!("<li><b>{}</b>: {}</li>", c.name, esc(&c.detail)))
        .collect();
    if !failed.is_empty() {
        let _ = write!(h, r#"<ul class="checks">{}</ul>"#, failed.concat());
    }
    h.push_str("</div>");
    h
}

fn summarize(provider: &Provider, outcomes: &[Outcome]) -> ProviderSummary {
    let mine: Vec<&Outcome> = outcomes
        .iter()
        .filter(|o| o.provider == provider.name)
        .collect();
    let answers: Vec<_> = mine.iter().filter_map(|o| o.answer.as_ref()).collect();
    let scores: Vec<f64> = mine
        .iter()
        .filter_map(|o| o.judgment.as_ref())
        .map(|j| f64::from(j.score))
        .collect();
    let costs: Vec<f64> = answers.iter().map(|a| a.cost).collect();
    let latencies: Vec<f64> = answers.iter().map(|a| a.latency_ms as f64).collect();
    let chars: Vec<f64> = answers
        .iter()
        .map(|a| a.text.chars().count() as f64)
        .collect();
    let sms: Vec<f64> = answers
        .iter()
        .map(|a| split_into_messages(&a.text, SMS_LENGTH).len() as f64)
        .collect();

    ProviderSummary {
        name: provider.name.clone(),
        model: provider.model.clone(),
        plugins: provider.answer.plugins.clone(),
        reasoning: provider.answer.reasoning,
        max_output_tokens: provider.answer.max_output_tokens,
        instructions: provider.answer.instructions.clone(),
        runs: mine.len(),
        answered: answers.len(),
        judged: scores.len(),
        mean_score: mean(&scores),
        format_passed: mine
            .iter()
            .filter(|o| o.answer.is_some() && o.checks_passed())
            .count(),
        shortened: answers.iter().filter(|a| a.asked_for_concise).count(),
        truncated: answers.iter().filter(|a| a.truncated).count(),
        mean_chars: mean(&chars),
        mean_sms: mean(&sms),
        mean_cost: mean(&costs),
        mean_latency_ms: mean(&latencies),
        failures: mine.iter().filter(|o| o.failed()).count(),
    }
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn anchor(id: &str) -> String {
    format!("scenario-{id}")
}

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const STYLE: &str = r#"
:root {
  color-scheme: light;
  --surface: #fcfcfb;
  --surface-2: #f3f2ef;
  --border: #e2e1dc;
  --text: #0b0b0b;
  --text-2: #52514e;
  --accent: #2a78d6;
  --error: #b42626;
  --s1: #cde2fb; --s1-text: #0b0b0b;
  --s2: #86b6ef; --s2-text: #0b0b0b;
  --s3: #3987e5; --s3-text: #ffffff;
  --s4: #256abf; --s4-text: #ffffff;
  --s5: #104281; --s5-text: #ffffff;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    color-scheme: dark;
    --surface: #1a1a19;
    --surface-2: #242423;
    --border: #383835;
    --text: #f0efec;
    --text-2: #c3c2b7;
    --accent: #6da7ec;
    --error: #f07a7a;
    --s1: #0d366b; --s1-text: #f0efec;
    --s2: #184f95; --s2-text: #f0efec;
    --s3: #256abf; --s3-text: #ffffff;
    --s4: #3987e5; --s4-text: #0b0b0b;
    --s5: #86b6ef; --s5-text: #0b0b0b;
  }
}
:root[data-theme="dark"] {
  color-scheme: dark;
  --surface: #1a1a19;
  --surface-2: #242423;
  --border: #383835;
  --text: #f0efec;
  --text-2: #c3c2b7;
  --accent: #6da7ec;
  --error: #f07a7a;
  --s1: #0d366b; --s1-text: #f0efec;
  --s2: #184f95; --s2-text: #f0efec;
  --s3: #256abf; --s3-text: #ffffff;
  --s4: #3987e5; --s4-text: #0b0b0b;
  --s5: #86b6ef; --s5-text: #0b0b0b;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  background: var(--surface);
  color: var(--text);
  font: 15px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
}
main { max-width: 1100px; margin: 0 auto; padding: 24px 16px 64px; }
h1 { font-size: 24px; margin: 0 0 4px; }
h2 { font-size: 18px; margin: 32px 0 8px; }
h3 { font-size: 14px; margin: 0 0 6px; }
code { font: 13px ui-monospace, SFMono-Regular, Menlo, monospace; }
.meta, .legend { color: var(--text-2); font-size: 13px; margin: 0 0 8px; }
.legend { display: flex; flex-wrap: wrap; gap: 4px 16px; align-items: center; }
.swatches i { display: inline-block; width: 14px; height: 14px; border-radius: 3px; vertical-align: -2px; margin-right: 2px; }
.swatches i.s1 { background: var(--s1); } .swatches i.s2 { background: var(--s2); }
.swatches i.s3 { background: var(--s3); } .swatches i.s4 { background: var(--s4); }
.swatches i.s5 { background: var(--s5); }
.scroll { overflow-x: auto; }
table { border-collapse: separate; border-spacing: 0; font-size: 14px; }
th, td { padding: 6px 10px; text-align: left; border-bottom: 1px solid var(--border); white-space: nowrap; }
thead th { color: var(--text-2); font-weight: 600; font-size: 13px; }
.num { text-align: right; font-variant-numeric: tabular-nums; }
.best { font-weight: 700; }
.summary { width: 100%; }
.summary .settings { white-space: normal; color: var(--text-2); font-size: 13px; min-width: 260px; }
.summary .settings span { text-decoration: underline dotted; cursor: help; }
.matrix { border-spacing: 2px; }
.matrix th { border: 0; }
.matrix td { border: 0; border-radius: 4px; text-align: center; min-width: 72px; font-variant-numeric: tabular-nums; font-weight: 600; cursor: help; }
.matrix a { color: var(--accent); text-decoration: none; }
.s1 { background: var(--s1); color: var(--s1-text); }
.s2 { background: var(--s2); color: var(--s2-text); }
.s3 { background: var(--s3); color: var(--s3-text); }
.s4 { background: var(--s4); color: var(--s4-text); }
.s5 { background: var(--s5); color: var(--s5-text); }
.none { background: var(--surface-2); color: var(--text-2); }
.flag { margin-left: 3px; color: inherit; }
.legend .flag { color: var(--text); margin: 0 2px 0 0; }
details { border: 1px solid var(--border); border-radius: 8px; margin: 8px 0; background: var(--surface); }
summary { padding: 10px 14px; cursor: pointer; }
summary .id { font-weight: 600; margin-right: 6px; }
.scenario { padding: 0 14px 14px; }
.criteria { background: var(--surface-2); border-radius: 6px; padding: 8px 12px; margin: 0 0 12px; }
.provider { border-top: 1px solid var(--border); padding-top: 10px; margin-top: 10px; }
.answer + .answer { margin-top: 10px; }
.verdict { display: flex; gap: 8px; align-items: baseline; flex-wrap: wrap; }
.badge { border-radius: 4px; padding: 1px 6px; font-weight: 700; font-size: 13px; }
.sample { color: var(--text-2); font-size: 13px; }
.reason { color: var(--text-2); }
.text { white-space: pre-wrap; margin: 6px 0 2px; }
.checks { margin: 4px 0 0; padding-left: 18px; color: var(--error); font-size: 13px; }
.error { color: var(--error); white-space: pre-wrap; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html() {
        assert_eq!(
            esc(r#"<b>"Tom" & 'Jerry'</b>"#),
            "&lt;b&gt;&quot;Tom&quot; &amp; &#39;Jerry&#39;&lt;/b&gt;"
        );
    }
}
