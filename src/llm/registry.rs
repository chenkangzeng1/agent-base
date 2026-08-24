use std::sync::Arc;

use super::{AnthropicAdapter, OpenAiAdapter, StreamClient};

#[derive(Clone, Debug)]
pub enum LlmProvider {
    OpenAi,
    OpenAiResponses,
    Anthropic,
    Custom(String),
}

impl LlmProvider {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "openai" => Self::OpenAi,
            "openai-responses" | "responses" => Self::OpenAiResponses,
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

    /// Build an `Arc<dyn StreamClient>`.
    ///
    /// This is the primary build method. All providers implement `StreamClient`.
    pub fn build(self) -> Arc<dyn StreamClient> {
        let base_url = self.base_url;
        match self.provider {
            LlmProvider::OpenAi => {
                let url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
                Arc::new(OpenAiAdapter::chat_client(
                    self.api_key,
                    self.model,
                    Some(url),
                ))
            }
            LlmProvider::OpenAiResponses => {
                let url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
                Arc::new(OpenAiAdapter::responses_client(
                    self.api_key,
                    self.model,
                    Some(url),
                ))
            }
            LlmProvider::Anthropic => {
                let url = base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string());
                Arc::new(AnthropicAdapter::new(self.api_key, self.model, Some(url)))
            }
            LlmProvider::Custom(_) => {
                let url = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
                Arc::new(OpenAiAdapter::chat_client(
                    self.api_key,
                    self.model,
                    Some(url),
                ))
            }
        }
    }

    /// Build an `Arc<dyn StreamClient>` (alias for [`build`](Self::build)).
    #[deprecated(
        since = "0.4.0",
        note = "use `build()` instead — it now returns `Arc<dyn StreamClient>` directly"
    )]
    pub fn build_stream_client(self) -> Arc<dyn StreamClient> {
        self.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_from_str_is_case_insensitive() {
        assert!(matches!(
            LlmProvider::from_str("openai"),
            LlmProvider::OpenAi
        ));
        assert!(matches!(
            LlmProvider::from_str("OpenAI"),
            LlmProvider::OpenAi
        ));
        assert!(matches!(
            LlmProvider::from_str("OPENAI"),
            LlmProvider::OpenAi
        ));
        assert!(matches!(
            LlmProvider::from_str("anthropic"),
            LlmProvider::Anthropic
        ));
        assert!(matches!(
            LlmProvider::from_str("Anthropic"),
            LlmProvider::Anthropic
        ));
    }

    #[test]
    fn provider_from_str_openai_responses() {
        assert!(matches!(
            LlmProvider::from_str("openai-responses"),
            LlmProvider::OpenAiResponses
        ));
        assert!(matches!(
            LlmProvider::from_str("OpenAI-Responses"),
            LlmProvider::OpenAiResponses
        ));
        assert!(matches!(
            LlmProvider::from_str("responses"),
            LlmProvider::OpenAiResponses
        ));
    }

    #[test]
    fn provider_from_str_unknown_becomes_custom() {
        assert!(
            matches!(LlmProvider::from_str("ollama"), LlmProvider::Custom(ref s) if s == "ollama")
        );
        assert!(matches!(LlmProvider::from_str(""), LlmProvider::Custom(ref s) if s.is_empty()));
    }

    #[test]
    fn build_routes_openai() {
        let client = LlmClientBuilder::new(LlmProvider::OpenAi, "sk-test", "gpt-4o").build();
        assert_eq!(client.model_name(), "gpt-4o");
        assert_eq!(client.capabilities().max_context_tokens, Some(128_000));
    }

    #[test]
    fn build_routes_anthropic() {
        let client = LlmClientBuilder::new(LlmProvider::Anthropic, "sk-ant", "claude").build();
        assert_eq!(client.model_name(), "claude");
        assert_eq!(client.capabilities().max_context_tokens, Some(200_000));
    }

    #[test]
    fn build_custom_defaults_to_openai() {
        let client =
            LlmClientBuilder::new(LlmProvider::Custom("ollama".into()), "sk", "llama").build();
        assert_eq!(client.model_name(), "llama");
        assert_eq!(client.capabilities().max_context_tokens, Some(128_000));
    }

    #[test]
    fn build_stream_client_responses() {
        let client = LlmClientBuilder::new(LlmProvider::OpenAiResponses, "sk", "gpt-4o").build();
        assert_eq!(client.model_name(), "gpt-4o");
        assert_eq!(client.capabilities().max_context_tokens, Some(128_000));
    }

    #[test]
    fn base_url_is_chainable() {
        let client = LlmClientBuilder::new(LlmProvider::OpenAi, "sk", "gpt-4o")
            .base_url("http://localhost:9999/v1")
            .build();
        assert_eq!(client.model_name(), "gpt-4o");
    }

    #[test]
    fn builder_from_env_reads_vars_and_requires_key() {
        // Single test (no intra-module parallelism) so env mutations can't race.
        unsafe {
            std::env::remove_var("LLM_API_KEY");
            std::env::remove_var("LLM_MODEL");
            std::env::remove_var("LLM_BASE_URL");
            std::env::remove_var("LLM_PROVIDER");
        }
        assert!(LlmClientBuilder::from_env().is_none());

        unsafe {
            std::env::set_var("LLM_API_KEY", "env-key");
            std::env::set_var("LLM_MODEL", "env-model");
            std::env::set_var("LLM_BASE_URL", "http://env.test/v1");
            std::env::set_var("LLM_PROVIDER", "anthropic");
        }
        let client = LlmClientBuilder::from_env().expect("all vars set").build();
        assert_eq!(client.model_name(), "env-model");
        assert_eq!(client.capabilities().max_context_tokens, Some(200_000));

        unsafe {
            std::env::remove_var("LLM_API_KEY");
            std::env::remove_var("LLM_MODEL");
            std::env::remove_var("LLM_BASE_URL");
            std::env::remove_var("LLM_PROVIDER");
        }
    }
}
