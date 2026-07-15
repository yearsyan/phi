use std::{fmt, sync::Arc};

use async_trait::async_trait;

use crate::{
    error::HookError,
    types::{ContextUsage, Message},
};

/// Default percentage of the configured context window that activates context
/// managers after a successful provider response.
pub const DEFAULT_CONTEXT_MANAGEMENT_THRESHOLD_PERCENT: u8 = 80;

/// Why the context-manager chain was activated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextManagementTrigger {
    /// A successful provider response reported usage at or above the configured
    /// percentage of the model's context window.
    UsageThreshold {
        usage: ContextUsage,
        threshold_percent: u8,
    },
    /// The provider rejected a request because its context window was exceeded
    /// before emitting any assistant output.
    ContextLengthExceeded { error: String },
}

/// Mutable transcript view passed to one context manager.
///
/// The primary system prompt is intentionally read-only and separate from the
/// transcript. Managers may rewrite retained conversation messages. The Agent
/// validates tool-call/result pairing after the complete manager chain and
/// persists a changed transcript before continuing or returning an error.
pub struct ContextManagementContext<'a> {
    trigger: ContextManagementTrigger,
    system_prompt: &'a str,
    max_context_tokens: Option<u64>,
    messages: &'a mut Vec<Message>,
    touched: bool,
}

impl<'a> ContextManagementContext<'a> {
    pub(crate) fn new(
        trigger: ContextManagementTrigger,
        system_prompt: &'a str,
        max_context_tokens: Option<u64>,
        messages: &'a mut Vec<Message>,
    ) -> Self {
        Self {
            trigger,
            system_prompt,
            max_context_tokens,
            messages,
            touched: false,
        }
    }

    pub fn trigger(&self) -> &ContextManagementTrigger {
        &self.trigger
    }

    pub fn system_prompt(&self) -> &str {
        self.system_prompt
    }

    pub fn max_context_tokens(&self) -> Option<u64> {
        self.max_context_tokens
    }

    pub fn messages(&self) -> &[Message] {
        self.messages
    }

    /// Returns unrestricted mutable transcript access.
    ///
    /// Prefer the higher-level operations when possible. Regardless of which
    /// operation is used, the complete transcript is protocol-validated after
    /// all registered managers finish.
    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
        self.touched = true;
        self.messages
    }

    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.touched = true;
        *self.messages = messages;
    }

    pub fn clear_messages(&mut self) {
        self.touched = true;
        self.messages.clear();
    }

    /// Replaces the first `end` retained messages with `replacement`.
    ///
    /// This is the common operation for replacing old turns with a summary.
    /// The selected boundary must not split an assistant tool-call batch from
    /// its tool results; final protocol validation rejects such a rewrite.
    pub fn replace_prefix(
        &mut self,
        end: usize,
        replacement: impl IntoIterator<Item = Message>,
    ) -> Result<(), HookError> {
        if end > self.messages.len() {
            return Err(HookError::new(format!(
                "context prefix ends at message {end}, but the transcript contains only {} messages",
                self.messages.len()
            )));
        }
        self.touched = true;
        self.messages.splice(..end, replacement);
        Ok(())
    }

    /// Truncates the retained transcript to `len` messages.
    pub fn truncate(&mut self, len: usize) -> Result<(), HookError> {
        if len > self.messages.len() {
            return Err(HookError::new(format!(
                "cannot truncate context to {len} messages because it contains only {} messages",
                self.messages.len()
            )));
        }
        self.touched = true;
        self.messages.truncate(len);
        Ok(())
    }

    pub fn was_touched(&self) -> bool {
        self.touched
    }
}

/// One asynchronous context-management hook.
///
/// Managers run sequentially in registration order. Return `Ok(true)` when the
/// next registered manager should also run, or `Ok(false)` when this manager
/// has fully handled the trigger and the chain should stop. Returning an error
/// aborts the run and rolls back unpersisted transcript changes from the chain.
#[async_trait]
pub trait ContextManager: Send + Sync {
    async fn manage_context(
        &self,
        context: &mut ContextManagementContext<'_>,
    ) -> Result<bool, HookError>;
}

/// Ordered, cloneable collection of context managers.
#[derive(Clone, Default)]
pub struct ContextManagerRegistry {
    managers: Vec<Arc<dyn ContextManager>>,
}

impl ContextManagerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_manager(mut self, manager: impl ContextManager + 'static) -> Self {
        self.register(manager);
        self
    }

    pub fn register(&mut self, manager: impl ContextManager + 'static) {
        self.managers.push(Arc::new(manager));
    }

    pub fn extend(&mut self, other: Self) {
        self.managers.extend(other.managers);
    }

    pub fn len(&self) -> usize {
        self.managers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.managers.is_empty()
    }

    pub(crate) async fn run(
        &self,
        context: &mut ContextManagementContext<'_>,
    ) -> Result<(), HookError> {
        for manager in &self.managers {
            if !manager.manage_context(context).await? {
                break;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for ContextManagerRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextManagerRegistry")
            .field("manager_count", &self.managers.len())
            .finish()
    }
}

pub(crate) fn threshold_reached(usage: ContextUsage, threshold_percent: u8) -> bool {
    usage.max_tokens > 0
        && u128::from(usage.used_tokens) * 100
            >= u128::from(usage.max_tokens) * u128::from(threshold_percent)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct RecordingManager {
        name: &'static str,
        continue_chain: bool,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl ContextManager for RecordingManager {
        async fn manage_context(
            &self,
            context: &mut ContextManagementContext<'_>,
        ) -> Result<bool, HookError> {
            tokio::task::yield_now().await;
            self.calls.lock().unwrap().push(self.name);
            context
                .messages_mut()
                .push(Message::user(format!("managed by {}", self.name)));
            Ok(self.continue_chain)
        }
    }

    #[tokio::test]
    async fn managers_run_in_order_and_false_stops_the_chain() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let registry = ContextManagerRegistry::new()
            .with_manager(RecordingManager {
                name: "first",
                continue_chain: true,
                calls: Arc::clone(&calls),
            })
            .with_manager(RecordingManager {
                name: "second",
                continue_chain: false,
                calls: Arc::clone(&calls),
            })
            .with_manager(RecordingManager {
                name: "skipped",
                continue_chain: true,
                calls: Arc::clone(&calls),
            });
        let mut messages = vec![Message::user("original")];
        let mut context = ContextManagementContext::new(
            ContextManagementTrigger::ContextLengthExceeded {
                error: "too long".to_owned(),
            },
            "system",
            Some(100),
            &mut messages,
        );

        registry.run(&mut context).await.unwrap();

        assert_eq!(calls.lock().unwrap().as_slice(), ["first", "second"]);
        assert!(context.was_touched());
        assert_eq!(context.messages().len(), 3);
    }

    #[test]
    fn threshold_comparison_uses_wide_integer_arithmetic() {
        assert!(threshold_reached(
            ContextUsage {
                max_tokens: u64::MAX,
                used_tokens: u64::MAX,
                remaining_tokens: 0,
            },
            80,
        ));
        assert!(threshold_reached(
            ContextUsage {
                max_tokens: 100,
                used_tokens: 80,
                remaining_tokens: 20,
            },
            80,
        ));
        assert!(!threshold_reached(
            ContextUsage {
                max_tokens: 100,
                used_tokens: 79,
                remaining_tokens: 21,
            },
            80,
        ));
    }
}
