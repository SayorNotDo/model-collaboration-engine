//! Host tool batches and their accounting.
use super::{Engine, RunContext};
use crate::{
    adapter::{Message, ModelOutput},
    contracts::{
        digest, id, now_ms, EngineError, Result, TaskSpec, ToolCall, ToolResult, ToolSpec,
    },
    tools::ToolRequest,
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};
impl Engine {
    #[allow(clippy::too_many_arguments)]
    async fn execute_tool(
        &self,
        task: &TaskSpec,
        context: &RunContext,
        node: &str,
        model_attempt: &str,
        call: ToolCall,
        spec: &ToolSpec,
    ) -> Result<ToolResult> {
        let executor = context
            .tools
            .as_ref()
            .ok_or_else(|| EngineError::new("configuration", "host tool executor missing"))?;
        let request = self
            .reserve_tool(task, context, node, model_attempt, call, spec)
            .await?;
        let execution_id = request.execution_id.clone();
        let remaining = request.deadline_ms.saturating_sub(now_ms());
        if remaining == 0 || context.cancel.is_cancelled() {
            self.store
                .settle(
                    &task.task_id,
                    &execution_id,
                    Some(0),
                    json!({
                        "not_dispatched": true,
                        "call_metrics": {"status": "not_dispatched"},
                    }),
                )
                .await?;
            return Err(if context.cancel.is_cancelled() {
                EngineError::new("cancelled", "cancelled before tool dispatch")
            } else {
                EngineError::new("deadline", "deadline before tool dispatch")
            });
        }
        let started = Instant::now();
        let result = tokio::select! {
            biased;
            _ = context.cancel.cancelled() => Err(EngineError::new("cancelled", "cancelled during host tool execution")),
            result = tokio::time::timeout(Duration::from_millis(remaining), executor.execute(request)) => result.unwrap_or_else(|_| Err(EngineError::new("deadline", "host tool timed out"))),
        };
        let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        match result {
            Ok(result) => {
                self.store
                    .settle(
                        &task.task_id,
                        &execution_id,
                        result.actual_cost,
                        json!({
                            "kind": "tool",
                            "output_checksum": digest(&result.output),
                            "call_metrics": {"status": "succeeded", "latency_ms": latency_ms},
                        }),
                    )
                    .await?;
                if result.output.len() > self.config.context_max_bytes {
                    return Err(EngineError::new(
                        "context",
                        "tool result exceeds context byte limit",
                    ));
                }
                self.emit(
                    task,
                    context,
                    Some(node),
                    Some(&execution_id),
                    "tool_completed",
                    json!({"name":spec.name,"actual_cost":result.actual_cost}),
                )
                .await?;
                Ok(result)
            }
            Err(error) => {
                self.store
                    .settle(
                        &task.task_id,
                        &execution_id,
                        None,
                        json!({
                            "kind": "tool", "error": error,
                            "call_metrics": {
                                "status": match error.kind.as_str() {
                                    "cancelled" => "cancelled",
                                    "deadline" => "timed_out",
                                    _ => "failed",
                                },
                                "latency_ms": latency_ms,
                            },
                        }),
                    )
                    .await?;
                Err(error)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn continue_tools(
        &self,
        task: &TaskSpec,
        context: &RunContext,
        node: &str,
        model_attempt: &str,
        output: ModelOutput,
        tools: &[ToolSpec],
        messages: &mut Vec<Message>,
        seen_calls: &mut BTreeSet<String>,
    ) -> Result<()> {
        // Validate the whole batch before dispatching its first side effect.
        if output.tool_calls.len() > 128 {
            return Err(EngineError::new("protocol", "too many tool calls"));
        }
        for call in &output.tool_calls {
            if call.id.is_empty()
                || !seen_calls.insert(call.id.clone())
                || !call.arguments.is_object()
                || !tools.iter().any(|t| t.name == call.name)
            {
                return Err(EngineError::new(
                    "protocol",
                    "undeclared, duplicate or malformed tool call",
                ));
            }
        }
        messages.push(Message {
            role: "assistant".into(),
            content: output.text,
            tool_calls: output.tool_calls.clone(),
            tool_call_id: None,
        });
        for call in output.tool_calls {
            let spec = tools.iter().find(|t| t.name == call.name).unwrap();
            let result = self
                .execute_tool(task, context, node, model_attempt, call.clone(), spec)
                .await?;
            messages.push(Message {
                role: "tool".into(),
                content: result.output,
                tool_calls: vec![],
                tool_call_id: Some(call.id),
            });
            if serde_json::to_vec(&json!({"messages":messages,"tools":tools}))
                .unwrap()
                .len()
                > self.config.context_max_bytes
            {
                return Err(EngineError::new(
                    "context",
                    "tool conversation exceeds context byte limit",
                ));
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    async fn reserve_tool(
        &self,
        task: &TaskSpec,
        context: &RunContext,
        node: &str,
        model_attempt: &str,
        call: ToolCall,
        spec: &ToolSpec,
    ) -> Result<ToolRequest> {
        let execution_id = id();
        let request = ToolRequest {
            task_id: task.task_id.clone(),
            execution_id: execution_id.clone(),
            model_attempt_id: model_attempt.into(),
            call,
            max_cost: spec.max_cost,
            deadline_ms: task.deadline_ms.saturating_sub(task.finalization_ms),
        };
        self.emit(
            task,
            context,
            Some(node),
            Some(&execution_id),
            "tool_requested",
            json!({
                "name": spec.name,
                "call_id": request.call.id,
                "model_attempt_id": model_attempt,
                "max_cost": spec.max_cost,
            }),
        )
        .await?;
        self.store
            .reserve(
                &task.task_id,
                &execution_id,
                spec.max_cost,
                task.max_calls,
                json!({"kind":"tool","request":request}),
            )
            .await?;
        Ok(request)
    }
}
