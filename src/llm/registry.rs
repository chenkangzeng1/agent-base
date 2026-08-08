use std::sync::Arc;

use super::{AnthropicClient, LlmClient, OpenAiClient};

#[derive(Clone, Debug)]
pub enum LlmProvider {
    OpenAi,
    Anthropic,
    Custom(String),
}

impl LlmProvider {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "openai" => Self::OpenAi,
            "anthropic" => Self::Anthropic,
            other => Self::Custom(other.to_string()),
        }
    }
}

pub struct LlmClientBuilder {
    provider: LlmProvider,
    api_key: String,
    model: String,
    base_url: Option<String>,
}

impl LlmClientBuilder {
    pub fn new(
        provider: LlmProvider,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            api_key: api_key.into(),
            model: model.into(),
            base_url: None,
        }
    }

    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("LLM_API_KEY").ok()?;
        let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
        let base_url = std::env::var("LLM_BASE_URL").ok();
        let provider_str = std::env::var("LLM_PROVIDER").unwrap_or_else(|_| "openai".to_string());

        Some(Self {
            provider: LlmProvider::from_str(&provider_str),
            api_key,
            model,
            base_url,
        })
    }

    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    pub fn build(self) -> Arc<dyn LlmClient> {
        let base_url = self.base_url;
        match self.provider {
            LlmProvider::OpenAi => {
                let url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
                Arc::new(OpenAiClient::new(self.api_key, self.model, Some(url)))
            }
            LlmProvider::Anthropic => {
                let url = base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string());
                Arc::new(AnthropicClient::new(self.api_key, self.model, Some(url)))
            }
            LlmProvider::Custom(_) => {
                let url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
                Arc::new(OpenAiClient::new(self.api_key, self.model, Some(url)))
            }
        }
    }
}
