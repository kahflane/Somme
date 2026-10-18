//! Fetches the web images that Markdown files and GitHub profiles show. Any
//! document can name an image, so only secure addresses on the open
//! internet are fetched, never this computer or the local network, and only
//! so much of each.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use gpui_kit::http_client::http::HeaderValue;
use gpui_kit::http_client::{AsyncBody, HttpClient, Request, Response, Url};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::runtime;

const AGENT: &str = concat!("somme/", env!("CARGO_PKG_VERSION"));
const LIMIT: usize = 20 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
const REDIRECTS: usize = 5;

pub struct WebImages {
    client: reqwest::Client,
    agent: HeaderValue,
}

impl WebImages {
    pub fn new() -> anyhow::Result<Arc<Self>> {
        let client = reqwest::Client::builder()
            .user_agent(AGENT)
            .timeout(TIMEOUT)
            .dns_resolver(Arc::new(PublicOnly))
            // A proxy would look the name up itself, past `PublicOnly`.
            .no_proxy()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= REDIRECTS {
                    attempt.error("too many redirects")
                } else if !allowed(attempt.url()) {
                    attempt.error("redirected to an address Somme doesn't load images from")
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        Ok(Arc::new(Self {
            client,
            agent: HeaderValue::from_static(AGENT),
        }))
    }
}

/// Secure, and not named as this computer or the local network. Names that
/// lead there are stopped by [`PublicOnly`].
fn allowed(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str().filter(|_| url.scheme() == "https") else { return false };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    match host.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>() {
        Ok(ip) => public(ip),
        Err(_) => !(host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local")),
    }
}

/// Whether `ip` is on the open internet.
fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            // 100.64.0.0/10 is shared by carriers, like a local network.
            let shared = a == 100 && (b & 0xc0) == 64;
            // 0.0.0.0/8 can mean this computer, 192.0.0.0/24 is for protocols,
            // 198.18.0.0/15 for test networks, and 240.0.0.0/4 is not given out.
            let reserved = a == 0 || (a, b, c) == (192, 0, 0) || (a == 198 && (b & 0xfe) == 18) || a >= 240;
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_documentation()
                || shared
                || reserved)
        }
        IpAddr::V6(ip) => {
            let parts = ip.segments();
            let v4 = |high: u16, low: u16| {
                let [a, b] = high.to_be_bytes();
                let [c, d] = low.to_be_bytes();
                IpAddr::V4(Ipv4Addr::new(a, b, c, d))
            };
            match parts {
                // These carry an IPv4 address and lead where it does: mapped,
                // the old compatible form (with :: and ::1), NAT64, and 6to4.
                [0, 0, 0, 0, 0, 0xffff, high, low]
                | [0, 0, 0, 0, 0, 0, high, low]
                | [0x64, 0xff9b, 0, 0, 0, 0, high, low]
                | [0x2002, high, low, ..] => public(v4(high, low)),
                // fc00::/7 is local to a site and fe80::/10 to a link. 2001::/32
                // (Teredo) and 64:ff9b:1::/48 hide where they lead, and
                // 2001:db8::/32 is for examples.
                [first, second, third, ..] => {
                    !(ip.is_multicast()
                        || (first & 0xfe00) == 0xfc00
                        || (first & 0xffc0) == 0xfe80
                        || (first, second) == (0x2001, 0)
                        || (first, second) == (0x2001, 0xdb8)
                        || (first, second, third) == (0x64, 0xff9b, 1))
                }
            }
        }
    }
}

