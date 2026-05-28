use serde::de::DeserializeOwned;

/// Parses JSON objects of type `T` from a stream of text chunks.
///
/// It scans for objects inside a JSON array (by default) and yields each
/// fully-formed object as soon as braces are balanced. Useful when an LLM
/// streams a JSON plan and you want to display / process steps incrementally.
#[derive(Debug)]
pub struct StreamingJsonParser<T> {
    buffer: String,
    scan_offset: usize,
    items: Vec<T>,
    items_start_byte: usize,
    in_items: bool,
    in_string: bool,
    escape_next: bool,
    array_key: Option<String>,
}

impl<T: DeserializeOwned + Clone> StreamingJsonParser<T> {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            scan_offset: 0,
            items: Vec::new(),
            items_start_byte: 0,
            in_items: false,
            in_string: false,
            escape_next: false,
            array_key: None,
        }
    }

    /// Set the array key to look for. e.g. `with_key("steps")` will look for
    /// `"steps":[...]` in the JSON.
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.array_key = Some(key.into());
        self
    }

    /// Append a new chunk and return any newly parsed items.
    pub fn process_chunk(&mut self, chunk: &str) -> Vec<T> {
        let mut new_items = Vec::new();
        self.buffer.push_str(chunk);

        if !self.in_items {
            if let Some(pos) = self.find_items_array_start() {
                self.items_start_byte = pos + 1;
                self.scan_offset = 0;
                self.in_items = true;
            }
        }

        if self.in_items {
            new_items = self.extract_items();
            self.items.extend(new_items.clone());
        }

        new_items
    }

    /// Return all accumulated items so far.
    pub fn accumulated(&self) -> &[T] {
        &self.items
    }

    /// Consume parser and return the full raw text.
    pub fn into_buffer(self) -> String {
        self.buffer
    }

    fn find_items_array_start(&self) -> Option<usize> {
        if let Some(ref key) = self.array_key {
            if let Some(pos) = self.buffer.find(&format!("\"{}\"", key)) {
                let after = &self.buffer[pos..];
                if let Some(bracket_pos) = after.find('[') {
                    return Some(pos + bracket_pos);
                }
            }
        } else {
            // Fallback: look for any quoted key followed by '['
            if let Some(pos) = self.buffer.find('"') {
                let after = &self.buffer[pos..];
                if let Some(bracket_pos) = after.find('[') {
                    return Some(pos + bracket_pos);
                }
            }
        }
        // Last fallback: raw array
        self.buffer.find('[')
    }

    fn extract_items(&mut self) -> Vec<T> {
        let mut results = Vec::new();
        let slice = &self.buffer[self.items_start_byte..];
        let mut brace_depth: i32 = 0;
        let mut item_start_byte: Option<usize> = None;

        for (byte_offset, ch) in slice.char_indices().skip(self.scan_offset) {
            if self.escape_next {
                self.escape_next = false;
                self.scan_offset = byte_offset + ch.len_utf8();
                continue;
            }

            if self.in_string {
                if ch == '\\' {
                    self.escape_next = true;
                } else if ch == '"' {
                    self.in_string = false;
                }
                self.scan_offset = byte_offset + ch.len_utf8();
                continue;
            }

            match ch {
                '"' => self.in_string = true,
                '{' => {
                    if brace_depth == 0 {
                        let abs_byte = self.items_start_byte + byte_offset;
                        item_start_byte = Some(abs_byte);
                    }
                    brace_depth += 1;
                }
                '}' => {
                    brace_depth -= 1;
                    if brace_depth == 0 {
                        if let Some(start) = item_start_byte.take() {
                            let end = self.items_start_byte + byte_offset + ch.len_utf8();
                            let item_json = &self.buffer[start..end];
                            if let Ok(item) = serde_json::from_str::<T>(item_json) {
                                results.push(item);
                            }
                        }
                    }
                }
                _ => {}
            }

            self.scan_offset = byte_offset + ch.len_utf8();
        }

        results
    }
}

impl<T: DeserializeOwned + Clone> Default for StreamingJsonParser<T> {
    fn default() -> Self {
        Self::new()
    }
}
