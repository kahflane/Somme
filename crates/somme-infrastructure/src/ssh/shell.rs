use std::sync::Arc;

use async_trait::async_trait;
use russh::client::Msg;
use russh::{ChannelMsg, ChannelWriteHalf};
use somme_application::ports::{GatewayError, OutputSink, RemoteFs, TermSize, Terminal};
use tokio::sync::OnceCell;

use crate::ssh::connect::Connection;
use crate::ssh::sftp::SftpFs;
use crate::ssh::{connection_error, hooks, session_error};

const TERM: &str = "xterm-256color";

pub struct SshTerminal {
    conn: Arc<Connection>,
    writer: ChannelWriteHalf<Msg>,
    hooks: bool,
    /// Opened on first use. None once the server refused it.
    files: OnceCell<Option<Arc<SftpFs>>>,
}

impl SshTerminal {
    /// Starts the login shell, with Somme's hooks when `marks` holds their
    /// secret and the server can take them.
    pub async fn start(
        conn: Connection,
        size: TermSize,
        marks: Option<&str>,
        sink: Arc<dyn OutputSink>,
    ) -> Result<Self, GatewayError> {
        let start = match marks {
            Some(token) => hooks::prepare(&conn, token).await,
            None => None,
        };
        let channel = conn
            .target()
            .channel_open_session()
            .await
            .map_err(connection_error)?;
        channel
            .request_pty(false, TERM, u32::from(size.cols), u32::from(size.rows), 0, 0, &[])
            .await
            .map_err(session_error)?;
        match &start {
            Some(command) => channel.exec(false, command.as_str()).await,
            None => channel.request_shell(false).await,
        }
        .map_err(session_error)?;
        let banner = conn.take_banner();
        if !banner.is_empty() {
            sink.data(screen_text(&banner).as_bytes());
        }
        let (mut reader, writer) = channel.split();
        let conn = Arc::new(conn);
        let watched = Arc::clone(&conn);
        tokio::spawn(async move {
            let mut reason = Some("connection lost".to_owned());
            while let Some(msg) = reader.wait().await {
                match msg {
                    ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => sink.data(&data),
                    ChannelMsg::ExitStatus { exit_status } => {
                        reason = (exit_status != 0).then(|| format!("exited with code {exit_status}"));
                    }
                    ChannelMsg::Eof => {
                        if reason.as_deref() == Some("connection lost") {
                            reason = None;
                        }
                    }
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            watched.disconnect().await;
            sink.closed(reason);
        });
        Ok(Self {
            conn,
            writer,
            hooks: start.is_some(),
            files: OnceCell::new(),
        })
    }
}

#[async_trait]
impl Terminal for SshTerminal {
    async fn write(&self, data: &[u8]) -> Result<(), GatewayError> {
        self.writer.data(data).await.map_err(session_error)
    }

    async fn resize(&self, size: TermSize) -> Result<(), GatewayError> {
        self.writer
            .window_change(u32::from(size.cols), u32::from(size.rows), 0, 0)
            .await
            .map_err(session_error)
    }

    async fn close(&self) {
        let _ = self.writer.close().await;
        self.conn.disconnect().await;
    }

    fn has_hooks(&self) -> bool {
        self.hooks
    }

    async fn files(&self) -> Option<Arc<dyn RemoteFs>> {
        let files = self
            .files
            .get_or_init(|| async {
                match SftpFs::beside(Arc::clone(&self.conn)).await {
                    Ok(files) => Some(Arc::new(files)),
                    Err(err) => {
                        tracing::info!(%err, "no file access beside the terminal");
                        None
                    }
                }
            })
            .await;
        files.clone().map(|files| files as Arc<dyn RemoteFs>)
    }
}

/// Server text made safe to print, the way OpenSSH prints a banner: control
/// characters other than tabs and line breaks are dropped, and each line ends
/// with a carriage return too, since no terminal driver adds one.
fn screen_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\r\n"),
            '\t' => out.push('\t'),
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_text_cannot_steer_the_terminal() {
        assert_eq!(screen_text("Authorized use only\r\n\x1b]52;c;aGk=\x07\tbye\n"), "Authorized use only\r\n]52;c;aGk=\tbye\r\n");
    }
}
