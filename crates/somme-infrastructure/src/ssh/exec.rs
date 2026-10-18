//! Runs a script on its own channel, with no terminal, and collects what it
//! prints. Agents use this through Somme's MCP server.

use std::collections::VecDeque;
use std::time::Duration;

use russh::{ChannelMsg, Sig};
use somme_application::ports::{GatewayError, ScriptOutput};

use crate::ssh::connect::Connection;
use crate::ssh::{connection_error, session_error};

pub async fn run(conn: &Connection, script: &str, timeout: Duration, limit: usize) -> Result<ScriptOutput, GatewayError> {
    let mut channel = conn.target().channel_open_session().await.map_err(connection_error)?;
    channel.exec(true, script).await.map_err(session_error)?;
    // Nothing comes on its input, so a command that reads it ends instead of waiting.
    channel.eof().await.map_err(session_error)?;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut stdout = Capped::new(limit);
    let mut stderr = Capped::new(limit);
    let mut out = ScriptOutput::default();
    loop {
        match tokio::time::timeout_at(deadline, channel.wait()).await {
            Ok(Some(ChannelMsg::Data { data })) => stdout.push(&data),
            Ok(Some(ChannelMsg::ExtendedData { data, .. })) => stderr.push(&data),
            Ok(Some(ChannelMsg::ExitStatus { exit_status })) => out.exit_status = Some(exit_status),
            Ok(Some(ChannelMsg::ExitSignal { signal_name, .. })) => out.exit_signal = Some(signal_label(signal_name)),
            Ok(Some(ChannelMsg::Failure)) => {
                return Err(GatewayError::Session("the server would not run the script".into()));
            }
            Ok(Some(ChannelMsg::Close) | None) => break,
            Ok(Some(_)) => {}
            Err(_) => {
                out.timed_out = true;
                // Servers that take signals stop it; closing the channel covers the rest.
                let _ = channel.signal(Sig::KILL).await;
                let _ = channel.close().await;
                break;
            }
        }
    }
    out.stdout = stdout.finish();
    out.stderr = stderr.finish();
    Ok(out)
}

fn signal_label(signal: Sig) -> String {
    match signal {
        Sig::Custom(name) => name,
        other => format!("{other:?}"),
    }
}

/// Output that keeps its start and its end once it grows past the limit,
/// since errors and summaries tend to come last.
struct Capped {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    head_max: usize,
    tail_max: usize,
    cut: u64,
}

impl Capped {
    fn new(limit: usize) -> Self {
        let head_max = limit / 4;
        Self {
            head: Vec::new(),
            tail: VecDeque::new(),
            head_max,
            tail_max: limit - head_max,
            cut: 0,
        }
    }

    fn push(&mut self, mut data: &[u8]) {
        if self.head.len() < self.head_max {
            let take = (self.head_max - self.head.len()).min(data.len());
            self.head.extend_from_slice(&data[..take]);
            data = &data[take..];
        }
        self.tail.extend(data);
        let over = self.tail.len().saturating_sub(self.tail_max);
        if over > 0 {
            self.tail.drain(..over);
            self.cut += over as u64;
        }
    }

    fn finish(self) -> Vec<u8> {
        let mut out = self.head;
        if self.cut > 0 {
            out.extend_from_slice(format!("\n[… {} bytes cut …]\n", self.cut).as_bytes());
        }
        out.extend(self.tail);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_output_keeps_its_start_and_end() {
        let mut out = Capped::new(8);
        out.push(b"ab");
        out.push(b"cdefghij");
        out.push(b"kl");
        assert_eq!(out.finish(), b"ab\n[\xe2\x80\xa6 4 bytes cut \xe2\x80\xa6]\nghijkl".to_vec());

        let mut short = Capped::new(8);
        short.push(b"hello");
        assert_eq!(short.finish(), b"hello".to_vec());
    }
}
