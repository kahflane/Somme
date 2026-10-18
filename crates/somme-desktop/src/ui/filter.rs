use somme_application::dto::HostView;

/// Scores how well `query` matches `text`: all query characters must appear
/// in order. Higher is better; `None` means no match.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut qi = 0;
    let mut last_match: Option<usize> = None;
    for (ti, ch) in text.iter().enumerate() {
        if qi < query.len() && *ch == query[qi] {
            score += 10;
            match last_match {
                Some(last) if last + 1 == ti => score += 15,
                // Letters far apart match worse: "search files" is "Search Files…", not "Search Text in Files…".
                Some(last) => score -= (ti - last - 1).min(10) as i32,
                None => {}
            }
            if ti == 0 || !text[ti - 1].is_alphanumeric() {
                score += 20;
            }
            last_match = Some(ti);
            qi += 1;
        }
    }
    if qi < query.len() {
        return None;
    }
    // Prefer shorter texts when scores tie.
    Some(score * 100 - text.len() as i32)
}

/// What the host search box looks at.
pub fn host_haystack(host: &HostView) -> String {
    let mut text = format!("{} {} {}", host.label, host.address, host.username.as_deref().unwrap_or(""));
    for tag in &host.tags {
        text.push(' ');
        text.push_str(tag);
    }
    text
}

/// Hosts matching a search box query. Each word must match somewhere.
pub fn host_matches(host: &HostView, query: &str) -> bool {
    let haystack = host_haystack(host).to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

/// Parses quick-connect text like `user@host:2222` or `ssh -p 2222 user@host`.
pub fn parse_quick_connect(text: &str) -> Option<(Option<String>, String, u16)> {
    let mut words: Vec<&str> = text.split_whitespace().collect();
    if words.first() == Some(&"ssh") {
        words.remove(0);
    }
    let mut port = None;
    let mut user = None;
    let mut target = None;
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "-p" => {
                port = words.get(i + 1).and_then(|p| p.parse().ok());
                i += 2;
                continue;
            }
            "-l" => {
                user = words.get(i + 1).map(|u| (*u).to_owned());
                i += 2;
                continue;
            }
            word if word.starts_with('-') => {}
            word => target = Some(word),
        }
        i += 1;
    }
    let target = target?;
    let (target_user, rest) = match target.rsplit_once('@') {
        Some((u, rest)) => (Some(u.to_owned()), rest),
        None => (None, target),
    };
    let (host, inline_port) = match rest.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (h, p.parse().ok()),
        _ => (rest, None),
    };
    let host = host.trim_matches(['[', ']']);
    if host.is_empty() {
        return None;
    }
    Some((
        target_user.or(user),
        host.to_owned(),
        inline_port.or(port).unwrap_or(22),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_prefers_word_starts() {
        assert!(fuzzy_score("ws", "web server") > fuzzy_score("ws", "towns"));
        assert!(fuzzy_score("xyz", "web").is_none());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn parses_quick_connect() {
        assert_eq!(
            parse_quick_connect("root@10.0.0.5:2222"),
            Some((Some("root".into()), "10.0.0.5".into(), 2222))
        );
        assert_eq!(
            parse_quick_connect("ssh -p 2200 deploy@example.com"),
            Some((Some("deploy".into()), "example.com".into(), 2200))
        );
        assert_eq!(parse_quick_connect("example.com"), Some((None, "example.com".into(), 22)));
        assert_eq!(parse_quick_connect("ssh"), None);
    }
}
