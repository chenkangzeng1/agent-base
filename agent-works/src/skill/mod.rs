use std::collections::HashMap;
use std::sync::Arc;

use agent_base::{PlanItem, Tool};
use serde::Serialize;

pub mod apply_tool;
pub mod detail_tool;
pub mod prompter;
pub mod registry;

#[cfg(feature = "prompt_skill")]
pub mod prompt_skill;

#[cfg(feature = "yaml_skill")]
pub mod yaml_skill;

pub use apply_tool::ApplySkillTool;
pub use detail_tool::SkillDetailTool;
pub use prompter::{FullDetailPrompter, LazySkillPrompter};
pub use registry::{SkillRegistry, SkillSummary};

// ── Skill parameter types ──

/// Parameter type for template-based skills.
#[derive(Debug, Clone, Serialize)]
pub enum SkillParamType {
    String,
    Number,
    HostRef,
}

/// Parameter definition for a template-based skill.
#[derive(Debug, Clone, Serialize)]
pub struct SkillParam {
    pub name: String,
    pub description: String,
    pub param_type: SkillParamType,
    pub required: bool,
    pub default: Option<String>,
}

// ── Skill trait ──

pub trait Skill: Send + Sync {
    fn name(&self) -> &'static str;
    fn brief_description(&self) -> String;
    fn detailed_description(&self) -> String;

    /// Tools provided by knowledge-type skills.
    /// Template-type skills should return `vec![]`.
    fn tools(&self) -> Vec<Arc<dyn Tool>>;

    /// Generate PlanItems from this template skill with the given params.
    /// Template-type skills override this; knowledge-type skills return None.
    fn plan_steps(&self, _params: &HashMap<String, String>) -> Option<Vec<PlanItem>> {
        None
    }

    /// Parameter definitions for template-type skills.
    fn parameters(&self) -> &[SkillParam] {
        &[]
    }

    fn version(&self) -> &'static str {
        "0.1.0"
    }

    fn tags(&self) -> &[&'static str] {
        &[]
    }

    fn author(&self) -> &'static str {
        ""
    }

    fn category(&self) -> &'static str {
        ""
    }
}

// ── SkillPrompter trait ──

pub trait SkillPrompter: Send + Sync {
    /// Build the system prompt snippet for the given skills.
    /// `detail_tool_name` is the name of the tool the LLM can call to get
    /// detailed skill descriptions (used by `LazySkillPrompter`).
    fn build_prompt(&self, skills: &[Arc<dyn Skill>], detail_tool_name: &str) -> String;
}
