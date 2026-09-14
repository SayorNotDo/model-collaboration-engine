use crate::contracts::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub task_id: String,
    pub node_id: Option<String>,
    pub attempt_id: Option<String>,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub kind: String,
    pub data: Value,
}
#[derive(Clone)]
pub struct EventSink {
    pub sender: Option<mpsc::Sender<Event>>,
    pub max_bytes: usize,
    pub cancel: CancellationToken,
}
impl EventSink {
    pub async fn send(&self, event: Event) -> Result<()> {
        if serde_json::to_vec(&event).unwrap().len() > self.max_bytes {
            return Err(EngineError::new(
                "protocol",
                "event exceeds configured size limit",
            ));
        }
        if let Some(sender) = &self.sender {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Err(EngineError::new("cancelled", "task cancelled")),
                _ = sender.closed() => {},
                _ = sender.send(event) => {},
            }
        }
        Ok(())
    }
}
