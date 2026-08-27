#![no_main]

use libfuzzer_sys::fuzz_target;
use agent_base::{AgentSession, SessionId};

fuzz_target!(|data: &str| {
    let mut session = AgentSession::new(SessionId::new(0));

    // Split by NUL: id\0name\0args
    let parts: Vec<&str> = data.split('\0').collect();
    if parts.len() >= 3 {
        let tool_calls = vec![(
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2..].join("\0"),
        )];
        session.push_assistant_tool_calls(&tool_calls, None, None);
    }
});
