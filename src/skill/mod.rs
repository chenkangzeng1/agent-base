use std::sync::Arc;

use crate::tool::Tool;

pub mod detail_tool;
pub mod prompter;

pub(crate) use detail_tool::SkillDetailTool;
pub use prompter::{FullDetailPrompter, LazySkillPrompter};

/// Skill - a reusable capability unit
///
/// Each Skill declares:
/// - Brief description (resident in system prompt)
/// - Detailed description (loaded on demand)
/// - Tool collection (auto-registered)
pub trait Skill: Send + Sync {
    fn name(&self) -> &'static str;
    fn brief_description(&self) -> String;
    fn detailed_description(&self) -> String;
    fn tools(&self) -> Vec<Arc<dyn Tool>>;

    fn version(&self) -> &'static str {
        "0.1.0"
    }

    fn tags(&self) -> &[&'static str] {
        &[]
    }

    fn author(&self) -> &'static str {
        ""
    }
}

/// Prompt injection strategy
///
/// Generates prompt text to inject into the system prompt based on registered skills.
pub trait SkillPrompter: Send + Sync {
    fn build_prompt(&self, skills: &[Arc<dyn Skill>]) -> String;
}
