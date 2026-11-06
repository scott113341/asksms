//! Format rules checked in code rather than by the judge, since they're fixed.

use asksms::llm::MAX_ANSWER_CHARS;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub passed: bool,
    /// Why it failed; empty when it passed.
    pub detail: String,
}

/// Runs every format check against an answer.
pub fn run(text: &str, truncated: bool) -> Vec<Check> {
    vec![
        check("not_empty", not_empty(text)),
        check("not_truncated", not_truncated(truncated)),
        check("length", length(text)),
        check("gsm7", gsm7(text)),
        check("no_markdown", no_markdown(text)),
        check("no_citations", no_citations(text)),
    ]
}

fn check(name: &'static str, result: Result<(), String>) -> Check {
    match result {
        Ok(()) => Check {
            name,
            passed: true,
            detail: String::new(),
        },
        Err(detail) => Check {
            name,
            passed: false,
            detail,
        },
    }
}

fn not_empty(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        Err("the answer is empty".to_owned())
    } else {
        Ok(())
    }
}

fn not_truncated(truncated: bool) -> Result<(), String> {
    if truncated {
        Err("cut off by the output-token limit".to_owned())
    } else {
        Ok(())
    }
}

fn length(text: &str) -> Result<(), String> {
    let len = text.chars().count();
    if len > MAX_ANSWER_CHARS {
        Err(format!("{len} chars, over the {MAX_ANSWER_CHARS} limit"))
    } else {
        Ok(())
    }
}

/// The GSM 03.38 basic character set, plus its extension table.
const GSM7: &str = "@£$¥èéùìòÇ\nØø\rÅåΔ_ΦΓΛΩΠΨΣΘΞÆæßÉ !\"#¤%&'()*+,-./0123456789:;<=>?\
    ¡ABCDEFGHIJKLMNOPQRSTUVWXYZÄÖÑÜ§¿abcdefghijklmnopqrstuvwxyzäöñüà\
    ^{}\\[~]|€\u{0C}";

/// Characters Twilio's Smart Encoding swaps for GSM-7 ones before sending
/// (curly quotes, dashes, ellipses and odd spaces), so they're allowed too.
const SMART_ENCODED: &str = "‘’‚‛“”„‟′″«»‐‑‒–—―…\u{00A0}\u{2002}\u{2003}\u{2009}\u{200A}\u{202F}";

fn gsm7(text: &str) -> Result<(), String> {
    let mut bad: Vec<char> = Vec::new();
    for c in text.chars() {
        if !GSM7.contains(c) && !SMART_ENCODED.contains(c) && !bad.contains(&c) {
            bad.push(c);
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    bad.truncate(10);
    let shown: Vec<String> = bad.iter().map(|c| format!("{c:?}")).collect();
    Err(format!("not GSM-7: {}", shown.join(" ")))
}

fn no_markdown(text: &str) -> Result<(), String> {
    let mut found = Vec::new();
    if text.contains("**") || text.contains("__") {
        found.push("bold");
    }
    if text.contains('`') {
        found.push("code");
    }
    if text.contains("](") {
        found.push("link");
    }
    let heading = text.lines().any(|line| {
        let hashes = line.trim_start().trim_start_matches('#');
        hashes.len() < line.trim_start().len() && hashes.starts_with(' ')
    });
    if heading {
        found.push("heading");
    }
    if found.is_empty() {
        Ok(())
    } else {
        Err(format!("Markdown: {}", found.join(", ")))
    }
}

/// Finds citation markers like `[1]` or `[12]`.
fn no_citations(text: &str) -> Result<(), String> {
    let markers: Vec<&str> = text
        .match_indices('[')
        .filter_map(|(start, _)| {
            let rest = &text[start + 1..];
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            (digits > 0 && rest[digits..].starts_with(']'))
                .then(|| &text[start..start + digits + 2])
        })
        .collect();
    if markers.is_empty() {
        Ok(())
    } else {
        Err(format!("citations: {}", markers.join(" ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failures(text: &str) -> Vec<&'static str> {
        run(text, false)
            .into_iter()
            .filter(|c| !c.passed)
            .map(|c| c.name)
            .collect()
    }

    #[test]
    fn a_plain_answer_passes() {
        assert!(failures("It opens at 11:00 AM (Sat). Price: $5 - €4.").is_empty());
    }

    #[test]
    fn smart_quotes_and_dashes_pass() {
        assert!(failures("It’s “open” 9–5…").is_empty());
    }

    #[test]
    fn catches_each_problem() {
        assert_eq!(failures(" "), ["not_empty"]);
        assert_eq!(
            run("It opens at", true)
                .iter()
                .filter(|c| !c.passed)
                .map(|c| c.name)
                .collect::<Vec<_>>(),
            ["not_truncated"]
        );
        assert_eq!(failures(&"a".repeat(801)), ["length"]);
        assert_eq!(failures("Sure! 😀"), ["gsm7"]);
        assert_eq!(failures("It is **open**"), ["no_markdown"]);
        assert_eq!(failures("## Hours\nOpen"), ["no_markdown"]);
        assert_eq!(failures("See [here](http://x)"), ["no_markdown"]);
        assert_eq!(failures("Open at 11 [1][2]."), ["no_citations"]);
    }

    #[test]
    fn hashtags_and_brackets_are_not_flagged() {
        assert!(failures("#1 seed. Array [a] and [12a].").is_empty());
    }

    #[test]
    fn describes_failures() {
        let checks = run("Hi 😀 [3]", false);
        let gsm = checks.iter().find(|c| c.name == "gsm7").unwrap();
        assert_eq!(gsm.detail, "not GSM-7: '😀'");
        let cites = checks.iter().find(|c| c.name == "no_citations").unwrap();
        assert_eq!(cites.detail, "citations: [3]");
    }
}
