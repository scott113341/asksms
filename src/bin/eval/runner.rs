//! Asks each (provider, scenario, sample) its question and judges the answer,
//! reusing cached answers and judgments wherever their inputs haven't changed.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use asksms::llm;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cache::Cache;
use crate::checks::{self, Check};
use crate::config::{Provider, Scenario};
use crate::judge::{Judge, Judgment};

/// How many questions are in flight at once.
pub const CONCURRENCY: usize = 8;

/// Bump to throw away every cached answer.
const ANSWER_KEY_VERSION: u32 = 1;

pub struct Runner {
    pub cache: Cache,
    pub http: reqwest::Client,
    pub base_url: String,
    pub api_key: String,
    pub judge: Judge,
    /// Report what's missing from the cache without calling any model.
    pub dry_run: bool,
    pub refresh_answers: bool,
    pub refresh_judgments: bool,
}

#[derive(Clone, Copy)]
pub struct Job<'a> {
    pub provider: &'a Provider,
    pub scenario: &'a Scenario,
    /// Starts at 1.
    pub sample: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CachedAnswer {
    pub text: String,
    pub cost: f64,
    pub asked_for_concise: bool,
    /// Whether the output-token limit cut the answer off.
    pub truncated: bool,
    pub latency_ms: u64,
    pub created_at: String,
}

/// Where a result came from in this run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Cache,
    Fetched,
    /// Not cached, and this is a dry run.
    Missing,
    Failed,
    /// Not judged because there's no answer.
    Skipped,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub provider: String,
    pub scenario: String,
    pub sample: u32,
    pub answer_source: Source,
    pub answer: Option<CachedAnswer>,
    pub answer_error: Option<String>,
    pub checks: Vec<Check>,
    pub judgment_source: Source,
    pub judgment: Option<Judgment>,
    pub judgment_error: Option<String>,
}

impl Outcome {
    pub fn checks_passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }

    pub fn failed(&self) -> bool {
        self.answer_source == Source::Failed || self.judgment_source == Source::Failed
    }
}

/// The cache key for an answer: everything that affects what the model is asked.
pub fn answer_key(provider: &Provider, question: &str, sample: u32) -> Value {
    let answer = &provider.answer;
    json!({
        "version": ANSWER_KEY_VERSION,
        "model": provider.model,
        "instructions": answer.instructions,
        "max_output_tokens": answer.max_output_tokens,
        "plugins": answer.plugins,
        "reasoning": answer.reasoning,
        "max_answer_chars": llm::MAX_ANSWER_CHARS,
        "concise_prompt": llm::CONCISE_PROMPT,
        "question": question,
        "sample": sample,
    })
}

impl Runner {
    /// Runs every job, returning outcomes in the same order.
    pub async fn run(&self, jobs: Vec<Job<'_>>) -> Vec<Outcome> {
        let total = jobs.len();
        let done = &AtomicUsize::new(0);
        futures::stream::iter(jobs)
            .map(|job| async move {
                let outcome = self.run_job(job).await;
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                progress(n, total, &outcome);
                outcome
            })
            .buffered(CONCURRENCY)
            .collect()
            .await
    }

    async fn run_job(&self, job: Job<'_>) -> Outcome {
        let mut outcome = Outcome {
            provider: job.provider.name.clone(),
            scenario: job.scenario.id.clone(),
            sample: job.sample,
            answer_source: Source::Missing,
            answer: None,
            answer_error: None,
            checks: Vec::new(),
            judgment_source: Source::Skipped,
            judgment: None,
            judgment_error: None,
        };

        let key = answer_key(job.provider, &job.scenario.question, job.sample);
        let cached = (!self.refresh_answers)
            .then(|| self.cache.get::<CachedAnswer>("answers", &key))
            .flatten();
        let answer = match cached {
            Some(answer) => {
                outcome.answer_source = Source::Cache;
                answer
            }
            None if self.dry_run => return outcome,
            None => match self.fetch_answer(job).await {
                Ok(answer) => {
                    outcome.answer_source = Source::Fetched;
                    if let Err(err) = self.cache.put("answers", &key, &answer) {
                        eprintln!("warning: couldn't cache an answer: {err:#}");
                    }
                    answer
                }
                Err(err) => {
                    outcome.answer_source = Source::Failed;
                    outcome.answer_error = Some(format!("{err:#}"));
                    return outcome;
                }
            },
        };
        outcome.checks = checks::run(&answer.text, answer.truncated);

        let prompt =
            self.judge
                .prompt(&job.scenario.question, &job.scenario.criteria, &answer.text);
        outcome.answer = Some(answer);
        let key = self.judge.key(&prompt);
        let cached = (!self.refresh_judgments)
            .then(|| self.cache.get::<Judgment>("judgments", &key))
            .flatten();
        match cached {
            Some(judgment) => {
                outcome.judgment_source = Source::Cache;
                outcome.judgment = Some(judgment);
            }
            None if self.dry_run => outcome.judgment_source = Source::Missing,
            None => {
                let model = self.model(&self.judge.model);
                match self.judge.grade(&model, &prompt).await {
                    Ok(judgment) => {
                        outcome.judgment_source = Source::Fetched;
                        if let Err(err) = self.cache.put("judgments", &key, &judgment) {
                            eprintln!("warning: couldn't cache a judgment: {err:#}");
                        }
                        outcome.judgment = Some(judgment);
                    }
                    Err(err) => {
                        outcome.judgment_source = Source::Failed;
                        outcome.judgment_error = Some(format!("{err:#}"));
                    }
                }
            }
        }
        outcome
    }

