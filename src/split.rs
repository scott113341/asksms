/// Maximum length of a single SMS segment.
pub const SMS_LENGTH: usize = 160;

/// Room reserved at the start of each part for its "1/2: " prefix.
const PREFIX_RESERVE: usize = 5;

/// Splits `message` into parts of at most `max_length` characters, breaking on
/// whitespace and prefixing each part with "index/total: ". A message that already
/// fits is returned unchanged, without a prefix.
pub fn split_into_messages(message: &str, max_length: usize) -> Vec<String> {
    if message.chars().count() <= max_length {
        return vec![message.to_owned()];
    }

    let limit = max_length.saturating_sub(PREFIX_RESERVE);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0;

    for word in message.split_whitespace() {
        let word_len = word.chars().count();
        if current_len + 1 + word_len <= limit {
            current.push(' ');
            current.push_str(word);
            current_len += 1 + word_len;
        } else {
            push_chunk(&mut chunks, &current);
            current = word.to_owned();
            current_len = word_len;
        }
    }
    push_chunk(&mut chunks, &current);

    let total = chunks.len();
    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| format!("{}/{total}: {chunk}", i + 1))
        .collect()
}

fn push_chunk(chunks: &mut Vec<String>, chunk: &str) {
    let chunk = chunk.trim();
    if !chunk.is_empty() {
        chunks.push(chunk.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_original_message_when_short_enough() {
        assert_eq!(
            split_into_messages("Short message", SMS_LENGTH),
            vec!["Short message"]
        );
    }

    #[test]
    fn splits_long_messages_into_multiple_chunks() {
        let result = split_into_messages(&"a ".repeat(100), SMS_LENGTH);
        assert_eq!(result.len(), 2);
        assert!(result[0].starts_with("1/2: a"));
        assert!(result[1].starts_with("2/2: a"));
    }

    #[test]
    fn preserves_word_boundaries() {
        let message = "this message is 300 long ".repeat(12);
        assert_eq!(message.chars().count(), 300);

        let result = split_into_messages(&message, SMS_LENGTH);
        assert_eq!(result.len(), 2);
        assert!(result[0].starts_with("1/2: this message is 300 long"));
        assert!(result[1].starts_with("2/2: message is 300 long this"));
        assert!(result.iter().all(|m| m.chars().count() <= SMS_LENGTH));
    }

    #[test]
    fn counts_characters_not_bytes() {
        // 160 characters but 320 bytes: still fits in one message.
        let message = "é".repeat(160);
        assert_eq!(split_into_messages(&message, SMS_LENGTH), vec![message]);
    }

    #[test]
    fn does_not_emit_an_empty_part_for_a_long_first_word() {
        let message = format!("{} end", "x".repeat(170));
        let result = split_into_messages(&message, SMS_LENGTH);
        assert_eq!(result.len(), 2);
        assert!(result[0].starts_with("1/2: xxx"));
        assert_eq!(result[1], "2/2: end");
    }
}