/// Looks names up, keeping only addresses on the open internet, so a name
/// that leads to the local network gets nothing.
struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let found = tokio::net::lookup_host((name.as_str(), 0)).await?;
            let public: Vec<SocketAddr> = found.filter(|addr| public(addr.ip())).collect();
            if public.is_empty() {
                return Err(format!("{} is not on the open internet", name.as_str()).into());
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

/// Many SVG cards on GitHub profiles start blank, then fade or type their
/// content in. Drawn once they would stay blank, so they are shown as they
/// end up: faded-in parts shown, and the first typed line typed out.
fn settle(svg: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(svg).ok()?;
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for node in doc.descendants().filter(roxmltree::Node::is_element) {
        if let Some(style) = node.attribute_node("style")
            && style.value().contains("animation")
        {
            let range = style.range_value();
            edits.push((range.clone(), shown(&svg[range])));
        }
        match node.tag_name().name() {
            "style" => {
                for text in node.children().filter(roxmltree::Node::is_text) {
                    let range = text.range();
                    edits.push((range.clone(), settle_css(&svg[range])));
                }
            }
            "animate" => edits.extend(settle_animate(node, svg)),
            _ => {}
        }
    }
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut out = svg.to_owned();
    for (range, text) in edits {
        out.replace_range(range, &text);
    }
    Some(out)
}

/// CSS declarations with a hidden start made visible.
fn shown(declarations: &str) -> String {
    let hidden = |part: &str| {
        part.split_once(':').is_some_and(|(name, value)| {
            name.trim() == "opacity" && value.trim().parse::<f32>().is_ok_and(|value| value == 0.)
        })
    };
    declarations.split(';').map(|part| if hidden(part) { " opacity: 1" } else { part }).collect::<Vec<_>>().join(";")
}

/// A style sheet whose animated rules start visible. Keyframes, which hold no
/// `animation` themselves, stay as they are.
fn settle_css(css: &str) -> String {
    let mut out = css.to_owned();
    let mut opens = Vec::new();
    let mut blocks = Vec::new();
    for (at, ch) in css.char_indices() {
        match ch {
            '{' => opens.push(at),
            '}' => {
                if let Some(start) = opens.pop() {
                    blocks.push(start + 1..at);
                }
            }
            _ => {}
        }
    }
    blocks.retain(|block| !css[block.clone()].contains('{') && css[block.clone()].contains("animation"));
    blocks.sort_by_key(|block| std::cmp::Reverse(block.start));
    for block in blocks {
        let body = shown(&css[block.clone()]);
        out.replace_range(block, &body);
    }
    out
}

/// Gives the element an `<animate>` changes the value it has once the
/// animation has run, when the element has none of its own. Only animations
/// that start straight away: later ones take turns in the same place.
fn settle_animate(animate: roxmltree::Node, svg: &str) -> Option<(Range<usize>, String)> {
    let name = animate.attribute("attributeName")?;
    let target = animate.parent_element()?;
    let starts = animate.attribute("begin").is_none_or(|begin| begin.split(';').any(|at| matches!(at.trim(), "0" | "0s")));
    if !starts || target.has_attribute(name) || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let values: Vec<&str> = animate.attribute("values")?.split(';').map(str::trim).collect();
    // Typing cards go: nothing typed; all typed; …
    let value = values.get(1).or(values.first())?;
    let tag = &svg[target.range()];
    let at = target.range().start + tag.find(|c: char| c.is_whitespace() || c == '>' || c == '/')?;
    let value = value.replace('&', "&amp;").replace('\'', "&apos;").replace('<', "&lt;");
    Some((at..at, format!(" {name}='{value}'")))
}

impl HttpClient for WebImages {
    fn user_agent(&self) -> Option<&HeaderValue> {
        Some(&self.agent)
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(&self, request: Request<AsyncBody>) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let client = self.client.clone();
        let url = request.uri().to_string();
        Box::pin(async move {
            let url = reqwest::Url::parse(&url)?;
            anyhow::ensure!(allowed(&url), "Somme only loads images from secure addresses on the internet");
            tracing::debug!(%url, "fetching an image");
            let (status, body) = runtime::io(async move {
                let mut response = client.get(url).send().await?;
                let status = response.status().as_u16();
                anyhow::ensure!(response.content_length().is_none_or(|length| length <= LIMIT as u64), "the image is too big");
                let mut body = Vec::new();
                while let Some(chunk) = response.chunk().await? {
                    body.extend_from_slice(&chunk);
                    anyhow::ensure!(body.len() <= LIMIT, "the image is too big");
                }
                let svg = String::from_utf8_lossy(&body[..body.len().min(1024)]).contains("<svg");
                let settled = svg.then(|| std::str::from_utf8(&body).ok().and_then(settle)).flatten();
                Ok((status, settled.map_or(body, String::into_bytes)))
            })
            .await
            .inspect_err(|err| tracing::debug!(%err, "image not fetched"))?;
            tracing::debug!(status, bytes = body.len(), "image fetched");
            Ok(Response::builder().status(status).body(AsyncBody::from(body))?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animated_cards_show_how_they_end() {
        let typing = "<svg xmlns='http://www.w3.org/2000/svg'><path id='p0'><animate attributeName='d' begin='0s;d1.end' values='m0,25 h0 ; m0,25 h650 ; m0,25 h0'/></path><path id='p1'><animate attributeName='d' begin='d0.end' values='m0,25 h0 ; m0,25 h650'/></path></svg>";
        let settled = settle(typing).unwrap();
        assert!(settled.contains("<path d='m0,25 h650' id='p0'>"), "{settled}");
        assert!(settled.contains("<path id='p1'>"), "{settled}");

        let fading = "<svg xmlns='http://www.w3.org/2000/svg'><style>.s { opacity: 0; animation: f 1s forwards; } @keyframes f { 0% { opacity: 0; } 100% { opacity: 1; } } .h { opacity: 0 }</style><text style='opacity: 0; animation: f 0.5s linear forwards'>17</text></svg>";
        let settled = settle(fading).unwrap();
        assert!(settled.contains(".s { opacity: 1; animation: f 1s forwards; }"), "{settled}");
        assert!(settled.contains("0% { opacity: 0; }"), "{settled}");
        assert!(settled.contains(".h { opacity: 0 }"), "{settled}");
        assert!(settled.contains("style=' opacity: 1; animation: f 0.5s linear forwards'"), "{settled}");
        assert_eq!(settle("<svg xmlns='http://www.w3.org/2000/svg'><rect/></svg>"), None);
    }

    #[test]
    fn only_secure_public_addresses() {
        let ok = |url: &str| allowed(&reqwest::Url::parse(url).unwrap());
        assert!(ok("https://img.shields.io/badge/x.svg"));
        assert!(ok("https://8.8.8.8/a.png"));
        assert!(ok("https://[2606:4700:4700::1111]/a.png"));
        assert!(ok("https://[64:ff9b::808:808]/a.png"), "NAT64 to a public address");
        for url in [
            "http://img.shields.io/a.svg",
            "file:///etc/hosts",
            "https://localhost/a.png",
            "https://printer.local/a.png",
            "https://127.0.0.1/a.png",
            "https://192.168.1.1/a.png",
            "https://10.0.0.8/a.png",
            "https://169.254.169.254/latest",
            "https://100.64.1.1/a.png",
            "https://[::1]/a.png",
            "https://[fd00::1]/a.png",
            "https://[::ffff:192.168.1.1]/a.png",
            "https://0.0.0.0/a.png",
            "https://0.1.2.3/a.png",
            "https://198.18.0.1/a.png",
            "https://224.0.0.1/a.png",
            "https://250.1.1.1/a.png",
            "https://[::]/a.png",
            "https://[::7f00:1]/a.png",
            "https://[64:ff9b::a00:1]/a.png",
            "https://[2002:c0a8:101::1]/a.png",
            "https://[2001:0:4136:e378::1]/a.png",
            "https://[ff02::1]/a.png",
        ] {
            assert!(!ok(url), "{url}");
        }
    }
}
