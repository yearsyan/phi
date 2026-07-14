use std::{fmt, sync::Arc};

use async_trait::async_trait;
use reqwest::header::HeaderMap;
use serde_json::Value;

use crate::{
    error::HookError,
    types::{Message, ProviderRequest, ProviderResponse},
};

/// Wire protocol selected by a built-in HTTP provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderApi {
    AnthropicMessages,
    OpenAiChatCompletions,
    OpenAiResponses,
}

/// Mutable wire request exposed after adapter serialization and immediately
/// before the first HTTP attempt.
///
/// The default authentication headers are already present. Hooks run once per
/// logical provider request, and their result is reused by every HTTP retry.
/// This type intentionally does not implement `Debug` because headers may
/// contain credentials.
pub struct BeforeRequestContext {
    pub api: ProviderApi,
    pub endpoint: String,
    pub headers: HeaderMap,
    pub body: Value,
}

/// Mutable normalized request exposed at the beginning of an agent turn.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnStartContext {
    pub turn: usize,
    pub request: ProviderRequest,
}

/// Mutable complete provider response exposed before it enters agent state.
#[derive(Clone, Debug, PartialEq)]
pub struct LlmResponseContext {
    pub turn: usize,
    pub response: ProviderResponse,
}

/// Mutable data produced by a complete turn, after tool execution when tools
/// were requested and before the next turn starts.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnEndContext {
    pub turn: usize,
    pub message: Message,
    pub tool_results: Vec<Message>,
}

/// Asynchronous lifecycle hook. Implement only the stages that are needed;
/// every method is a no-op by default.
///
/// Multiple hooks run sequentially in registration order, so each hook sees
/// mutations made by earlier hooks.
#[async_trait]
pub trait Hook: Send + Sync {
    async fn before_request(&self, _context: &mut BeforeRequestContext) -> Result<(), HookError> {
        Ok(())
    }

    async fn on_turn_start(&self, _context: &mut TurnStartContext) -> Result<(), HookError> {
        Ok(())
    }

    async fn on_llm_response(&self, _context: &mut LlmResponseContext) -> Result<(), HookError> {
        Ok(())
    }

    async fn on_turn_end(&self, _context: &mut TurnEndContext) -> Result<(), HookError> {
        Ok(())
    }
}

/// Ordered, cloneable collection of lifecycle hooks.
#[derive(Clone, Default)]
pub struct HookRegistry {
    hooks: Vec<Arc<dyn Hook>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_hook(mut self, hook: impl Hook + 'static) -> Self {
        self.register(hook);
        self
    }

    pub fn register(&mut self, hook: impl Hook + 'static) {
        self.hooks.push(Arc::new(hook));
    }

    pub fn extend(&mut self, other: Self) {
        self.hooks.extend(other.hooks);
    }

    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Runs all registered `before_request` hooks in order.
    pub async fn run_before_request(
        &self,
        context: &mut BeforeRequestContext,
    ) -> Result<(), HookError> {
        for hook in &self.hooks {
            hook.before_request(context).await?;
        }
        Ok(())
    }

    /// Runs all registered `on_turn_start` hooks in order.
    pub async fn run_turn_start(&self, context: &mut TurnStartContext) -> Result<(), HookError> {
        for hook in &self.hooks {
            hook.on_turn_start(context).await?;
        }
        Ok(())
    }

    /// Runs all registered `on_llm_response` hooks in order.
    pub async fn run_llm_response(
        &self,
        context: &mut LlmResponseContext,
    ) -> Result<(), HookError> {
        for hook in &self.hooks {
            hook.on_llm_response(context).await?;
        }
        Ok(())
    }

    /// Runs all registered `on_turn_end` hooks in order.
    pub async fn run_turn_end(&self, context: &mut TurnEndContext) -> Result<(), HookError> {
        for hook in &self.hooks {
            hook.on_turn_end(context).await?;
        }
        Ok(())
    }
}

impl fmt::Debug for HookRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HookRegistry")
            .field("hook_count", &self.hooks.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use reqwest::header::{HeaderMap, HeaderValue};
    use serde_json::json;

    use super::*;

    struct MutatingHook {
        value: &'static str,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl Hook for MutatingHook {
        async fn before_request(
            &self,
            context: &mut BeforeRequestContext,
        ) -> Result<(), HookError> {
            self.calls.lock().unwrap().push(self.value);
            context.body["order"]
                .as_array_mut()
                .unwrap()
                .push(json!(self.value));
            context
                .headers
                .insert("x-hook", HeaderValue::from_static(self.value));
            Ok(())
        }
    }

    #[tokio::test]
    async fn runs_async_request_hooks_in_registration_order() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let registry = HookRegistry::new()
            .with_hook(MutatingHook {
                value: "first",
                calls: Arc::clone(&calls),
            })
            .with_hook(MutatingHook {
                value: "second",
                calls: Arc::clone(&calls),
            });
        let mut context = BeforeRequestContext {
            api: ProviderApi::AnthropicMessages,
            endpoint: "https://example.com/v1/messages".to_owned(),
            headers: HeaderMap::new(),
            body: json!({ "order": [] }),
        };

        registry.run_before_request(&mut context).await.unwrap();

        assert_eq!(calls.lock().unwrap().as_slice(), ["first", "second"]);
        assert_eq!(context.body["order"], json!(["first", "second"]));
        assert_eq!(context.headers["x-hook"], "second");
    }
}
