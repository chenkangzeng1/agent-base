//! Focus — A focused LLM call.
//!
//! Each Focus instance is bound to a system prompt and dedicated to one specific
//! judgment question. Callers only need to provide input and the expected return
//! type; the framework handles LLM invocation, JSON parsing, and timeouts.
//!
//! # Design Philosophy
//!
//! Throwing all judgments at the LLM at once overwhelms it. Especially for
//! generalized concerns, we need to decompose — the LLM should focus on one thing
//! at a time, keeping it simple enough to handle. If the business decomposition
//! is good, even a weak model can do one thing well.

mod core;

pub use core::{Context, Focus, FocusError, FocusInput, FocusOutput};
