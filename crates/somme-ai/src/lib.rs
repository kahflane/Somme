//! Somme's AI parts, kept apart from the app so the background worker and,
//! later, the runners use the same code:
//!
//! - [`llm`]: any OpenAI-compatible chat server, with tools and images.
//! - [`jev`]: TypeSafe's Jev, which answers yes/no, pick-one and score
//!   questions with probabilities.
//! - [`decide`]: one question, answered by Jev, else by an LLM.
//! - [`guard`]: checks an action against the user's rules before it runs.
//! - [`clean`]: takes secrets out of text before it is sent anywhere.

pub mod clean;
pub mod decide;
pub mod guard;
pub mod jev;
pub mod llm;

use std::time::Duration;

/// Why an AI call gave no answer, in words the user can act on.
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("could not reach {service}: {reason}")]
    Network { service: &'static str, reason: String },
    #[error("{service} answered {status}: {body}")]
    Status { service: &'static str, status: u16, body: String },
    #[error("{service}'s answer could not be read: {reason}")]
    Answer { service: &'static str, reason: String },
}

impl AiError {
    fn network(service: &'static str, err: impl std::fmt::Display) -> Self {
        Self::Network { service, reason: err.to_string() }
    }

    fn answer(service: &'static str, reason: impl Into<String>) -> Self {
        Self::Answer { service, reason: reason.into() }
    }

    /// Busy or rate limited: worth trying again after a pause.
    fn retryable(&self) -> bool {
        match self {
            Self::Network { .. } => true,
            Self::Status { status, .. } => matches!(status, 429 | 500 | 502 | 503 | 504 | 529),
            Self::Answer { .. } => false,
        }
    }
}

/// A short, safe excerpt of an error body for messages.
fn excerpt(body: &str) -> String {
    let body = body.trim();
    let mut end = body.len().min(300);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    body[..end].to_owned()
}

/// Runs `call` up to three times while it fails in a way worth retrying.
async fn with_retries<T, F, Fut>(mut call: F) -> Result<T, AiError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, AiError>>,
{
    let mut pause = Duration::from_millis(500);
    for _ in 0..2 {
        match call().await {
            Err(err) if err.retryable() => {
                tracing::debug!(%err, "trying again");
                tokio::time::sleep(pause).await;
                pause *= 2;
            }
            other => return other,
        }
    }
    call().await
}

#[cfg(test)]
pub(crate) mod test_server {
    //! A one-shot HTTP server that gives canned answers, for client tests.

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    /// Serves `answers` in order, one per connection, and gives back the
    /// address and what each request sent.
    pub async fn serve(answers: Vec<(u16, String)>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (status, body) in answers {
                let (mut stream, _) = listener.accept().await.unwrap();
                seen.push(read_request(&mut stream).await);
                // A streamed answer is a run of `data:` lines.
                let kind = if body.starts_with("data:") { "text/event-stream" } else { "application/json" };
                let head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).await.unwrap();
                stream.write_all(body.as_bytes()).await.unwrap();
                stream.shutdown().await.ok();
            }
            seen
        });
        (address, task)
    }

    async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&data);
            if let Some(split) = text.find("\r\n\r\n") {
                let length = text[..split]
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if data.len() >= split + 4 + length {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&data).into_owned()
    }
}
