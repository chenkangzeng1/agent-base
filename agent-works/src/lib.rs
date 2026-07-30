#[cfg(feature = "mcp")]
pub mod mcp;

#[cfg(feature = "skill")]
pub mod skill;

#[cfg(feature = "builtin-tools")]
pub mod builtin;

#[cfg(feature = "cli")]
pub mod cli;

pub mod focus;

mod builder;
pub mod handle;

pub use crate::builder::AgentBuilder;
pub use crate::handle::{AgentHandle, SendError};

#[cfg(feature = "skill")]
pub use skill::{
    ApplySkillTool, FullDetailPrompter, LazySkillPrompter, Skill, SkillDetailTool, SkillParam,
    SkillParamType, SkillRegistry, SkillSummary,
};

#[cfg(feature = "prompt_skill")]
pub use skill::prompt_skill::PromptSkill;

#[cfg(feature = "yaml_skill")]
pub use skill::yaml_skill::YamlSkill;
