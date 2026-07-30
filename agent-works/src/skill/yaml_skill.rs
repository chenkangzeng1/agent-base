//! YamlSkill: implements `Skill` from a YAML definition.
//!
//! Supports both built-in (embedded at compile time) and user-defined
//! (loaded from disk at runtime) skills.

use std::collections::HashMap;
use std::sync::Arc;

use crate::skill::{Skill, SkillParam, SkillParamType};
use agent_base::{PlanItem, PlanStepStatus};
use serde::Deserialize;

// ── YAML schema ──

#[derive(Debug, Deserialize)]
pub struct SkillDef {
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
    #[serde(default)]
    pub long_description: Option<String>,
    #[serde(default)]
    pub parameters: Vec<SkillParamDef>,
    #[serde(default)]
    pub phases: Vec<SkillPhaseDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillParamDef {
    pub name: String,
    pub description: String,
    #[serde(rename = "type", default = "default_param_type")]
    pub param_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
}

fn default_param_type() -> String {
    "string".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillPhaseDef {
    pub title: String,
    #[serde(default)]
    pub steps: Vec<SkillStepDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillStepDef {
    pub description: String,
    pub tool_call: SkillToolCallDef,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillToolCallDef {
    pub tool_name: String,
    pub args: serde_json::Value,
}

// ── YamlSkill ──

/// A skill loaded from a YAML definition.
///
/// Implements `Skill` — can be registered into `SkillRegistry`
/// and invoked via `ApplySkillTool`.
pub struct YamlSkill {
    def: SkillDef,
    /// Cached parameter definitions (converted from YAML schema)
    params: Vec<SkillParam>,
    /// Leaked name for &'static str return
    static_name: &'static str,
    /// Leaked tags for &'static [&'static str] return
    static_tags: Vec<&'static str>,
    /// Leaked version for &'static str return
    static_version: &'static str,
    /// Leaked author for &'static str return
    static_author: &'static str,
    /// Leaked category for &'static str return
    static_category: &'static str,
}

impl YamlSkill {
    /// Parse a YAML string into a `YamlSkill`.
    pub fn from_yaml(yaml: &str) -> Result<Self, String> {
        let def: SkillDef =
            serde_yaml::from_str(yaml).map_err(|e| format!("YAML parse error: {e}"))?;

        if def.name.is_empty() {
            return Err("Skill name is empty".to_string());
        }

        let params: Vec<SkillParam> = def
            .parameters
            .iter()
            .map(|p| SkillParam {
                name: p.name.clone(),
                description: p.description.clone(),
                param_type: match p.param_type.as_str() {
                    "number" => SkillParamType::Number,
                    "host_ref" => SkillParamType::HostRef,
                    _ => SkillParamType::String,
                },
                required: p.required,
                default: p.default.clone(),
            })
            .collect();

        let static_name: &'static str = Box::leak(def.name.clone().into_boxed_str());
        let static_tags: Vec<&'static str> = def
            .tags
            .iter()
            .map(|t| Box::leak(t.clone().into_boxed_str()) as &'static str)
            .collect();
        let static_version: &'static str = Box::leak(
            def.version
                .clone()
                .unwrap_or_else(|| "1.0".to_string())
                .into_boxed_str(),
        );
        let static_author: &'static str =
            Box::leak(def.author.clone().unwrap_or_default().into_boxed_str());
        let static_category: &'static str = Box::leak(def.category.clone().into_boxed_str());

        Ok(Self {
            def,
            params,
            static_name,
            static_tags,
            static_version,
            static_author,
            static_category,
        })
    }

    /// The raw skill definition (for introspection / editing).
    pub fn definition(&self) -> &SkillDef {
        &self.def
    }

    /// Substitute `{{var}}` placeholders in a string with parameter values.
    fn substitute(
        &self,
        template: &str,
        params: &HashMap<String, String>,
    ) -> Result<String, String> {
        let mut result = template.to_string();
        for (key, value) in params {
            let placeholder = format!("{{{{{}}}}}", key);
            if result.contains(&placeholder) {
                result = result.replace(&placeholder, value);
            }
        }
        // Check for unresolved placeholders
        if let Some(pos) = result.find("{{") {
            if let Some(end) = result[pos..].find("}}") {
                let unresolved = &result[pos..pos + end + 2];
                return Err(format!("Unresolved template variable: {}", unresolved));
            }
        }
        Ok(result)
    }

    /// Substitute template variables in a JSON Value (recursively).
    fn substitute_json(
        &self,
        value: &serde_json::Value,
        params: &HashMap<String, String>,
    ) -> Result<serde_json::Value, String> {
        match value {
            serde_json::Value::String(s) => {
                Ok(serde_json::Value::String(self.substitute(s, params)?))
            }
            serde_json::Value::Object(map) => {
                let mut new_map = serde_json::Map::new();
                for (k, v) in map {
                    new_map.insert(k.clone(), self.substitute_json(v, params)?);
                }
                Ok(serde_json::Value::Object(new_map))
            }
            serde_json::Value::Array(arr) => {
                let new_arr: Result<Vec<_>, _> = arr
                    .iter()
                    .map(|v| self.substitute_json(v, params))
                    .collect();
                Ok(serde_json::Value::Array(new_arr?))
            }
            _ => Ok(value.clone()),
        }
    }
}

impl Skill for YamlSkill {
    fn name(&self) -> &'static str {
        self.static_name
    }

    fn brief_description(&self) -> String {
        self.def.description.clone()
    }

    fn detailed_description(&self) -> String {
        self.def
            .long_description
            .clone()
            .unwrap_or_else(|| self.def.description.clone())
    }

    fn tools(&self) -> Vec<Arc<dyn agent_base::Tool>> {
        // Template-type skills don't provide tools; they generate plan_steps.
        vec![]
    }

    fn plan_steps(&self, params: &HashMap<String, String>) -> Option<Vec<PlanItem>> {
        if self.def.phases.is_empty() {
            return None;
        }

        let mut steps = Vec::new();
        for phase in &self.def.phases {
            for step_def in &phase.steps {
                let description = match self.substitute(&step_def.description, params) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(
                            skill = self.def.name,
                            error = e,
                            "template substitution failed"
                        );
                        return None;
                    }
                };

                steps.push(PlanItem {
                    step: description,
                    status: PlanStepStatus::Pending,
                });
            }
        }

        if steps.is_empty() { None } else { Some(steps) }
    }

    fn parameters(&self) -> &[SkillParam] {
        &self.params
    }

    fn tags(&self) -> &[&'static str] {
        &self.static_tags
    }

    fn version(&self) -> &'static str {
        self.static_version
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

    const TEST_SKILL: &str = r#"
name: test_skill
description: A test skill
category: test
version: "2.0"
author: ops-team
tags: [example, template]
long_description: |
  ## Test
  This is a test skill for unit testing.

parameters:
  - name: target_host
    description: Target host
    type: host_ref
    required: true
  - name: service_name
    description: Service name
    type: string

phases:
  - title: Check service
    steps:
      - description: Check if {{service_name}} is running on {{target_host}}
        tool_call:
          tool_name: execute_command
          args:
            command: "ps aux | grep {{service_name}}"
            target_host: "{{target_host}}"
"#;

    #[test]
    fn test_parse_yaml() {
        let skill = YamlSkill::from_yaml(TEST_SKILL).unwrap();
        assert_eq!(skill.name(), "test_skill");
        assert_eq!(skill.brief_description(), "A test skill");
        assert_eq!(skill.version(), "2.0");
        assert_eq!(skill.author(), "ops-team");
        assert_eq!(skill.category(), "test");
        assert_eq!(skill.tags(), &["example", "template"]);
        assert!(skill.detailed_description().contains("## Test"));
        assert_eq!(skill.parameters().len(), 2);
    }

    #[test]
    fn test_plan_steps() {
        let skill = YamlSkill::from_yaml(TEST_SKILL).unwrap();
        let mut params = HashMap::new();
        params.insert("target_host".to_string(), "prod-1".to_string());
        params.insert("service_name".to_string(), "nginx".to_string());

        let steps = skill.plan_steps(&params).unwrap();
        assert_eq!(steps.len(), 1);
        assert!(steps[0].step.contains("nginx"));
        assert!(steps[0].step.contains("prod-1"));
    }

    #[test]
    fn test_plan_steps_missing_param() {
        let skill = YamlSkill::from_yaml(TEST_SKILL).unwrap();
        let params = HashMap::new(); // missing required params
        // Template substitution returns None when placeholders are unresolved
        assert!(skill.plan_steps(&params).is_none());
    }

    #[test]
    fn test_no_phases_returns_none() {
        let yaml = r#"
name: knowledge_skill
description: A knowledge skill
"#;
        let skill = YamlSkill::from_yaml(yaml).unwrap();
        assert!(skill.plan_steps(&HashMap::new()).is_none());
        assert_eq!(skill.version(), "1.0");
        assert_eq!(skill.author(), "");
        assert_eq!(skill.category(), "");
        assert!(skill.tags().is_empty());
    }

    #[test]
    fn test_invalid_yaml() {
        assert!(YamlSkill::from_yaml("not: [valid: yaml").is_err());
    }

    #[test]
    fn test_empty_name() {
        let yaml = "name: ''\ndescription: test";
        assert!(YamlSkill::from_yaml(yaml).is_err());
    }
}