    async fn fetch_answer(&self, job: Job<'_>) -> anyhow::Result<CachedAnswer> {
        let model = self.model(&job.provider.model);
        let started = Instant::now();
        let answer = llm::get_answer(&model, &job.provider.answer, &job.scenario.question).await?;
        Ok(CachedAnswer {
            text: answer.text,
            cost: answer.cost,
            asked_for_concise: answer.asked_for_concise,
            truncated: answer.truncated,
            latency_ms: started.elapsed().as_millis() as u64,
            created_at: crate::now(),
        })
    }

    fn model(&self, model_id: &str) -> llm::LlmModel {
        llm::model(&self.http, &self.base_url, &self.api_key, model_id)
    }
}

/// The start of a long error, for progress lines. The full error is in the results.
fn shorten(err: &str) -> String {
    const MAX: usize = 300;
    if err.chars().count() <= MAX {
        err.to_owned()
    } else {
        format!("{}…", err.chars().take(MAX).collect::<String>())
    }
}

/// Prints a line for each job that did work, so a fully cached run stays quiet.
fn progress(n: usize, total: usize, outcome: &Outcome) {
    let label = format!(
        "[{n}/{total}] {} / {} #{}",
        outcome.provider, outcome.scenario, outcome.sample
    );
    if let Some(err) = &outcome.answer_error {
        eprintln!("{label}: answer failed: {}", shorten(err));
    } else if let Some(err) = &outcome.judgment_error {
        eprintln!("{label}: judging failed: {}", shorten(err));
    } else if outcome.answer_source == Source::Fetched || outcome.judgment_source == Source::Fetched
    {
        let mut parts = Vec::new();
        if let (Source::Fetched, Some(answer)) = (outcome.answer_source, &outcome.answer) {
            parts.push(format!(
                "answered in {:.1}s (${:.4})",
                answer.latency_ms as f64 / 1000.0,
                answer.cost
            ));
        }
        if let Some(judgment) = &outcome.judgment {
            parts.push(format!("scored {}/5", judgment.score));
        }
        eprintln!("{label}: {}", parts.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use asksms::llm::AnswerConfig;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn completion(content: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "id": "gen-1",
            "object": "chat.completion",
            "created": 1_761_284_632,
            "model": "m",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "cost": 0.001 }
        }))
    }

    async fn mock_openrouter() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_partial_json(json!({ "model": "judge/model" })))
            .respond_with(completion(r#"{"reason": "Fine.", "score": 4}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(completion("An answer."))
            .mount(&server)
            .await;
        server
    }

    fn runner(server: &MockServer, cache_dir: std::path::PathBuf) -> Runner {
        Runner {
            cache: Cache::new(cache_dir),
            http: reqwest::Client::new(),
            base_url: server.uri(),
            api_key: "key".to_owned(),
            judge: Judge {
                model: "judge/model".to_owned(),
                reasoning: None,
                template: "{{question}} {{criteria}} {{answer}}".to_owned(),
            },
            dry_run: false,
            refresh_answers: false,
            refresh_judgments: false,
        }
    }

    fn provider(name: &str) -> Provider {
        Provider {
            name: name.to_owned(),
            model: format!("{name}/model"),
            answer: AnswerConfig::default(),
        }
    }

    fn scenario(id: &str) -> Scenario {
        Scenario {
            id: id.to_owned(),
            question: format!("question {id}?"),
            criteria: "anything".to_owned(),
        }
    }

    fn jobs<'a>(providers: &'a [Provider], scenarios: &'a [Scenario]) -> Vec<Job<'a>> {
        providers
            .iter()
            .flat_map(|provider| {
                scenarios.iter().map(move |scenario| Job {
                    provider,
                    scenario,
                    sample: 1,
                })
            })
            .collect()
    }

    /// Runs `jobs` against a fresh mock OpenRouter, returning how many answer and
    /// judge requests it got, and the outcomes.
    async fn run_counting(
        cache_dir: &std::path::Path,
        jobs: Vec<Job<'_>>,
        configure: impl FnOnce(&mut Runner),
    ) -> ((usize, usize), Vec<Outcome>) {
        let server = mock_openrouter().await;
        let mut runner = runner(&server, cache_dir.to_owned());
        configure(&mut runner);
        let outcomes = runner.run(jobs).await;

        let requests = server.received_requests().await.unwrap();
        let judge = requests
            .iter()
            .filter(|r| r.body_json::<Value>().unwrap()["model"] == "judge/model")
            .count();
        ((requests.len() - judge, judge), outcomes)
    }

    #[tokio::test]
    async fn only_runs_what_is_not_cached() {
        let dir = crate::test_dir("runner");
        let providers = [provider("a")];
        let scenarios = [scenario("s1"), scenario("s2")];

        let (counts, outcomes) = run_counting(&dir, jobs(&providers, &scenarios), |_| {}).await;
        assert_eq!(counts, (2, 2));
        assert!(outcomes.iter().all(|o| o.answer_source == Source::Fetched));
        assert!(
            outcomes
                .iter()
                .all(|o| o.judgment.as_ref().unwrap().score == 4)
        );

        // Everything is cached now.
        let (counts, outcomes) = run_counting(&dir, jobs(&providers, &scenarios), |_| {}).await;
        assert_eq!(counts, (0, 0));
        assert!(outcomes.iter().all(|o| o.answer_source == Source::Cache));
        assert!(outcomes.iter().all(|o| o.judgment_source == Source::Cache));

        // A new provider answers every scenario; the new scenario is answered by
        // every provider. The mock gives "b" the same answers "a" got for s1 and
        // s2, and judgments are keyed by question, criteria and answer, so those
        // two are already judged.
        let providers = [provider("a"), provider("b")];
        let scenarios = [scenario("s1"), scenario("s2"), scenario("s3")];
        let (counts, _) = run_counting(&dir, jobs(&providers, &scenarios), |_| {}).await;
        assert_eq!(counts, (4, 2));
    }

    #[tokio::test]
    async fn changing_a_setting_or_the_criteria_reruns_only_what_it_affects() {
        let dir = crate::test_dir("runner-invalidation");
        let mut providers = [provider("a")];
        let mut scenarios = [scenario("s1")];

        let (counts, _) = run_counting(&dir, jobs(&providers, &scenarios), |_| {}).await;
        assert_eq!(counts, (1, 1));

        // New criteria: same answer, new judgment.
        scenarios[0].criteria = "something else".to_owned();
        let (counts, _) = run_counting(&dir, jobs(&providers, &scenarios), |_| {}).await;
        assert_eq!(counts, (0, 1));

        // New system prompt: new answer. The mock replies with the same text, so
        // the judgment is still cached.
        providers[0].answer.instructions = "Be terse.".to_owned();
        let (counts, _) = run_counting(&dir, jobs(&providers, &scenarios), |_| {}).await;
        assert_eq!(counts, (1, 0));

        // A second sample is a new answer.
        let mut two = jobs(&providers, &scenarios);
        two.push(Job {
            sample: 2,
            ..two[0]
        });
        let (counts, _) = run_counting(&dir, two, |_| {}).await;
        assert_eq!(counts, (1, 0));

        // Refreshing asks again even though everything is cached.
        let (counts, _) = run_counting(&dir, jobs(&providers, &scenarios), |r| {
            r.refresh_answers = true;
            r.refresh_judgments = true;
        })
        .await;
        assert_eq!(counts, (1, 1));
    }

    #[tokio::test]
    async fn a_dry_run_calls_nothing() {
        let dir = crate::test_dir("runner-dry");
        let providers = [provider("a")];
        let scenarios = [scenario("s1")];
        let (counts, outcomes) =
            run_counting(&dir, jobs(&providers, &scenarios), |r| r.dry_run = true).await;
        assert_eq!(counts, (0, 0));
        assert_eq!(outcomes[0].answer_source, Source::Missing);
    }

    #[tokio::test]
    async fn records_failures_without_caching_them() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("upstream down"))
            .mount(&server)
            .await;
        let dir = crate::test_dir("runner-failure");

        let providers = [provider("a")];
        let scenarios = [scenario("s1")];
        let outcomes = runner(&server, dir.clone())
            .run(jobs(&providers, &scenarios))
            .await;
        assert_eq!(outcomes[0].answer_source, Source::Failed);
        assert!(outcomes[0].failed());

        let key = answer_key(&providers[0], &scenarios[0].question, 1);
        assert_eq!(Cache::new(dir).get::<CachedAnswer>("answers", &key), None);
    }
}
