//! Model providers. A provider turns one prompt into one JSON text; it never sees tools,
//! the database or credentials of the platform, and its output is always validated by the
//! capability that called it.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("provider unavailable")]
    Unavailable,
    #[error("provider timed out")]
    Timeout,
    #[error("provider returned output that does not match the schema")]
    InvalidOutput,
    #[error("input too large")]
    InputTooLarge,
    #[error("provider misconfigured: {0}")]
    Config(&'static str),
}

impl AiError {
    /// Stable code for API errors and the suggestion record.
    pub const fn code(&self) -> &'static str {
        match self {
            AiError::Unavailable => "ai_unavailable",
            AiError::Timeout => "ai_timeout",
            AiError::InvalidOutput => "ai_invalid_output",
            AiError::InputTooLarge => "ai_input_too_large",
            AiError::Config(_) => "ai_unavailable",
        }
    }

    /// Worth another attempt (transport problems, malformed output).
    const fn retryable(&self) -> bool {
        matches!(
            self,
            AiError::Unavailable | AiError::Timeout | AiError::InvalidOutput
        )
    }
}

/// One model call: instructions, fenced untrusted input, and the JSON schema of the answer.
#[derive(Debug, Clone)]
pub struct ModelCall {
    /// Which capability is asking (lets the offline mock answer deterministically).
    pub capability: &'static str,
    pub system: String,
    pub user: String,
    pub schema: Value,
    pub max_output_tokens: u32,
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Provider: Send + Sync {
    /// Provider kind for provenance, e.g. `mock`, `openai_compat`.
    fn name(&self) -> &str;
    /// Model identifier for provenance.
    fn model(&self) -> &str;
    /// Returns the raw JSON text the model produced.
    fn complete<'a>(&'a self, call: &'a ModelCall) -> BoxFuture<'a, Result<String, AiError>>;
}

/// Calls the provider with a hard timeout, retrying transport errors and malformed output
/// (`parse` decides what is valid) up to `retries` more times.
pub async fn call_validated<T>(
    provider: &dyn Provider,
    call: &ModelCall,
    timeout: Duration,
    retries: u32,
    parse: impl Fn(&str) -> Result<T, AiError>,
) -> Result<T, AiError> {
    let mut attempt = 0;
    loop {
        let result = match tokio::time::timeout(timeout, provider.complete(call)).await {
            Err(_) => Err(AiError::Timeout),
            Ok(Err(e)) => Err(e),
            Ok(Ok(text)) => parse(&text),
        };
        match result {
            Err(e) if e.retryable() && attempt < retries => {
                attempt += 1;
                tracing::warn!(capability = call.capability, attempt, error = %e, "retrying model call");
            }
            other => return other,
        }
    }
}

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

/// Any OpenAI-compatible chat-completions endpoint (a local model server such as
/// llama.cpp/vLLM/Ollama, or a hosted API once the founder approves one). Structured output
/// via `response_format: json_schema`, temperature 0, no tools.
pub struct OpenAiCompat {
    http: reqwest::Client,
    endpoint: reqwest::Url,
    model: String,
    api_key: Option<String>,
}

impl OpenAiCompat {
    /// `base_url` must be https, or plain http to this machine (local model server).
    pub fn new(base_url: &str, model: &str, api_key: Option<String>) -> Result<Self, AiError> {
        let url = reqwest::Url::parse(base_url).map_err(|_| AiError::Config("base_url"))?;
        let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if !(url.scheme() == "https" || (url.scheme() == "http" && local)) {
            return Err(AiError::Config(
                "base_url must be https or a local http address",
            ));
        }
        if model.trim().is_empty() {
            return Err(AiError::Config("model"));
        }
        let endpoint = url
            .join(&format!(
                "{}/chat/completions",
                url.path().trim_end_matches('/')
            ))
            .map_err(|_| AiError::Config("base_url"))?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| AiError::Config("http client"))?;
        Ok(OpenAiCompat {
            http,
            endpoint,
            model: model.trim().to_string(),
            api_key: api_key.filter(|k| !k.is_empty()),
        })
    }
}

