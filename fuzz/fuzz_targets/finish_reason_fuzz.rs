#![no_main]

use libfuzzer_sys::fuzz_target;
use agent_base::FinishReason;

fuzz_target!(|data: &str| {
    let _ = FinishReason::from_raw(Some(data));
    let _ = FinishReason::from_openai(Some(data));
    let _ = FinishReason::from_anthropic(Some(data));
    let _ = FinishReason::from_responses(Some(data));
});
