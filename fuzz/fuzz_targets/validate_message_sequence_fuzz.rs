#![no_main]

use libfuzzer_sys::fuzz_target;
use agent_base::engine::validate_message_sequence;
use agent_base::ChatMessage;

fuzz_target!(|data: &[u8]| {
    // Try to deserialize a Vec<ChatMessage> from arbitrary bytes
    if let Ok(messages) = serde_json::from_slice::<Vec<ChatMessage>>(data) {
        let _ = validate_message_sequence(&messages);
    }
});
