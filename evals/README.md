# Evals

Compares how well different OpenRouter models and settings answer questions.
Every scenario is asked of every provider using the same code path as production
(`asksms::llm::get_answer`). The format rules are checked in code, and an LLM
judge scores each answer from 1 to 5 against the scenario's criteria.

```sh
cargo eval                       # run everything that isn't cached yet
cargo eval --dry-run             # show what would run, and roughly what it'd cost
cargo eval -p sonar -s rattlesnake-bite
cargo eval -n 3                  # ask each question 3 times
cargo eval --refresh-answers -p sonar   # ask again even if cached
```

It reads `OPENROUTER_API_KEY` from the environment or `.env`. Results print to
the terminal and are written to `evals/results/<time>.json` and `.html`.

## Files

| Path | What it holds |
|---|---|
| `eval.toml` | Judge model and the default number of samples |
| `providers.toml` | Each provider: model, plugins, reasoning effort, system prompt, token cap |
| `scenarios/*.toml` | One scenario per file: a `question` and the `criteria` the judge grades against |
| `judge-prompt.md` | The judge's prompt, with `{{question}}`, `{{criteria}}` and `{{answer}}` placeholders |
| `.cache/`, `results/` | Cached answers and judgments, and result files (both gitignored) |

## Caching

Answers are cached by a hash of the model, plugins, reasoning effort, system
prompt, token cap, the question and the sample number. Judgments are cached by a
hash of the judge model, its reasoning effort and the filled-in judge prompt
(the template, question, criteria and answer). So:

- A new provider answers every scenario. A new scenario is answered by every provider.
- Editing a scenario's criteria or the judge prompt re-judges, but reuses the answers.
- Changing a provider's settings asks its questions again. Renaming it doesn't.
- Raising the sample count only fetches the new samples.
- Failed requests aren't cached, so running again retries them.

To throw everything away, delete `evals/.cache/`.

## Writing scenarios

The judge trusts the criteria over its own knowledge, so put the facts in the
criteria, along with what should cost points. Answers that change over time (opening
hours, current officeholders) need their criteria updated when they go stale.
