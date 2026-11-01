//! A provider that replays scripted answers, for tests of validation and of adversarial
//! (prompt-injected or hallucinating) model output.

use std::collections::VecDeque;
use std::sync::Mutex;

use crate::provider::{AiError, BoxFuture, ModelCall, Provider};

#[derive(Default)]
pub struct ScriptedProvider {
    answers: Mutex<VecDeque<Result<String, &'static str>>>,
    /// Every prompt received, for assertions about fencing.
    pub calls: Mutex<Vec<ModelCall>>,
}

impl ScriptedProvider {
    /// `Ok(text)` is returned as model output; `Err("timeout" | "unavailable")` as an error.
    pub fn new(answers: Vec<Result<String, &'static str>>) -> Self {
        ScriptedProvider {
            answers: Mutex::new(answers.into()),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    fn model(&self) -> &str {
        "scripted-test"
    }

    fn complete<'a>(&'a self, call: &'a ModelCall) -> BoxFuture<'a, Result<String, AiError>> {
        Box::pin(async move {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(call.clone());
            }
            let next = self
                .answers
                .lock()
                .map_err(|_| AiError::Unavailable)?
                .pop_front();
            match next {
                Some(Ok(text)) => Ok(text),
                Some(Err("timeout")) => Err(AiError::Timeout),
                _ => Err(AiError::Unavailable),
            }
        })
    }
}
