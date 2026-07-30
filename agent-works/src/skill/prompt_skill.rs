//! PromptSkill: implements `Skill` from a Markdown file with YAML frontmatter.
//!
//! Prompt Skills are lightweight instruction files that get injected into the
//! system prompt to guide AI behavior. Compatible with Trae/Cursor/Claude rules format.

use std::sync::Arc;

use crate::skill::{Skill, SkillParam};
use serde::Deserialize;

// ── Frontmatter schema ──

#[derive(Debug, Clone, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
}

// ── PromptSkill ──

/// A skill loaded from a Markdown file with YAML frontmatter.
///
/// Implements `Skill` — can be registered into `SkillRegistry`
/// and used for system prompt injection.
pub struct PromptSkill {
    frontmatter: SkillFrontmatter,
    /// The full Markdown content (including frontmatter)
    content: String,
    /// The Markdown body (excluding frontmatter)
    body: String,
    /// Leaked name for &'static str return
    static_name: &'static str,
    /// Leaked version for &'static str return
    static_version: &'static str,
    /// Leaked category for &'static str return
    static_category: &'static str,
    /// Leaked author for &'static str return
    static_author: &'static str,
    /// Leaked tags for &'static [&'static str] return
    static_tags: Vec<&'static str>,
}

impl PromptSkill {
    /// Parse a Markdown string with YAML frontmatter into a `PromptSkill`.
    ///
    /// Expected format:
    /// ```markdown
    /// ---
    /// name: my-skill
    /// description: A short description
    /// category: 运维
    /// tags: [tag1, tag2]
    /// version: "1.0"
    /// author: ops-agent
    /// ---
    ///
    /// # Skill content here
    /// ```
    pub fn from_markdown(content: &str) -> Result<Self, String> {
        let (frontmatter_str, body) = split_frontmatter(content)?;

        let frontmatter: SkillFrontmatter = serde_yaml::from_str(frontmatter_str)
            .map_err(|e| format!("Frontmatter parse error: {e}"))?;

        if frontmatter.name.is_empty() {
            return Err("Skill name is empty".to_string());
        }

        if frontmatter.description.is_empty() {
            return Err("Skill description is empty".to_string());
        }

        // Validate name format (alphanumeric + hyphens)
        if !frontmatter
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!(
                "Invalid skill name '{}': only alphanumeric, hyphens, and underscores allowed",
                frontmatter.name
            ));
        }

        let static_name: &'static str = Box::leak(frontmatter.name.clone().into_boxed_str());
        let static_version: &'static str = Box::leak(
            frontmatter
                .version
                .clone()
                .unwrap_or_else(|| "1.0".to_string())
                .into_boxed_str(),
        );
        let static_category: &'static str =
            Box::leak(frontmatter.category.clone().into_boxed_str());
        let static_author: &'static str = Box::leak(
            frontmatter
                .author
                .clone()
                .unwrap_or_default()
                .into_boxed_str(),
        );
        let static_tags: Vec<&'static str> = frontmatter
            .tags
            .iter()
            .map(|t| Box::leak(t.clone().into_boxed_str()) as &'static str)
            .collect();

        Ok(Self {
            frontmatter,
            content: content.to_string(),
            body: body.to_string(),
            static_name,
            static_version,
            static_category,
            static_author,
            static_tags,
        })
    }

    /// The raw frontmatter (for introspection).
    pub fn frontmatter(&self) -> &SkillFrontmatter {
        &self.frontmatter
    }

    /// The Markdown body (without frontmatter).
    pub fn body(&self) -> &str {
        &self.body
    }
}

/// Split a Markdown file into (frontmatter, body) at the `---` delimiters.
fn split_frontmatter(content: &str) -> Result<(&str, &str), String> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        return Err("Missing YAML frontmatter (file must start with ---)".to_string());
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let closing = after_first
        .find("\n---")
        .or_else(|| after_first.find("\r\n---"))
        .ok_or("Missing closing --- for frontmatter")?;

    let frontmatter_str = &after_first[..closing];
    let body_start = closing + 4; // skip past "\n---"
    let body = after_first[body_start..]
        .trim_start_matches('\n')
        .trim_start_matches('\r');

    Ok((frontmatter_str, body))
}

impl Skill for PromptSkill {
    fn name(&self) -> &'static str {
        self.static_name
    }

    fn brief_description(&self) -> String {
        self.frontmatter.description.clone()
    }

    fn detailed_description(&self) -> String {
        self.content.clone()
    }

    fn tools(&self) -> Vec<Arc<dyn agent_base::Tool>> {
        vec![]
    }

    fn parameters(&self) -> &[SkillParam] {
        &[]
    }

    fn version(&self) -> &'static str {
        self.static_version
    }

    fn tags(&self) -> &[&'static str] {
        &self.static_tags
    }

    fn author(&self) -> &'static str {
        self.static_author
    }

    fn category(&self) -> &'static str {
        self.static_category
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PROMPT_SKILL: &str = r#"---
name: test-prompt
description: A test prompt skill
category: 测试
tags: [test, example]
version: "1.0"
author: test
---

# Test Prompt Skill

## 角色
你是一个测试助手。

## 指导
- 始终用中文回复
- 保持简洁
"#;

    #[test]
    fn test_parse_markdown() {
        let skill = PromptSkill::from_markdown(TEST_PROMPT_SKILL).unwrap();
        assert_eq!(skill.name(), "test-prompt");
        assert_eq!(skill.brief_description(), "A test prompt skill");
        assert_eq!(skill.category(), "测试");
        assert_eq!(skill.version(), "1.0");
        assert_eq!(skill.author(), "test");
        assert_eq!(skill.tags().len(), 2);
        assert!(skill.body().contains("# Test Prompt Skill"));
    }

    #[test]
    fn test_full_content() {
        let skill = PromptSkill::from_markdown(TEST_PROMPT_SKILL).unwrap();
        let desc = skill.detailed_description();
        assert!(desc.contains("---"));
        assert!(desc.contains("name: test-prompt"));
        assert!(desc.contains("# Test Prompt Skill"));
    }

    #[test]
    fn test_no_frontmatter() {
        let md = "# Just a heading\nSome content";
        assert!(PromptSkill::from_markdown(md).is_err());
    }

    #[test]
    fn test_empty_name() {
        let md = "---\nname: ''\ndescription: test\n---\nContent";
        assert!(PromptSkill::from_markdown(md).is_err());
    }

    #[test]
    fn test_empty_description() {
        let md = "---\nname: test-skill\ndescription: ''\n---\nContent";
        assert!(PromptSkill::from_markdown(md).is_err());
    }

    #[test]
    fn test_invalid_name() {
        let md = "---\nname: invalid name!\ndescription: test\n---\nContent";
        assert!(PromptSkill::from_markdown(md).is_err());
    }

    #[test]
    fn test_minimal_frontmatter() {
        let md = "---\nname: minimal\ndescription: Minimal skill\n---\nBody here";
        let skill = PromptSkill::from_markdown(md).unwrap();
        assert_eq!(skill.name(), "minimal");
        assert_eq!(skill.version(), "1.0"); // default
        assert_eq!(skill.author(), ""); // default
        assert_eq!(skill.category(), ""); // default
        assert!(skill.tags().is_empty());
    }
}
