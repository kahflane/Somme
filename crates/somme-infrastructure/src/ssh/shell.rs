use std::sync::Arc;

use async_trait::async_trait;
use russh::client::Msg;
use russh::{ChannelMsg, ChannelWriteHalf};
use somme_application::ports::{GatewayError, OutputSink, TermSize, Terminal};

use crate::ssh::connect::Connection;
use crate::ssh::{connection_error, session_error};

const TERM: &str = "xterm-256color";

pub struct SshTerminal {
    conn: Arc<Connection>,
    writer: ChannelWriteHalf<Msg>,
}

impl SshTerminal {
    pub async fn start(conn: Connection, size: TermSize, sink: Arc<dyn OutputSink>) -> Result<Self, GatewayError> {
        let channel = conn
            .target()
            .channel_open_session()
            .await
            .map_err(connection_error)?;
        channel
            .request_pty(false, TERM, u32::from(size.cols), u32::from(size.rows), 0, 0, &[])
            .await
            .map_err(session_error)?;
        channel.request_shell(false).await.map_err(session_error)?;
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
        Ok(Self { conn, writer })
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
}
