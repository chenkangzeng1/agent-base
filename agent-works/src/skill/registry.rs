use std::collections::HashMap;
use std::sync::Arc;

use agent_base::{AgentError, AgentResult, UpdatePlanArgs};
use tokio::sync::RwLock;

use super::{Skill, SkillParam};

/// Lightweight summary of a skill — returned by `list()`.
/// Excludes `detailed_description` and `plan_steps` to save tokens.
#[derive(Clone, serde::Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub param_defs: Vec<SkillParam>,
    pub has_plan: bool,
    pub version: String,
    pub category: String,
    pub author: String,
}

/// Runtime registry for skills. Supports dynamic registration,
/// unlike `AgentBuilder` which only registers at build time.
pub struct SkillRegistry {
    skills: RwLock<Vec<Arc<dyn Skill>>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: RwLock::new(Vec::new()),
        }
    }

    /// Register a skill. If a skill with the same name exists, it's replaced.
    pub async fn register(&self, skill: Arc<dyn Skill>) {
        let mut skills = self.skills.write().await;
        skills.retain(|s| s.name() != skill.name());
        skills.push(skill);
    }

    /// List all skills as summaries (brief descriptions only).
    pub async fn list(&self) -> Vec<SkillSummary> {
        let skills = self.skills.read().await;
        skills
            .iter()
            .map(|s| SkillSummary {
                name: s.name().to_string(),
                description: s.brief_description(),
                tags: s.tags().iter().map(|t| t.to_string()).collect(),
                param_defs: s.parameters().to_vec(),
                has_plan: s.plan_steps(&HashMap::new()).is_some(),
                version: s.version().to_string(),
                category: s.category().to_string(),
                author: s.author().to_string(),
            })
            .collect()
    }

    /// Get a skill by name (full info including detailed_description).
    pub async fn get(&self, name: &str) -> Option<Arc<dyn Skill>> {
        let skills = self.skills.read().await;
        skills.iter().find(|s| s.name() == name).cloned()
    }

    /// Apply a template skill: substitute parameters → generate plan args.
    /// Returns `None` if the skill doesn't exist or is not a template skill.
    pub async fn apply(
        &self,
        name: &str,
        params: &HashMap<String, String>,
    ) -> AgentResult<Option<UpdatePlanArgs>> {
        let skill = match self.get(name).await {
            Some(s) => s,
            None => return Ok(None),
        };

        // Validate required parameters
        for param in skill.parameters() {
            if param.required && !params.contains_key(&param.name) {
                return Err(AgentError::internal(format!(
                    "Skill '{}': missing required parameter '{}'",
                    name, param.name
                )));
            }
        }

        // Generate plan items via template expansion
        let steps = match skill.plan_steps(params) {
            Some(s) => s,
            None => return Ok(None), // Not a template skill
        };

        if steps.is_empty() {
            return Err(AgentError::internal(format!(
                "Skill '{}' generated empty plan steps",
                name
            )));
        }

        let objective = format!("{}: {}", name, skill.brief_description());

        let plan = UpdatePlanArgs {
            objective: Some(objective),
            explanation: Some(format!("从技能模板 '{}' 生成", name)),
            plan: steps,
        };

        Ok(Some(plan))
    }

    /// Remove a skill by name.
    pub async fn remove(&self, name: &str) {
        let mut skills = self.skills.write().await;
        skills.retain(|s| s.name() != name);
    }

    /// Number of registered skills.
    pub async fn count(&self) -> usize {
        self.skills.read().await.len()
    }

    /// Get all skills as full `Arc<dyn Skill>` for use with
    /// `SkillDetailTool` and `SkillPrompter`.
    pub async fn all_skills(&self) -> Vec<Arc<dyn Skill>> {
        self.skills.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::{Skill, SkillParam, SkillParamType};
    use std::sync::Arc;

    struct TestSkill {
        name: &'static str,
        desc: &'static str,
    }

    impl Skill for TestSkill {
        fn name(&self) -> &'static str {
            self.name
        }
        fn brief_description(&self) -> String {
            self.desc.to_string()
        }
        fn detailed_description(&self) -> String {
            format!("Detailed: {}", self.desc)
        }
        fn tools(&self) -> Vec<Arc<dyn agent_base::Tool>> {
            vec![]
        }
    }

    #[tokio::test]
    async fn test_register_and_list() {
        let registry = SkillRegistry::new();
        registry
            .register(Arc::new(TestSkill {
                name: "test-skill",
                desc: "A test skill",
            }))
            .await;

        let list = registry.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "test-skill");
        assert_eq!(list[0].description, "A test skill");
        assert!(!list[0].has_plan);
    }

    #[tokio::test]
    async fn test_get() {
        let registry = SkillRegistry::new();
        registry
            .register(Arc::new(TestSkill {
                name: "test-skill",
                desc: "A test skill",
            }))
            .await;

        let skill = registry.get("test-skill").await;
        assert!(skill.is_some());
        assert_eq!(skill.unwrap().name(), "test-skill");

        assert!(registry.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_register_replace() {
        let registry = SkillRegistry::new();
        registry
            .register(Arc::new(TestSkill {
                name: "test-skill",
                desc: "v1",
            }))
            .await;
        registry
            .register(Arc::new(TestSkill {
                name: "test-skill",
                desc: "v2",
            }))
            .await;

        let list = registry.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].description, "v2");
    }

    #[tokio::test]
    async fn test_remove() {
        let registry = SkillRegistry::new();
        registry
            .register(Arc::new(TestSkill {
                name: "test-skill",
                desc: "test",
            }))
            .await;
        assert_eq!(registry.count().await, 1);
        registry.remove("test-skill").await;
        assert_eq!(registry.count().await, 0);
    }

    #[tokio::test]
    async fn test_apply_nonexistent() {
        let registry = SkillRegistry::new();
        let result = registry
            .apply("nonexistent", &HashMap::new())
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_apply_knowledge_skill_returns_none() {
        let registry = SkillRegistry::new();
        registry
            .register(Arc::new(TestSkill {
                name: "knowledge-skill",
                desc: "A knowledge skill",
            }))
            .await;
        let result = registry
            .apply("knowledge-skill", &HashMap::new())
            .await
            .unwrap();
        assert!(result.is_none());
    }
}
