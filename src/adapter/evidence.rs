//! Received billing evidence outlives the adapter future, including cancellation.
use crate::contracts::Usage;
use std::sync::{Arc, Mutex};

/// Per-invocation evidence shared with the engine's settlement boundary.
/// Adapters should record known usage before parsing dependent output or awaiting events.
#[derive(Clone, Default)]
pub struct ModelEvidence(Arc<Mutex<ReceivedEvidence>>);

#[derive(Clone, Default)]
pub(crate) struct ReceivedEvidence {
    pub usage: Option<Usage>,
    pub request_id: Option<String>,
}

impl ModelEvidence {
    /// Missing fields do not erase previously received evidence.
    pub fn record(&self, usage: Option<Usage>, request_id: Option<String>) {
        let mut received = self.0.lock().unwrap();
        if let Some(usage) = usage {
            received.usage = Some(usage);
        }
        if let Some(request_id) = request_id {
            received.request_id = Some(request_id);
        }
    }

    pub(crate) fn snapshot(&self) -> ReceivedEvidence {
        self.0.lock().unwrap().clone()
    }
}
