/// Removes Qwen3-ASR wrappers such as `language English<asr_text>...`.
pub fn clean_text(raw: &str) -> String {
    let text = strip_asr_text(raw);
    let text = strip_language_prefix(text);
    let text = strip_nul(text);
    trim_edges(text).to_string()
}

fn strip_asr_text(raw: &str) -> &str {
    let mut text = raw;
    if let Some(index) = text.find("<asr_text>") {
        text = &text[index + "<asr_text>".len()..];
    }
    if let Some(index) = text.find("</asr_text>") {
        text = &text[..index];
    }
    text
}

fn strip_language_prefix(raw: &str) -> &str {
    let trimmed = raw.trim_start();
    let prefix = "language ";
    if trimmed
        .get(..prefix.len())
        .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
    {
        let rest = &trimmed[prefix.len()..];
        if let Some(index) = rest.find('\n') {
            return rest[index + 1..].trim_start();
        }
        let mut end = 0;
        for (index, character) in rest.char_indices() {
            if character.is_whitespace() || character == '<' {
                end = index;
                break;
            }
            end = index + character.len_utf8();
        }
        return rest[end..].trim_start();
    }
    trimmed
}

fn strip_nul(raw: &str) -> &str {
    match raw.find('\0') {
        Some(index) => &raw[..index],
        None => raw,
    }
}

fn trim_edges(raw: &str) -> &str {
    raw.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_asr_text_wrapper() {
        assert_eq!(
            clean_text("language English<asr_text>And so, hello.</asr_text>"),
            "And so, hello."
        );
    }

    #[test]
    fn keeps_model_punctuation_and_case() {
        assert_eq!(
            clean_text("language zh<asr_text>你好，世界。"),
            "你好，世界。"
        );
    }

    #[test]
    fn empty_output_stays_empty() {
        assert_eq!(clean_text("language English<asr_text></asr_text>"), "");
    }

    #[test]
    fn multibyte_text_does_not_panic_on_prefix_check() {
        assert_eq!(clean_text("现在开始测试。"), "现在开始测试。");
        assert_eq!(
            clean_text("现在开始测试。 language zh"),
            "现在开始测试。 language zh"
        );
    }
}
