use std::sync::Arc;

use super::{Skill, SkillPrompter};

/// 按需加载策略（默认）
///
/// 只放 brief_description + 提示调用 get_skill_detail
pub struct LazySkillPrompter {
    title: String,
    instruction: String,
    item_prefix: String,
}

impl Default for LazySkillPrompter {
    fn default() -> Self {
        Self {
            title: "## 可用 Skills".to_string(),
            instruction:
                "> 需要某个 Skill 的详细操作指南时，调用 get_skill_detail 工具获取。"
                    .to_string(),
            item_prefix: "- **".to_string(),
        }
    }
}

impl LazySkillPrompter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn instruction(mut self, instruction: impl Into<String>) -> Self {
        self.instruction = instruction.into();
        self
    }

    pub fn item_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.item_prefix = prefix.into();
        self
    }
}

impl SkillPrompter for LazySkillPrompter {
    fn build_prompt(&self, skills: &[Arc<dyn Skill>]) -> String {
        if skills.is_empty() {
            return String::new();
        }

        let mut prompt = String::new();
        prompt.push_str(&self.title);
        prompt.push('\n');

        for skill in skills {
            prompt.push_str(&format!(
                "{}**{}**: {}\n",
                self.item_prefix,
                skill.name(),
                skill.brief_description()
            ));
        }

        prompt.push('\n');
        prompt.push_str(&self.instruction);

        prompt
    }
}

/// 全量注入策略
///
/// 把 brief + detailed 都塞进去
pub struct FullDetailPrompter;

impl SkillPrompter for FullDetailPrompter {
    fn build_prompt(&self, skills: &[Arc<dyn Skill>]) -> String {
        if skills.is_empty() {
            return String::new();
        }

        let mut prompt = String::from("## 可用 Skills\n\n");
        for skill in skills {
            prompt.push_str(&format!("### {}\n\n", skill.name()));
            prompt.push_str(&skill.brief_description());
            prompt.push_str("\n\n");
            prompt.push_str(&skill.detailed_description());
            prompt.push_str("\n\n---\n\n");
        }
        prompt
    }
}
