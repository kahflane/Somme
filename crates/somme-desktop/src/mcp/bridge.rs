//! `Somme --mcp`: what agents start. It passes their messages to Somme and
//! its answers back, and does nothing else. Somme may change hands while an
//! agent works, from the background service to the app as it opens. The
//! bridge then connects again and says hello again for the agent, so the
//! agent does not notice, apart from a script that was stopped.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Lines};
use tokio::sync::mpsc;

use super::transport::{self, Stream};

pub const NOT_RUNNING: &str = "Somme is not running, or agents are turned off. Open Somme and turn on Settings → Agents → Let agents run scripts.";

/// How long to wait for Somme when the agent starts, and when Somme changes hands.
const FIRST_WAIT: Duration = Duration::from_secs(3);
const HANDOVER_WAIT: Duration = Duration::from_secs(30);
/// The id of the hello the bridge sends again. Its answer is the bridge's own.
const REPLAY_ID: &str = "somme-bridge-hello";

pub fn run(dir: &Path) -> i32 {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("somme: {err}");
            return 1;
        }
    };
    let code = runtime.block_on(bridge(dir));
    // Reading the input may still be waiting, so leave without it.
    runtime.shutdown_background();
    code
}

async fn connect(dir: &Path, wait: Duration) -> Option<Stream> {
    let began = Instant::now();
    loop {
        if let Ok(stream) = transport::connect(dir).await {
            return Some(stream);
        }
        if began.elapsed() >= wait {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The ids of the requests in `message`, a request or a batch.
fn request_ids(message: &Value) -> Vec<Value> {
    let one = |item: &Value| (item.get("method").is_some()).then(|| item.get("id").cloned()).flatten();
    match message {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        item => one(item).into_iter().collect(),
    }
}

fn answer_ids(message: &Value) -> Vec<Value> {
    let one = |item: &Value| (item.get("method").is_none()).then(|| item.get("id").cloned()).flatten();
    match message {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        item => one(item).into_iter().collect(),
    }
}

fn key(id: &Value) -> String {
    id.to_string()
}

struct Link {
    lines: Lines<BufReader<tokio::io::ReadHalf<Stream>>>,
    write: tokio::io::WriteHalf<Stream>,
}

impl Link {
    fn new(stream: Stream) -> Self {
        let (read, write) = tokio::io::split(stream);
        Self {
            lines: BufReader::new(read).lines(),
            write,
        }
    }

    async fn send(&mut self, line: &str) -> std::io::Result<()> {
        self.write.write_all(line.as_bytes()).await?;
        self.write.write_all(b"\n").await?;
        self.write.flush().await
    }

    /// Says hello again as the agent did, and waits for the answer.
    async fn replay(&mut self, hello: &Value) -> Option<()> {
        let mut hello = hello.clone();
        hello["id"] = json!(REPLAY_ID);
        self.send(&hello.to_string()).await.ok()?;
        let answered = tokio::time::timeout(Duration::from_secs(5), async {
            while let Ok(Some(line)) = self.lines.next_line().await {
                if serde_json::from_str::<Value>(&line).is_ok_and(|v| v["id"] == REPLAY_ID) {
                    return Some(());
                }
            }
            None
        })
        .await
        .ok()
        .flatten();
        answered?;
        let ready = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        self.send(&ready.to_string()).await.ok()
    }
}

async fn bridge(dir: &Path) -> i32 {
    let Some(stream) = connect(dir, FIRST_WAIT).await else {
        eprintln!("somme: {NOT_RUNNING}");
        return 1;
    };
    let mut link = Some(Link::new(stream));
    // One reader for the whole run, so no line is lost between connections.
    let (lines, mut input) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let mut stdin = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = stdin.next_line().await {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdout = tokio::io::stdout();
    let mut hello: Option<Value> = None;
    let mut pending: HashSet<String> = HashSet::new();
    let mut input_done = false;
    loop {
        let Some(current) = link.as_mut() else {
            // Somme changed hands, or closed.
            for id in pending.drain() {
                let id: Value = serde_json::from_str(&id).unwrap_or(Value::Null);
                let answer = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000,
                    "message": "Somme stopped while this ran, as it closed or changed hands. If it was a script, check the script log in Somme to see if it ended." } });
                let _ = stdout.write_all(format!("{answer}\n").as_bytes()).await;
            }
            let _ = stdout.flush().await;
            if input_done {
                return 0;
            }
            let Some(stream) = connect(dir, HANDOVER_WAIT).await else {
                eprintln!("somme: {NOT_RUNNING}");
                return 1;
            };
            let mut fresh = Link::new(stream);
            if let Some(hello) = &hello
                && fresh.replay(hello).await.is_none()
            {
                eprintln!("somme: Somme did not answer after changing hands.");
                return 1;
            }
            link = Some(fresh);
            continue;
        };
        tokio::select! {
            line = input.recv(), if !input_done => match line {
                Some(line) => {
                    if let Ok(message) = serde_json::from_str::<Value>(&line) {
                        if message.get("method").and_then(Value::as_str) == Some("initialize") {
                            hello = Some(message.clone());
                        }
                        pending.extend(request_ids(&message).iter().map(key));
                    }
                    if current.send(&line).await.is_err() {
                        link = None;
                    }
                }
                None => {
                    // The agent is done; Somme finishes what it was asked and closes.
                    input_done = true;
                    let _ = current.write.shutdown().await;
                }
            },
            line = current.lines.next_line() => match line {
                Ok(Some(line)) => {
                    if let Ok(message) = serde_json::from_str::<Value>(&line) {
                        for id in answer_ids(&message) {
                            pending.remove(&key(&id));
                        }
                    }
                    if stdout.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                        return 0;
                    }
                    let _ = stdout.flush().await;
                }
                _ => link = None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_answers_are_told_apart() {
        let call = json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call" });
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let answer = json!({ "jsonrpc": "2.0", "id": 7, "result": {} });
        assert_eq!(request_ids(&call), [json!(7)]);
        assert!(request_ids(&note).is_empty());
        assert!(request_ids(&answer).is_empty());
        assert_eq!(answer_ids(&answer), [json!(7)]);
        assert_eq!(request_ids(&json!([call, note, { "id": "a", "method": "ping" }])), [json!(7), json!("a")]);
    }
}
