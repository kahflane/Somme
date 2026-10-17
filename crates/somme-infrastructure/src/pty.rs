use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use somme_application::ports::{GatewayError, OutputSink, PtyGateway, TermSize, Terminal};

const READ_BUFFER: usize = 16 * 1024;

fn session_error(err: impl std::fmt::Display) -> GatewayError {
    GatewayError::Session(err.to_string())
}

fn pty_size(size: TermSize) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NativePtyGateway;

struct LocalTerminal {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

#[async_trait]
impl Terminal for LocalTerminal {
    async fn write(&self, data: &[u8]) -> Result<(), GatewayError> {
        let mut writer = self.writer.lock().map_err(session_error)?;
        writer.write_all(data).map_err(session_error)?;
        writer.flush().map_err(session_error)
    }

    async fn resize(&self, size: TermSize) -> Result<(), GatewayError> {
        self.master
            .lock()
            .map_err(session_error)?
            .resize(pty_size(size))
            .map_err(session_error)
    }

    async fn close(&self) {
        if let Ok(mut killer) = self.killer.lock() {
            let _ = killer.kill();
        }
    }
}

impl PtyGateway for NativePtyGateway {
    fn spawn(&self, size: TermSize, sink: Arc<dyn OutputSink>) -> Result<Arc<dyn Terminal>, GatewayError> {
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(session_error)?;
        let mut cmd = CommandBuilder::new_default_prog();
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if let Some(home) = dirs::home_dir() {
            cmd.cwd(home);
        }
        let mut child = pair.slave.spawn_command(cmd).map_err(session_error)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(session_error)?;
        let writer = pair.master.take_writer().map_err(session_error)?;
        let killer = child.clone_killer();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; READ_BUFFER];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => sink.data(&buf[..n]),
                }
            }
            let reason = child
                .wait()
                .ok()
                .filter(|status| !status.success())
                .map(|status| format!("shell exited with code {}", status.exit_code()));
            sink.closed(reason);
        });
        Ok(Arc::new(LocalTerminal {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
        }))
    }
}
