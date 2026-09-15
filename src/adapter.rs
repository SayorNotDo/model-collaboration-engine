//! SDK wire contracts with a pull-based, bounded SSE transport. No hidden retries.
use crate::{
    contracts::*,
    events::{Event, EventSink},
};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
}
impl Message {
    pub fn text(role: &str, text: String) -> Self {
        Self {
            role: role.into(),
            content: text,
            tool_calls: vec![],
            tool_call_id: None,
        }
    }
}
pub struct InvokeRequest {
    pub model: Model,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub json_object: bool,
    pub output_tokens: u64,
    pub task_id: String,
    pub node: String,
    pub attempt_id: String,
    pub event_sink: EventSink,
    pub sequence: Arc<AtomicU64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelOutput {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
    pub request_id: Option<String>,
    pub complete: bool,
}
#[async_trait]
pub trait ModelAdapter: Send + Sync {
    async fn invoke(&self, request: InvokeRequest) -> Result<ModelOutput>;
}
pub struct OpenAIAdapter {
    client: reqwest::Client,
    frame_limit: usize,
    output_limit: usize,
}
impl OpenAIAdapter {
    pub fn new(config: &Config) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| EngineError::new("configuration", "cannot initialize HTTP client"))?;
        Ok(Self {
            client,
            frame_limit: config.stream_frame_max_bytes,
            output_limit: config.context_max_bytes,
        })
    }
}
pub fn wire_request(r: &InvokeRequest) -> Result<Value> {
    let mut body = match r.model.endpoint {
        Endpoint::ChatCompletions => {
            let messages:Vec<Value>=r.messages.iter().map(|m| {
                let mut v=json!({"role":m.role,"content":m.content});
                if let Some(id)=&m.tool_call_id {v["tool_call_id"]=json!(id);}
                if !m.tool_calls.is_empty() {v["tool_calls"]=json!(m.tool_calls.iter().map(|t|json!({"id":t.id,"type":"function","function":{"name":t.name,"arguments":t.arguments.to_string()}})).collect::<Vec<_>>());}
                v
            }).collect();
            let mut body = json!({"model":r.model.model,"messages":messages,"max_completion_tokens":r.output_tokens,"stream":true,"stream_options":{"include_usage":true}});
            if !r.tools.is_empty() {
                body["tools"]=json!(r.tools.iter().map(|t|json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}})).collect::<Vec<_>>());
            }
            if r.json_object {
                body["response_format"] = json!({"type":"json_object"});
            }
            body
        }
        Endpoint::Responses => {
            let mut input = vec![];
            for m in &r.messages {
                if let Some(call_id) = &m.tool_call_id {
                    input.push(
                        json!({"type":"function_call_output","call_id":call_id,"output":m.content}),
                    );
                } else {
                    if !m.content.is_empty() {
                        input.push(json!({"role":m.role,"content":m.content}));
                    }
                    for t in &m.tool_calls {
                        input.push(json!({"type":"function_call","call_id":t.id,"name":t.name,"arguments":t.arguments.to_string()}));
                    }
                }
            }
            let mut body = json!({"model":r.model.model,"input":input,"max_output_tokens":r.output_tokens,"stream":true,"store":false});
            if !r.tools.is_empty() {
                body["tools"]=json!(r.tools.iter().map(|t|json!({"type":"function","name":t.name,"description":t.description,"parameters":t.parameters,"strict":false})).collect::<Vec<_>>());
            }
            if r.json_object {
                body["text"] = json!({"format":{"type":"json_object"}});
            }
            body
        }
    };
    // Keep SDK types private. Deserializing validates each endpoint independently.
    match r.model.endpoint {
        Endpoint::ChatCompletions => {
            let _: async_openai::types::chat::CreateChatCompletionRequest =
                serde_json::from_value(body.clone()).map_err(|_| {
                    EngineError::new("protocol", "invalid Chat Completions request")
                })?;
        }
        Endpoint::Responses => {
            let _: async_openai::types::responses::CreateResponse =
                serde_json::from_value(body.clone())
                    .map_err(|_| EngineError::new("protocol", "invalid Responses request"))?;
        }
    }
    // Idempotency remains Runtime-owned for side effects; don't imply that model
    // endpoints implement exactly-once execution by inventing an idempotency key.
    body.as_object_mut().unwrap().retain(|_, v| !v.is_null());
    Ok(body)
}

