use crate::types::ChatMessage;

#[derive(Clone, Debug)]
pub struct ContextWindowManager {
    pub max_tokens: usize,
    /// Always keep first N messages (typically system prompt)
    pub keep_first_n: usize,
    /// Always keep last N messages
    pub keep_last_n: usize,
}

impl Default for ContextWindowManager {
    fn default() -> Self {
        Self {
            max_tokens: 128_000,
            keep_first_n: 1,
            keep_last_n: 20,
        }
    }
}

impl ContextWindowManager {
    /// OpenAI Vision API fixed token overhead per image
    const IMAGE_OVERHEAD_TOKENS: usize = 85;

    pub fn new(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            ..Default::default()
        }
    }

    pub fn with_keep_first_n(mut self, n: usize) -> Self {
        self.keep_first_n = n;
        self
    }

    pub fn with_keep_last_n(mut self, n: usize) -> Self {
        self.keep_last_n = n;
        self
    }

    /// Simple token estimation: ~4 chars/token for Latin, ~1.5 for CJK
    /// Mixed text uses a compromise of 3 chars/token
    pub fn estimate_tokens(text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        let chars = text.chars().count();
        let cjk_count = text.chars().filter(|c| is_cjk(*c)).count();
        let latin_count = chars - cjk_count;
        // CJK: ~1.5 chars/token, Latin: ~4 chars/token
        (cjk_count as f64 / 1.5 + latin_count as f64 / 4.0).ceil() as usize
    }

    fn message_tokens(msg: &ChatMessage) -> usize {
        match msg {
            ChatMessage::System { content } => Self::estimate_tokens(content),
            ChatMessage::User { content, images } => {
                let mut tokens = Self::estimate_tokens(content);
                for img in images {
                    match img {
                        crate::types::ImageAttachment::Url { url, detail: _ } => {
                            tokens += Self::estimate_tokens(url);
                        }
                        crate::types::ImageAttachment::Base64 { data, media_type, detail: _ } => {
                            tokens += data.len() / 4;
                            if let Some(mt) = media_type {
                                tokens += Self::estimate_tokens(mt);
                            }
                        }
                    }
                    tokens += Self::IMAGE_OVERHEAD_TOKENS;
                }
                tokens
            }
            ChatMessage::Assistant { content, reasoning_content: _, tool_calls } => {
                let mut tokens = content
                    .as_deref()
                    .map(|c| Self::estimate_tokens(c))
                    .unwrap_or(0);
                if let Some(tc) = tool_calls {
                    for t in tc {
                        tokens += Self::estimate_tokens(&t.name);
                        tokens += Self::estimate_tokens(&t.arguments);
                        tokens += Self::estimate_tokens(&t.id);
                    }
                }
                tokens
            }
            ChatMessage::Tool { tool_call_id, content } => {
                Self::estimate_tokens(tool_call_id) + Self::estimate_tokens(content)
            }
        }
    }

    /// Trim message list to keep total tokens under `max_tokens`。
    ///
    /// Trimming strategy:
    /// - Always keep the first `keep_first_n` messages (typically system prompt)
    /// - Always keep the last `keep_last_n` messages (recent conversation)
    /// - Remove oldest messages from the middle until within budget
    pub fn trim(&self, messages: &mut Vec<ChatMessage>) {
        if messages.is_empty() || self.max_tokens == 0 {
            return;
        }

        let total_tokens: usize = messages.iter().map(|m| Self::message_tokens(m)).sum();
        if total_tokens <= self.max_tokens {
            return;
        }

        let keep_first = self.keep_first_n.min(messages.len());
        let keep_last = self.keep_last_n.min(messages.len().saturating_sub(keep_first));

        // Trimmable range: [keep_first, messages.len() - keep_last)
        let trim_start = keep_first;
        let trim_end = messages.len().saturating_sub(keep_last);
        if trim_start >= trim_end {
            return;
        }

        let mut current_tokens: usize = total_tokens;
        let remove_idx = trim_start;

        while current_tokens > self.max_tokens && remove_idx < trim_end {
            let removed = Self::message_tokens(&messages[remove_idx]);
            messages.remove(remove_idx);
            current_tokens = current_tokens.saturating_sub(removed);
            // Do not increment remove_idx because remove shifts elements down
            let new_trim_end = messages.len().saturating_sub(keep_last);
            if remove_idx >= new_trim_end {
                break;
            }
        }
    }
}

fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Unified Ideographs Extension A
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth and Fullwidth Forms
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(ContextWindowManager::estimate_tokens(""), 0);
    }

    #[test]
    fn test_estimate_tokens_english() {
        let text = "Hello world this is a test";
        let tokens = ContextWindowManager::estimate_tokens(text);
        // ~28 chars / 4 ≈ 7
        assert!(tokens > 0 && tokens <= 15);
    }

    #[test]
    fn test_trim_no_trim_needed() {
        let mgr = ContextWindowManager::new(1000);
        let mut msgs = vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("Hello"),
            ChatMessage::assistant("Hi there!"),
        ];
        let original_len = msgs.len();
        mgr.trim(&mut msgs);
        assert_eq!(msgs.len(), original_len);
    }

    #[test]
    fn test_trim_keeps_first_and_last() {
        let mgr = ContextWindowManager::new(8)
            .with_keep_first_n(1)
            .with_keep_last_n(2);
        let mut msgs = vec![
            ChatMessage::system("system"),
            ChatMessage::user("message number one"),
            ChatMessage::assistant("message number two"),
            ChatMessage::user("message number three"),
            ChatMessage::assistant("message number four"),
            ChatMessage::user("message number five"),
            ChatMessage::assistant("message number six"),
        ];
        mgr.trim(&mut msgs);
        assert_eq!(msgs.len(), 3);
        assert!(matches!(msgs[0], ChatMessage::System { .. }));
    }
}