impl Provider for OpenAiCompat {
    fn name(&self) -> &str {
        "openai_compat"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn complete<'a>(&'a self, call: &'a ModelCall) -> BoxFuture<'a, Result<String, AiError>> {
        Box::pin(async move {
            let body = json!({
                "model": self.model,
                "temperature": 0,
                "max_tokens": call.max_output_tokens,
                "messages": [
                    { "role": "system", "content": call.system },
                    { "role": "user", "content": call.user },
                ],
                "response_format": {
                    "type": "json_schema",
                    "json_schema": { "name": call.capability, "strict": true, "schema": call.schema },
                },
            });
            let mut req = self.http.post(self.endpoint.clone()).json(&body);
            if let Some(key) = &self.api_key {
                req = req.bearer_auth(key);
            }
            let res = req.send().await.map_err(|e| {
                if e.is_timeout() {
                    AiError::Timeout
                } else {
                    AiError::Unavailable
                }
            })?;
            if !res.status().is_success() {
                tracing::warn!(status = res.status().as_u16(), "model endpoint error");
                return Err(AiError::Unavailable);
            }
            // Bounded read: a model answer is never more than a few hundred KB.
            let mut res = res;
            let mut body = Vec::new();
            while let Some(chunk) = res.chunk().await.map_err(|_| AiError::Unavailable)? {
                if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                    return Err(AiError::InvalidOutput);
                }
                body.extend_from_slice(&chunk);
            }
            let value: Value = serde_json::from_slice(&body).map_err(|_| AiError::InvalidOutput)?;
            value["choices"][0]["message"]["content"]
                .as_str()
                .map(str::to_owned)
                .ok_or(AiError::InvalidOutput)
        })
    }
}

/// Provider chosen by configuration: `PROCURE_AI_PROVIDER=mock` (default), `local` (a model
/// server on this machine: loopback address required) or `remote` (customer text leaves the
/// server: also requires `PROCURE_AI_REMOTE_APPROVED=yes`, set only after the founder approves
/// the provider). Plus `PROCURE_AI_BASE_URL`, `PROCURE_AI_MODEL`, optional `PROCURE_AI_API_KEY`.
pub fn from_env() -> Result<Box<dyn Provider>, AiError> {
    let kind = std::env::var("PROCURE_AI_PROVIDER").unwrap_or_else(|_| "mock".into());
    match kind.as_str() {
        "mock" => Ok(Box::new(crate::mock::MockProvider)),
        "local" | "remote" => {
            let base = std::env::var("PROCURE_AI_BASE_URL")
                .map_err(|_| AiError::Config("PROCURE_AI_BASE_URL"))?;
            let loopback = reqwest::Url::parse(&base)
                .is_ok_and(|u| matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")));
            if kind == "local" && !loopback {
                return Err(AiError::Config("local provider must be on this machine"));
            }
            if kind == "remote"
                && std::env::var("PROCURE_AI_REMOTE_APPROVED").as_deref() != Ok("yes")
            {
                return Err(AiError::Config(
                    "remote AI provider not approved (PROCURE_AI_REMOTE_APPROVED=yes)",
                ));
            }
            let model = std::env::var("PROCURE_AI_MODEL")
                .map_err(|_| AiError::Config("PROCURE_AI_MODEL"))?;
            let key = std::env::var("PROCURE_AI_API_KEY").ok();
            Ok(Box::new(OpenAiCompat::new(&base, &model, key)?))
        }
        _ => Err(AiError::Config("PROCURE_AI_PROVIDER")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_must_be_https_or_local() {
        assert!(OpenAiCompat::new("https://api.example.test/v1", "m", None).is_ok());
        assert!(OpenAiCompat::new("http://127.0.0.1:8080/v1", "m", None).is_ok());
        assert!(OpenAiCompat::new("http://10.0.0.5:8080/v1", "m", None).is_err());
        assert!(OpenAiCompat::new("ftp://x", "m", None).is_err());
        assert!(OpenAiCompat::new("https://api.example.test/v1", " ", None).is_err());
        let p = OpenAiCompat::new("http://localhost:8080/v1/", "m", None).unwrap();
        assert_eq!(
            p.endpoint.as_str(),
            "http://localhost:8080/v1/chat/completions"
        );
    }
}
