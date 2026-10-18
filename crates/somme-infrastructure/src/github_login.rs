//! Signs in with GitHub through the browser. The app waits on a loopback port,
//! opens GitHub's sign-in page, and GitHub sends the browser back to that port
//! with a one-time code. PKCE ties the code to this app, so a code caught on
//! the way is useless to anyone else.

use std::net::Ipv4Addr;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng as _;
use reqwest::Url;
use sha2::{Digest as _, Sha256};
use somme_application::ports::CloudError;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use zeroize::Zeroizing;

const AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
/// GitHub's codes last ten minutes, so there is no point waiting longer.
const WAIT: Duration = Duration::from_secs(10 * 60);
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REQUEST: usize = 16 * 1024;

/// What Somme Cloud needs to finish the sign-in.
pub struct Grant {
    pub code: Zeroizing<String>,
    pub redirect_uri: String,
    pub code_verifier: Zeroizing<String>,
}

fn unreachable(err: impl std::fmt::Display) -> CloudError {
    CloudError::Unreachable(err.to_string())
}

fn random() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Opens GitHub's sign-in page with `open` and waits for the browser to come back.
pub async fn sign_in(client_id: &str, open: impl FnOnce(&str) -> std::io::Result<()>) -> Result<Grant, CloudError> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.map_err(unreachable)?;
    let port = listener.local_addr().map_err(unreachable)?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let state = random();
    let code_verifier = Zeroizing::new(random());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
    let url = Url::parse_with_params(
        AUTHORIZE_URL,
        [
            ("client_id", client_id),
            ("redirect_uri", &redirect_uri),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ],
    )
    .map_err(unreachable)?;
    open(url.as_str()).map_err(|err| CloudError::Unreachable(format!("cannot open the browser: {err}")))?;
    let code = tokio::time::timeout(WAIT, wait_for_code(&listener, &state))
        .await
        .map_err(|_| CloudError::Rejected("GitHub sign-in timed out".into()))??;
    Ok(Grant {
        code,
        redirect_uri,
        code_verifier,
    })
}

/// Answers browser requests until one brings this sign-in's code. Anything
/// else, like a stale tab from an earlier try, gets an error page.
async fn wait_for_code(listener: &TcpListener, state: &str) -> Result<Zeroizing<String>, CloudError> {
    loop {
        let (mut stream, _) = listener.accept().await.map_err(unreachable)?;
        let Ok(Some(target)) = tokio::time::timeout(READ_TIMEOUT, request_target(&mut stream)).await else {
            continue;
        };
        let Some(query) = callback_query(&target) else {
            reply(&mut stream, "404 Not Found", "Not found", "").await;
            continue;
        };
        let param = |name: &str| query.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
        if param("state") != Some(state) {
            reply(&mut stream, "400 Bad Request", "This sign-in has expired", "Start again from Somme.").await;
            continue;
        }
        if let Some(code) = param("code").filter(|code| !code.is_empty()) {
            let code = Zeroizing::new(code.to_owned());
            reply(&mut stream, "200 OK", "Signed in to Somme", "You can close this tab and go back to the app.").await;
            return Ok(code);
        }
        reply(&mut stream, "200 OK", "Sign-in cancelled", "You can close this tab.").await;
        return Err(match param("error") {
            Some("access_denied") => CloudError::Rejected("GitHub sign-in was cancelled".into()),
            _ => CloudError::Rejected("GitHub did not sign you in".into()),
        });
    }
}

/// Reads a request's head and gives back its target, such as `/callback?code=…`.
async fn request_target(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).await.ok().filter(|&n| n > 0)?;
        head.extend_from_slice(&chunk[..read]);
        if head.len() > MAX_REQUEST {
            return None;
        }
    }
    let line = String::from_utf8_lossy(&head);
    let mut parts = line.lines().next()?.split_whitespace();
    (parts.next() == Some("GET")).then_some(())?;
    parts.next().map(ToOwned::to_owned)
}

fn callback_query(target: &str) -> Option<Vec<(String, String)>> {
    let url = Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    (url.path() == "/callback").then(|| url.query_pairs().into_owned().collect())
}

async fn reply(stream: &mut TcpStream, status: &str, title: &str, text: &str) {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
         <style>body{{font:16px -apple-system,system-ui,sans-serif;display:flex;align-items:center;\
         justify-content:center;height:100vh;margin:0;background:#16161a;color:#e8e8ea}}\
         p{{color:#9a9aa2}}</style></head><body><div><h2>{title}</h2><p>{text}</p></div></body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays the browser: sends each request to the page GitHub would send it
    /// back to, and returns the page's replies.
    fn browser(requests: Vec<String>) -> (impl FnOnce(&str) -> std::io::Result<()>, tokio::sync::oneshot::Receiver<Vec<String>>) {
        let (done, replies) = tokio::sync::oneshot::channel();
        let open = move |page: &str| {
            let url = Url::parse(page).unwrap();
            let param = |name: &str| url.query_pairs().find(|(key, _)| key == name).unwrap().1.into_owned();
            let (redirect, state) = (Url::parse(&param("redirect_uri")).unwrap(), param("state"));
            assert_eq!(param("code_challenge_method"), "S256");
            tokio::spawn(async move {
                let mut replies = Vec::new();
                for request in requests {
                    let mut stream = TcpStream::connect(("127.0.0.1", redirect.port().unwrap())).await.unwrap();
                    let target = request.replace("STATE", &state);
                    stream.write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).await.unwrap();
                    let mut reply = String::new();
                    stream.read_to_string(&mut reply).await.unwrap();
                    replies.push(reply);
                }
                let _ = done.send(replies);
            });
            Ok(())
        };
        (open, replies)
    }

    #[tokio::test]
    async fn waits_past_stray_requests_for_the_code() {
        let (open, replies) = browser(vec![
            "/favicon.ico".into(),
            "/callback?code=stale&state=old".into(),
            "/callback?code=good%2Bcode&state=STATE".into(),
        ]);
        let grant = sign_in("client", open).await.unwrap();
        assert_eq!(grant.code.as_str(), "good+code");
        assert!(grant.redirect_uri.starts_with("http://127.0.0.1:"));
        assert_eq!(grant.code_verifier.len(), 43);
        let replies = replies.await.unwrap();
        assert!(replies[0].starts_with("HTTP/1.1 404"));
        assert!(replies[1].starts_with("HTTP/1.1 400"));
        assert!(replies[2].starts_with("HTTP/1.1 200") && replies[2].contains("Signed in"));
    }

    #[tokio::test]
    async fn declining_on_github_stops_the_sign_in() {
        let (open, _) = browser(vec!["/callback?error=access_denied&state=STATE".into()]);
        assert!(matches!(sign_in("client", open).await, Err(CloudError::Rejected(_))));
    }
}
