//! Steps that reach the network from this computer: an HTTP request, a TLS
//! certificate check and a DNS lookup. They run the same in the app and in
//! the background worker.

use std::str::FromStr as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use serde_json::{Value, json};

use crate::nodes::OUTPUT_LIMIT;

/// What an HTTP request got back.
pub struct HttpReply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Sends one request. `headers` holds one `Name: value` per line.
pub async fn http(method: &str, url: &str, headers: &str, body: &str, timeout: Duration) -> Result<HttpReply, String> {
    let method = reqwest::Method::from_bytes(method.trim().to_uppercase().as_bytes()).map_err(|_| format!("\u{201c}{method}\u{201d} is not an HTTP method."))?;
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(15).min(timeout))
        .user_agent(concat!("Somme/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|err| format!("Could not start the request: {err}"))?;
    let mut request = client.request(method, url.trim());
    for line in headers.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or_else(|| format!("The header \u{201c}{line}\u{201d} needs a colon, like Accept: text/plain."))?;
        request = request.header(name.trim(), value.trim());
    }
    if !body.is_empty() {
        request = request.body(body.to_owned());
    }
    let response = request.send().await.map_err(|err| format!("The request failed: {}", plain_error(&err)))?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.as_str().to_owned(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
        .collect();
    let bytes = response.bytes().await.map_err(|err| format!("The answer could not be read: {}", plain_error(&err)))?;
    let mut end = bytes.len().min(OUTPUT_LIMIT);
    let text = loop {
        match std::str::from_utf8(&bytes[..end]) {
            Ok(text) => break text.to_owned(),
            // Cut inside a character: step back to its start.
            Err(err) if err.error_len().is_none() => end = err.valid_up_to(),
            Err(_) => break String::from_utf8_lossy(&bytes[..end]).into_owned(),
        }
    };
    Ok(HttpReply { status, headers, body: text })
}

/// The error and its causes, which reqwest keeps apart.
fn plain_error(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut cause = err.source();
    while let Some(next) = cause {
        text.push_str(": ");
        text.push_str(&next.to_string());
        cause = next.source();
    }
    text
}

/// A server's certificate, as a TLS check found it.
#[derive(Clone, Debug, PartialEq)]
pub struct Certificate {
    /// Seconds since 1970 when it stops being valid.
    pub expires: i64,
    pub subject: Option<String>,
    pub issuer: Option<String>,
    /// Why this computer does not trust it, when it doesn't.
    pub untrusted: Option<String>,
}

impl Certificate {
    pub fn days_left(&self, now: i64) -> i64 {
        (self.expires - now).div_euclid(86_400)
    }

    pub fn to_json(&self, now: i64) -> Value {
        let expires = chrono::DateTime::from_timestamp(self.expires, 0).map(|at| at.format("%Y-%m-%d").to_string()).unwrap_or_default();
        json!({
            "days_left": self.days_left(now),
            "expires": expires,
            "subject": self.subject,
            "issuer": self.issuer,
            "trusted": self.untrusted.is_none(),
        })
    }
}

/// Takes the server's certificates and this computer's verdict on them,
/// and lets the handshake end either way: the check only reads them, and
/// nothing is sent over the connection.
#[derive(Debug)]
struct Recorder {
    inner: Arc<dyn ServerCertVerifier>,
    seen: Mutex<Option<(CertificateDer<'static>, Option<String>)>>,
}

impl ServerCertVerifier for Recorder {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let verdict = self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now).err().map(|err| err.to_string());
        if let Ok(mut seen) = self.seen.lock() {
            *seen = Some((end_entity.clone().into_owned(), verdict));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// Reads the certificate `host` shows on `port`.
pub async fn tls(host: &str, port: u16, timeout: Duration) -> Result<Certificate, String> {
    let host = host.trim().trim_start_matches("https://").split('/').next().unwrap_or_default().to_owned();
    if host.is_empty() {
        return Err("Give a host name, like example.com.".into());
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let set_up = |err: rustls::Error| format!("Could not set up TLS: {err}");
    let inner = Arc::new(rustls_platform_verifier::Verifier::new(provider.clone()).map_err(set_up)?);
    let recorder = Arc::new(Recorder { inner, seen: Mutex::new(None) });
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(set_up)?
        .dangerous()
        .with_custom_certificate_verifier(recorder.clone())
        .with_no_client_auth();
    let name = ServerName::try_from(host.clone()).map_err(|_| format!("\u{201c}{host}\u{201d} is not a host name."))?;
    let connect = async {
        let stream = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|err| format!("Could not reach {host}:{port}: {err}"))?;
        tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, stream).await.map_err(|err| format!("The TLS handshake with {host} failed: {err}"))
    };
    let stream = tokio::time::timeout(timeout, connect).await.map_err(|_| format!("{host}:{port} did not answer in time."))??;
    drop(stream);
    let (der, untrusted) = recorder.seen.lock().ok().and_then(|mut seen| seen.take()).ok_or("The server showed no certificate.")?;
    let mut certificate = read_certificate(&der).ok_or("The certificate could not be read.")?;
    certificate.untrusted = untrusted;
    Ok(certificate)
}

/// One DER item: its tag, its content and what follows it.
fn item(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = data.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (length, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || rest.len() < count {
            return None;
        }
        (rest[..count].iter().fold(0usize, |n, &b| (n << 8) | usize::from(b)), &rest[count..])
    };
    (rest.len() >= length).then(|| (tag, &rest[..length], &rest[length..]))
}

/// The expiry and names of a certificate, from its DER bytes.
fn read_certificate(der: &[u8]) -> Option<Certificate> {
    let (_, certificate, _) = item(der)?;
    let (_, tbs, _) = item(certificate)?;
    let mut rest = tbs;
    // The version comes first when it is there, tagged [0].
    let (tag, _, after) = item(rest)?;
    if tag == 0xa0 {
        rest = after;
    }
    let (_, _serial, rest) = item(rest)?;
    let (_, _algorithm, rest) = item(rest)?;
    let (_, issuer, rest) = item(rest)?;
    let (_, validity, rest) = item(rest)?;
    let (_, subject, _) = item(rest)?;
    let (_, _not_before, rest) = item(validity)?;
    let (tag, not_after, _) = item(rest)?;
    Some(Certificate { expires: read_time(tag, not_after)?, subject: common_name(subject), issuer: common_name(issuer), untrusted: None })
}

/// A UTCTime (YYMMDDHHMMSSZ) or GeneralizedTime (YYYYMMDDHHMMSSZ).
fn read_time(tag: u8, text: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(text).ok()?.trim_end_matches('Z');
    let full = match tag {
        0x17 => {
            let year: i32 = text.get(..2)?.parse().ok()?;
            format!("{}{text}", if year >= 50 { "19" } else { "20" })
        }
        0x18 => text.to_owned(),
        _ => return None,
    };
    chrono::NaiveDateTime::parse_from_str(full.get(..14)?, "%Y%m%d%H%M%S").ok().map(|at| at.and_utc().timestamp())
}

/// The CN of a name, like `example.com` or `R3`.
fn common_name(name: &[u8]) -> Option<String> {
    let mut sets = name;
    while let Some((_, set, rest)) = item(sets) {
        sets = rest;
        let (_, pair, _) = item(set)?;
        let (_, oid, value) = item(pair)?;
        if oid == [0x55, 0x04, 0x03] {
            let (_, text, _) = item(value)?;
            return Some(String::from_utf8_lossy(text).into_owned());
        }
    }
    None
}

/// The record types a DNS step can ask for.
pub const RECORDS: &[&str] = &["A", "AAAA", "CNAME", "MX", "TXT", "NS"];

/// Looks `name` up with this computer's DNS servers.
pub async fn dns(name: &str, record: &str, timeout: Duration) -> Result<Vec<String>, String> {
    use hickory_resolver::proto::rr::RecordType;
    let name = name.trim().trim_end_matches('.');
    if name.is_empty() {
        return Err("Give a name, like example.com.".into());
    }
    let kind = RecordType::from_str(&record.trim().to_uppercase()).map_err(|_| format!("\u{201c}{record}\u{201d} is not a record type."))?;
    let mut builder = hickory_resolver::TokioResolver::builder_tokio().map_err(|err| format!("Could not read this computer's DNS settings: {err}"))?;
    builder.options_mut().timeout = timeout.min(Duration::from_secs(10));
    let resolver = builder.build();
    let found = tokio::time::timeout(timeout, resolver.lookup(format!("{name}."), kind)).await.map_err(|_| format!("Looking up {name} took too long."))?;
    match found {
        Ok(lookup) => Ok(lookup.record_iter().filter(|r| r.record_type() == kind).map(|r| r.data().to_string()).collect()),
        Err(err) if err.is_no_records_found() => Ok(Vec::new()),
        Err(err) => Err(format!("Could not look up {name}: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A DER item, for building a certificate's bytes by hand.
    fn der(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        if content.len() < 0x80 {
            out.push(content.len() as u8);
        } else {
            out.extend([0x82, (content.len() >> 8) as u8, content.len() as u8]);
        }
        out.extend_from_slice(content);
        out
    }

    fn name(cn: &str) -> Vec<u8> {
        let pair = [der(0x06, &[0x55, 0x04, 0x03]), der(0x0c, cn.as_bytes())].concat();
        der(0x30, &der(0x31, &der(0x30, &pair)))
    }

    #[test]
    fn a_certificate_tells_when_it_expires() {
        let validity = der(0x30, &[der(0x17, b"250101000000Z"), der(0x17, b"350101000000Z")].concat());
        let tbs = der(
            0x30,
            &[der(0xa0, &der(0x02, &[2])), der(0x02, &[1]), der(0x30, &der(0x06, &[0x2a])), name("R3"), validity, name("test.example")].concat(),
        );
        let certificate = der(0x30, &[tbs, der(0x30, &der(0x06, &[0x2a])), der(0x03, &[0])].concat());
        let read = read_certificate(&certificate).unwrap();
        assert_eq!(read.subject.as_deref(), Some("test.example"));
        assert_eq!(read.issuer.as_deref(), Some("R3"));
        let expires = chrono::NaiveDate::from_ymd_opt(2035, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
        assert_eq!(read.expires, expires);
        assert_eq!(read.days_left(expires - 3 * 86_400 - 5), 3);
    }

    #[test]
    fn short_and_long_times_read() {
        assert_eq!(read_time(0x17, b"491231235959Z"), Some(2524607999));
        assert_eq!(read_time(0x18, b"20500101000000Z"), Some(2524608000));
        assert_eq!(read_time(0x13, b"x"), None);
    }
}