#[derive(Default)]
struct Accumulator {
    text: String,
    tools: BTreeMap<u64, (String, String, String)>,
    usage: Option<Usage>,
    request_id: Option<String>,
    complete: bool,
}
impl Accumulator {
    fn frame(&mut self, v: Value, endpoint: &Endpoint) -> Result<Option<String>> {
        let mut delta = None;
        if v.get("error").is_some() {
            return Err(EngineError::new(
                "model",
                "provider returned a stream error",
            ));
        }
        match endpoint {
            Endpoint::ChatCompletions => {
                if let Some(s) = v["id"].as_str() {
                    self.request_id = Some(s.into());
                }
                if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                    self.usage = parse_usage(u, "prompt_tokens", "completion_tokens");
                }
                if let Some(choices) = v["choices"].as_array() {
                    for choice in choices {
                        if choice["index"].as_u64().unwrap_or(0) != 0 {
                            continue;
                        }
                        let d = &choice["delta"];
                        if let Some(s) = d["content"].as_str() {
                            self.text.push_str(s);
                            delta = Some(s.into());
                        }
                        if let Some(tools) = d["tool_calls"].as_array() {
                            for t in tools {
                                let index = t["index"].as_u64().ok_or_else(|| {
                                    EngineError::new("protocol", "tool index missing")
                                })?;
                                let a = self.tools.entry(index).or_default();
                                if let Some(s) = t["id"].as_str() {
                                    a.0.push_str(s);
                                }
                                if let Some(s) = t["function"]["name"].as_str() {
                                    a.1.push_str(s);
                                }
                                if let Some(s) = t["function"]["arguments"].as_str() {
                                    a.2.push_str(s);
                                }
                            }
                        }
                        if let Some(f) = choice["finish_reason"].as_str() {
                            self.complete = matches!(f, "stop" | "tool_calls");
                        }
                    }
                }
            }
            Endpoint::Responses => {
                match v["type"].as_str().unwrap_or("") {
                    "response.output_text.delta" => {
                        if let Some(s) = v["delta"].as_str() {
                            self.text.push_str(s);
                            delta = Some(s.into());
                        }
                    }
                    "response.completed" | "response.incomplete" | "response.failed" => {
                        let response = &v["response"];
                        self.complete =
                            v["type"] == "response.completed" && response["status"] == "completed";
                        self.request_id = response["id"].as_str().map(String::from);
                        self.usage =
                            parse_usage(&response["usage"], "input_tokens", "output_tokens");
                        if let Some(items) = response["output"].as_array() {
                            for (i, item) in items.iter().enumerate() {
                                if item["type"] == "function_call" {
                                    self.tools.insert(
                                        i as u64,
                                        (
                                            item["call_id"].as_str().unwrap_or("").into(),
                                            item["name"].as_str().unwrap_or("").into(),
                                            item["arguments"].as_str().unwrap_or("").into(),
                                        ),
                                    );
                                }
                            }
                        }
                    }
                    "error" => return Err(EngineError::new("model", "Responses stream error")),
                    // Reasoning and other non-output events are not exposed.
                    _ => {}
                }
            }
        }
        Ok(delta)
    }
    fn bytes(&self) -> usize {
        self.text.len()
            + self
                .tools
                .values()
                .map(|(a, b, c)| a.len() + b.len() + c.len())
                .sum::<usize>()
    }
    fn finish(self) -> Result<ModelOutput> {
        let mut tool_calls = vec![];
        for (_, (id, name, args)) in self.tools {
            if id.is_empty() || name.is_empty() {
                return Err(EngineError::new("protocol", "incomplete tool call"));
            }
            let arguments = serde_json::from_str(&args)
                .map_err(|_| EngineError::new("protocol", "invalid tool argument JSON"))?;
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
        Ok(ModelOutput {
            text: self.text,
            tool_calls,
            usage: self.usage,
            request_id: self.request_id,
            complete: self.complete,
        })
    }
}
fn parse_usage(v: &Value, input: &str, output: &str) -> Option<Usage> {
    Some(Usage {
        input_tokens: v[input].as_u64()?,
        output_tokens: v[output].as_u64()?,
    })
}
#[async_trait]
impl ModelAdapter for OpenAIAdapter {
    async fn invoke(&self, r: InvokeRequest) -> Result<ModelOutput> {
        let body = wire_request(&r)?;
        let key = std::env::var(&r.model.api_key_env).map_err(|_| {
            EngineError::new(
                "credentials",
                "credential environment variable is unavailable",
            )
        })?;
        let endpoint = match r.model.endpoint {
            Endpoint::ChatCompletions => "chat/completions",
            Endpoint::Responses => "responses",
        };
        let response = self
            .client
            .post(format!(
                "{}/{endpoint}",
                r.model.base_url.trim_end_matches('/')
            ))
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .map_err(|_| {
                EngineError::new("model", "transport failed; request execution is uncertain")
            })?;
        if !response.status().is_success() {
            return Err(EngineError::new("model", "provider rejected request")
                .details(json!({"http_status":response.status().as_u16()})));
        }
        if !response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.starts_with("text/event-stream"))
        {
            return Err(EngineError::new("protocol", "expected an SSE response"));
        }
        let mut stream = response.bytes_stream();
        let mut line = Vec::new();
        let mut data = String::new();
        let mut acc = Accumulator::default();
        let mut ended = false;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|_| EngineError::new("model", "stream interrupted; usage unresolved"))?;
            for byte in chunk {
                if byte != b'\n' {
                    line.push(byte);
                    if line.len() + data.len() > self.frame_limit {
                        return Err(EngineError::new("protocol", "SSE frame exceeds byte limit"));
                    }
                    continue;
                }
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                let current = std::str::from_utf8(&line)
                    .map_err(|_| EngineError::new("protocol", "invalid SSE UTF-8"))?;
                if current.is_empty() && !data.is_empty() {
                    if data.trim() == "[DONE]" {
                        ended = true;
                        break;
                    }
                    let value: Value = serde_json::from_str(data.trim_end())
                        .map_err(|_| EngineError::new("protocol", "invalid SSE JSON"))?;
                    let delta = acc.frame(value, &r.model.endpoint)?;
                    if acc.bytes() > self.output_limit || acc.tools.len() > 128 {
                        return Err(EngineError::new(
                            "protocol",
                            "model output exceeds byte or tool count limit",
                        ));
                    }
                    if let Some(delta) = delta {
                        // Split on character boundaries so every event has a firm size bound.
                        let limit = (r.event_sink.max_bytes / 8).max(1);
                        let mut part = String::new();
                        for ch in delta.chars() {
                            if part.len() + ch.len_utf8() > limit && !part.is_empty() {
                                emit_delta(&r, std::mem::take(&mut part)).await?;
                            }
                            part.push(ch);
                        }
                        if !part.is_empty() {
                            emit_delta(&r, part).await?;
                        }
                    }
                    data.clear();
                } else if let Some(s) = current.strip_prefix("data:") {
                    data.push_str(s.strip_prefix(' ').unwrap_or(s));
                    data.push('\n');
                }
                line.clear();
            }
            if ended {
                break;
            }
        }
        if (!line.is_empty() || !data.is_empty()) && !ended {
            return Err(EngineError::new("protocol", "truncated SSE frame"));
        }
        acc.finish()
    }
}
async fn emit_delta(r: &InvokeRequest, text: String) -> Result<()> {
    // Planning JSON is evidence, never a candidate artifact or a stream increment.
    if r.node == "planner" {
        return Ok(());
    }
    r.event_sink
        .send(Event {
            task_id: r.task_id.clone(),
            node_id: Some(r.node.clone()),
            attempt_id: Some(r.attempt_id.clone()),
            sequence: r.sequence.fetch_add(1, Ordering::Relaxed),
            timestamp_ms: now_ms(),
            kind: "content_delta".into(),
            data: json!({"text":text,"stage":"candidate"}),
        })
        .await
}
