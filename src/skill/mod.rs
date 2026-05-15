use std::sync::Arc;

use crate::tool::Tool;

pub mod detail_tool;
pub mod prompter;

pub(crate) use detail_tool::SkillDetailTool;
pub use prompter::{FullDetailPrompter, LazySkillPrompter};

/// Skill — 可复用的能力单元
///
/// 每个 Skill 声明：
/// - 简要描述（常驻 system prompt）
/// - 详细描述（按需加载）
/// - 工具集合（自动注册）
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

/// 提示词注入策略
///
/// 根据已注册的 skills，生成要注入 system prompt 的文本。
pub trait SkillPrompter: Send + Sync {
    fn build_prompt(&self, skills: &[Arc<dyn Skill>]) -> String;
}
