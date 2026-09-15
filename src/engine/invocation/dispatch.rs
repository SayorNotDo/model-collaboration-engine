//! Model dispatch and settlement share one cancellation boundary.
use super::super::{Engine, RunContext};
use crate::{
    adapter::{InvokeRequest, Message, ModelOutput},
    contracts::{now_ms, EngineError, Model, Result, Snapshot, TaskSpec, ToolSpec},
};
use serde_json::json;
use std::time::{Duration, Instant};
impl Engine {
    // The inner result is an adapter outcome; outer errors stop fallback (e.g. settlement failure).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::engine) async fn dispatch_model(
        &self,
        task: &TaskSpec,
        context: &RunContext,
        snapshot: &Snapshot,
        model: &Model,
        attempt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<Result<ModelOutput>> {
        let cancel = &context.cancel;
        let remaining = task
            .deadline_ms
            .saturating_sub(task.finalization_ms)
            .saturating_sub(now_ms());
        if remaining == 0 || cancel.is_cancelled() {
            self.store
                .settle(
                    &task.task_id,
                    attempt,
                    Some(0),
                    json!({
                        "not_dispatched": true,
                        "call_metrics": {"status": "not_dispatched"},
                    }),
                )
                .await?;
            return Ok(Err(if cancel.is_cancelled() {
                EngineError::new("cancelled", "task cancelled before dispatch")
            } else {
                EngineError::new("deadline", "execution deadline reached before dispatch")
            }));
        }
        let request = InvokeRequest {
            model: model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            json_object: task.acceptance.json_object || snapshot.role == "critic",
            output_tokens: task.output_tokens,
            task_id: task.task_id.clone(),
            node: snapshot.role.clone(),
            attempt_id: attempt.to_owned(),
            event_sink: if snapshot.role == "planner" {
                let mut sink = context.sink.clone();
                sink.sender = None;
                sink
            } else {
                context.sink.clone()
            },
            sequence: context.sequence.clone(),
        };
        let started = Instant::now();
        let output = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(EngineError::new("cancelled", "task cancelled during model invocation")),
            result = tokio::time::timeout(Duration::from_millis(remaining), self.adapter.invoke(request)) => result.unwrap_or_else(|_| Err(EngineError::new("deadline", "model invocation timed out"))),
        };
        let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        match &output {
            Ok(output) => {
                let cost = output.usage.as_ref().map(|u| {
                    u.input_tokens
                        .saturating_mul(model.input_price)
                        .saturating_add(u.output_tokens.saturating_mul(model.output_price))
                });
                let mut outcome = json!({
                    "request_id": output.request_id,
                    "complete": output.complete,
                    "call_metrics": {
                        "status": if output.complete { "succeeded" } else { "failed" },
                        "latency_ms": latency_ms,
                    },
                });
                if snapshot.role == "planner" {
                    let mut end = output.text.len().min(self.config.context_max_bytes);
                    while !output.text.is_char_boundary(end) {
                        end -= 1;
                    }
                    outcome["planning_output"] = json!({
                        "text": &output.text[..end], "truncated": end < output.text.len(),
                        "tool_calls_present": !output.tool_calls.is_empty(),
                    });
                }
                self.store
                    .settle(&task.task_id, attempt, cost, outcome)
                    .await?;
            }
            Err(error) => {
                self.store
                    .settle(
                        &task.task_id,
                        attempt,
                        None,
                        json!({
                            "error": error,
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
            }
        }
        Ok(output)
    }
}
