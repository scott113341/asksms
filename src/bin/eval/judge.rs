//! The LLM judge: fills in the prompt template and parses the score it returns.

use anyhow::{Context, bail};
use asksms::llm::{self, LlmModel};
use rig_core::completion::{CompletionRequest, Effort, Message};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The most the judge may write. Without a limit, OpenRouter reserves the model's
/// whole output window against the key's spending limit. It's left out of the
/// cache key: a verdict cut off by it doesn't parse, so it's never cached.
const MAX_OUTPUT_TOKENS: u64 = 1000;

/// Bump to throw away every cached judgment.
const KEY_VERSION: u32 = 1;

pub struct Judge {
    pub model: String,
    pub reasoning: Option<Effort>,
    /// The prompt, with `{{question}}`, `{{criteria}}` and `{{answer}}` placeholders.
    pub template: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Judgment {
    /// 1 (wrong) to 5 (correct and complete).
    pub score: u8,
    pub reason: String,
    pub cost: f64,
    pub created_at: String,
}

impl Judge {
    pub fn prompt(&self, question: &str, criteria: &str, answer: &str) -> String {
        self.template
            .replace("{{question}}", question.trim())
            .replace("{{criteria}}", criteria.trim())
            .replace("{{answer}}", answer.trim())
    }

    /// The cache key for judging with `prompt`. The prompt includes the template,
    /// question, criteria and answer, so changing any of them re-judges.
    pub fn key(&self, prompt: &str) -> Value {
        json!({
            "version": KEY_VERSION,
            "model": self.model,
            "reasoning": self.reasoning,
            "prompt": prompt,
        })
    }

    pub async fn grade(&self, model: &LlmModel, prompt: &str) -> anyhow::Result<Judgment> {
        let mut request =
            CompletionRequest::new(Message::user(prompt)).max_tokens(MAX_OUTPUT_TOKENS);
        if let Some(effort) = self.reasoning {
            request = request.reasoning(effort);
        }
        let reply = llm::send(model, request).await?;
        let verdict = parse_verdict(&reply.text)
            .with_context(|| format!("judge replied {:?}", reply.text))?;
        Ok(Judgment {
            score: verdict.score,
            reason: verdict.reason,
            cost: reply.cost,
            created_at: crate::now(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct Verdict {
    reason: String,
    score: u8,
}

/// Reads the `{"reason": ..., "score": ...}` object out of the judge's reply,
/// ignoring any text or code fence around it. If there's more than one, the last
/// one wins, since a judge sometimes drafts one and then corrects it.
fn parse_verdict(text: &str) -> anyhow::Result<Verdict> {
    let verdict = text
        .match_indices('{')
        .filter_map(|(start, _)| {
            serde_json::Deserializer::from_str(&text[start..])
                .into_iter::<Verdict>()
                .next()?
                .ok()
        })
        .next_back()
        .context("no {\"reason\", \"score\"} object")?;
    if !(1..=5).contains(&verdict.score) {
        bail!("score {} is not between 1 and 5", verdict.score);
    }
    Ok(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_verdict() {
        let verdict =
            parse_verdict("```json\n{\"reason\": \"Right.\", \"score\": 4}\n```").unwrap();
        assert_eq!(verdict.score, 4);
        assert_eq!(verdict.reason, "Right.");
    }

    #[test]
    fn uses_the_last_complete_verdict() {
        let reply =
            "{\"reason\": \"Draft.\"}\n\n```json\n{\"reason\": \"Final.\", \"score\": 3}\n```";
        let verdict = parse_verdict(reply).unwrap();
        assert_eq!((verdict.score, verdict.reason.as_str()), (3, "Final."));
    }

    #[test]
    fn rejects_bad_verdicts() {
        assert!(parse_verdict("Looks good, 5/5").is_err());
        assert!(parse_verdict(r#"{"reason": "x", "score": 9}"#).is_err());
    }

    #[test]
    fn fills_in_the_template() {
        let judge = Judge {
            model: "j".to_owned(),
            reasoning: None,
            template: "Q: {{question}}\nC: {{criteria}}\nA: {{answer}}".to_owned(),
        };
        assert_eq!(
            judge.prompt("why?\n", " because", "ok"),
            "Q: why?\nC: because\nA: ok"
        );
    }
}
