//! How agents reach the MCP server. On macOS and Linux it is a Unix socket in
//! a folder only this user may open, and other users' connections are
//! dropped. Windows has no such socket here, so it is a local TCP port, and
//! each connection must first send a secret kept in a file only this user
//! may read.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};

pub trait Conn: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Conn for T {}

pub type Stream = Box<dyn Conn>;

const SOCKET: &str = "mcp.sock";
/// The TCP port and its secret.
#[cfg_attr(unix, allow(dead_code))]
const ENDPOINT: &str = "endpoint.json";

#[derive(serde::Serialize, serde::Deserialize)]
#[cfg_attr(unix, allow(dead_code))]
struct Endpoint {
    port: u16,
    token: String,
}

pub enum Listener {
    #[cfg(unix)]
    Unix { listener: tokio::net::UnixListener, owner: u32 },
    #[cfg_attr(unix, allow(dead_code))]
    Tcp { listener: tokio::net::TcpListener, token: String },
}

/// What [`Listener::bind`] made, so it can be removed again, and only if no
/// newer server replaced it.
pub struct Mark {
    path: PathBuf,
    stamp: Option<String>,
}

impl Mark {
    pub fn remove(&self) {
        if stamp(&self.path) == self.stamp {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Something that tells this file apart from one made later at the same path.
fn stamp(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Some(format!("{}:{}", meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        std::fs::read_to_string(path).ok().map(|text| format!("{:?}{text}", meta.modified().ok()))
    }
}

impl Listener {
    /// Starts taking agents for the agents folder `dir`. Must run inside the
    /// IO runtime. Fails when another Somme already takes them.
    pub fn bind(dir: &Path) -> io::Result<(Self, Mark)> {
        #[cfg(unix)]
        return Self::bind_unix(dir);
        #[cfg(not(unix))]
        return Self::bind_tcp(dir);
    }

    #[cfg(unix)]
    fn bind_unix(dir: &Path) -> io::Result<(Self, Mark)> {
        use std::os::unix::fs::MetadataExt as _;

        let path = dir.join(SOCKET);
        if path.exists() {
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                return Err(io::Error::other("another copy of Somme is already taking agents"));
            }
            // Left behind by a Somme that did not quit cleanly.
            std::fs::remove_file(&path)?;
        }
        let owner = std::fs::metadata(dir)?.uid();
        let listener = std::os::unix::net::UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let listener = tokio::net::UnixListener::from_std(listener)?;
        let mark = Mark {
            stamp: stamp(&path),
            path,
        };
        Ok((Listener::Unix { listener, owner }, mark))
    }

    #[cfg_attr(unix, allow(dead_code))]
    pub fn bind_tcp(dir: &Path) -> io::Result<(Self, Mark)> {
        let path = dir.join(ENDPOINT);
        if let Some(endpoint) = read_endpoint(&path)
            && std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], endpoint.port).into(), Duration::from_millis(300))
                .is_ok()
        {
            return Err(io::Error::other("another copy of Somme is already taking agents"));
        }
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let raw = serde_json::to_string(&Endpoint {
            port,
            token: token.clone(),
        })
        .map_err(io::Error::other)?;
        write_private(&path, &raw)?;
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let mark = Mark {
            stamp: stamp(&path),
            path,
        };
        Ok((Listener::Tcp { listener, token }, mark))
    }

    /// The next agent. `None` when the one who knocked may not come in.
    pub async fn accept(&self) -> io::Result<Option<Stream>> {
        match self {
            #[cfg(unix)]
            Listener::Unix { listener, owner } => {
                let (stream, _) = listener.accept().await?;
                if stream.peer_cred().map(|cred| cred.uid()).ok() != Some(*owner) {
                    tracing::warn!("refused an agent connection from another user");
                    return Ok(None);
                }
                Ok(Some(Box::new(stream)))
            }
            Listener::Tcp { listener, token } => {
                let (stream, _) = listener.accept().await?;
                let mut stream = BufReader::new(stream);
                let mut line = Vec::new();
                let read = tokio::time::timeout(Duration::from_secs(5), async {
                    use tokio::io::AsyncReadExt as _;
                    (&mut stream).take(256).read_until(b'\n', &mut line).await
                })
                .await;
                let given = String::from_utf8_lossy(&line);
                if !matches!(read, Ok(Ok(_))) || !same(given.trim_end().as_bytes(), token.as_bytes()) {
                    tracing::warn!("refused an agent connection without the secret");
                    return Ok(None);
                }
                Ok(Some(Box::new(stream)))
            }
        }
    }
}

/// Reaches the server for the agents folder `dir`, as the bridge does.
pub async fn connect(dir: &Path) -> io::Result<Stream> {
    #[cfg(unix)]
    return Ok(Box::new(tokio::net::UnixStream::connect(dir.join(SOCKET)).await?));
    #[cfg(not(unix))]
    return connect_tcp(dir).await;
}

#[cfg_attr(unix, allow(dead_code))]
pub async fn connect_tcp(dir: &Path) -> io::Result<Stream> {
    let endpoint = read_endpoint(&dir.join(ENDPOINT)).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", endpoint.port)).await?;
    stream.write_all(format!("{}\n", endpoint.token).as_bytes()).await?;
    Ok(Box::new(stream))
}

#[cfg_attr(unix, allow(dead_code))]
fn read_endpoint(path: &Path) -> Option<Endpoint> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Compares secrets in the same time however much of them matches.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

#[cfg(unix)]
#[cfg_attr(unix, allow(dead_code))]
fn write_private(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let _ = std::fs::remove_file(path);
    let mut file = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(path)?;
    file.write_all(text.as_bytes())
}

/// The data folder sits in the user's own profile, which only they may read.
#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> io::Result<()> {
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    #[tokio::test]
    async fn the_port_lets_in_only_who_knows_the_secret() {
        let dir = tempfile::tempdir().unwrap();
        let (listener, mark) = Listener::bind_tcp(dir.path()).unwrap();
        let server = tokio::spawn(async move {
            let mut answers = Vec::new();
            for _ in 0..2 {
                answers.push(match listener.accept().await.unwrap() {
                    Some(mut stream) => {
                        stream.write_all(b"hello\n").await.unwrap();
                        true
                    }
                    None => false,
                });
            }
            (answers, listener)
        });
        let port = read_endpoint(&dir.path().join(ENDPOINT)).unwrap().port;
        let mut stranger = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stranger.write_all(b"guess\n").await.unwrap();
        let mut agent = connect_tcp(dir.path()).await.unwrap();
        let mut greeting = [0; 6];
        agent.read_exact(&mut greeting).await.unwrap();
        assert_eq!(&greeting, b"hello\n");
        let (answers, listener) = server.await.unwrap();
        assert_eq!(answers, [false, true]);
        assert!(Listener::bind_tcp(dir.path()).is_err(), "a second server must not start");
        drop(listener);
        mark.remove();
        assert!(!dir.path().join(ENDPOINT).exists());
    }

    #[test]
    fn secrets_compare_whole() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
    }
}
