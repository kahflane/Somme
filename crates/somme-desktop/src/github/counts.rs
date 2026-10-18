//! How many pull requests and issues are open in each repository, for the
//! badges on the title bar, and where each repository is now.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui_kit::{App, Global};

use super::api::{ApiError, GitHub, Summary};
use super::repo::Repo;
use crate::runtime;
use crate::services::Services;

/// How old the numbers may get before Somme asks GitHub again.
const REFRESH: Duration = Duration::from_secs(120);

#[derive(Default)]
pub struct Counts {
    /// None until there is a token, and after GitHub refused it.
    client: Option<GitHub>,
    /// The keychain is asked for the token once, as each ask can show a prompt.
    looked: bool,
    /// What GitHub last said about each repository (None when it couldn't
    /// say), and when.
    known: HashMap<Repo, (Option<Summary>, Instant)>,
    asking: HashSet<Repo>,
}

impl Global for Counts {}

impl Counts {
    pub fn summary(repo: &Repo, cx: &App) -> Option<Summary> {
        cx.try_global::<Self>()?.known.get(repo)?.0.clone()
    }

    /// Where `repo` is on GitHub now, when Somme knows.
    pub fn real(repo: &Repo, cx: &App) -> Option<Repo> {
        Self::summary(repo, cx).map(|summary| summary.repo)
    }

    /// Asks GitHub about `repo` when Somme knows nothing yet, what it knows
    /// is old, or the user asked.
    pub fn refresh(repo: &Repo, services: &Services, now: bool, cx: &mut App) {
        if let Some(counts) = cx.try_global::<Self>() {
            let fresh = !now && counts.known.get(repo).is_some_and(|(_, at)| at.elapsed() < REFRESH);
            if fresh || counts.asking.contains(repo) || (counts.looked && counts.client.is_none()) {
                return;
            }
        }
        let counts = cx.default_global::<Self>();
        if !counts.looked {
            counts.looked = true;
            counts.client = services.sync.github_token().and_then(|token| GitHub::new(token).ok());
        }
        let Some(client) = counts.client.clone() else { return };
        counts.asking.insert(repo.clone());
        let repo = repo.clone();
        cx.spawn(async move |cx| {
            let asked = repo.clone();
            let result = runtime::io(async move { client.summary(&asked).await }).await;
            cx.update(|cx| {
                let counts = cx.default_global::<Self>();
                counts.asking.remove(&repo);
                let summary = match result {
                    Ok(summary) => Some(summary),
                    Err(err) => {
                        if matches!(err, ApiError::Unauthorized) {
                            counts.client = None;
                        }
                        tracing::debug!(%err, repo = repo.full_name(), "no GitHub numbers");
                        // Keep showing the last numbers; ask again later.
                        counts.known.remove(&repo).and_then(|(summary, _)| summary)
                    }
                };
                counts.known.insert(repo, (summary, Instant::now()));
            });
        })
        .detach();
    }

    /// Keeps what the GitHub panel learned.
    pub fn learned(repo: Repo, summary: Summary, cx: &mut App) {
        cx.default_global::<Self>().known.insert(repo, (Some(summary), Instant::now()));
    }

    /// Asks GitHub with this client from now on, after the user connected.
    pub fn use_client(client: GitHub, cx: &mut App) {
        let counts = cx.default_global::<Self>();
        counts.client = Some(client);
        counts.looked = true;
        counts.known.clear();
    }

    /// Stops asking, after GitHub refused the token.
    pub fn forget_client(cx: &mut App) {
        let counts = cx.default_global::<Self>();
        counts.client = None;
        counts.looked = true;
    }
}
