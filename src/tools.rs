//! Explicit host boundary. The host owns authorization, argument validation and side effects.
use crate::contracts::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequest {
    pub task_id: String,
    pub execution_id: String,
    pub model_attempt_id: String,
    pub call: ToolCall,
    pub max_cost: u64,
    pub deadline_ms: u64,
}

/// Registration grants no blanket permission: implementations must authorize each request.
/// Futures may be dropped on cancellation; remote side effects may still finish.
/// An error has unknown cost. Return a ToolResult with a known cost when available.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, request: ToolRequest) -> Result<ToolResult>;
}
