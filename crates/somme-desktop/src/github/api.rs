//! GitHub's API, for a repository's pull requests and issues. Lists come from
//! GraphQL search, which has what a list row shows in one request; the rest
//! uses the REST API. Every call is async and runs on [`crate::runtime::io`].

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use reqwest::{Method, StatusCode, header};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use super::repo::Repo;

const API: &str = "https://api.github.com";
const TIMEOUT: Duration = Duration::from_secs(30);
/// Rows asked for at a time.
const PAGE: usize = 30;
/// Pages of timeline read at most, at 100 events each.
const TIMELINE_PAGES: usize = 5;
/// Bigger diffs are not shown; GitHub's own page gives up around here too.
const MAX_DIFF: usize = 8 * 1024 * 1024;
const MAX_AVATAR: usize = 1024 * 1024;
const AVATAR_HOST: &str = "avatars.githubusercontent.com";

#[derive(Debug, Clone, thiserror::Error)]
pub enum ApiError {
    /// The token is gone or was revoked.
    #[error("GitHub no longer accepts Somme's access. Connect again.")]
    Unauthorized,
    #[error("{0}")]
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pulls,
    Issues,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Open,
    Closed,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct User {
    #[serde(default)]
    pub login: String,
    #[serde(default, alias = "avatarUrl")]
    pub avatar_url: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Label {
    pub name: String,
    /// Hex without `#`.
    #[serde(default)]
    pub color: String,
}

/// A row of the pull request or issue list.
#[derive(Clone, Debug)]
pub struct Item {
    pub number: u64,
    pub title: String,
    /// `OPEN`, `CLOSED` or `MERGED`.
    pub state: String,
    /// For a closed issue: `COMPLETED` or `NOT_PLANNED`.
    pub state_reason: Option<String>,
    pub draft: bool,
    pub author: User,
    pub created_at: String,
    /// When it was closed, or merged for a merged pull request.
    pub closed_at: Option<String>,
    pub labels: Vec<Label>,
    pub comments: u64,
    /// `APPROVED`, `CHANGES_REQUESTED` or `REVIEW_REQUIRED`.
    pub review: Option<String>,
    /// `SUCCESS`, `FAILURE`, `ERROR`, `PENDING` or `EXPECTED`.
    pub checks: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub items: Vec<Item>,
    pub open: u64,
    pub closed: u64,
    /// Where the next page starts, when there is one.
    pub next: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Milestone {
    pub title: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Branch {
    #[serde(rename = "ref")]
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub sha: String,
    #[serde(default)]
    pub repo: Option<BranchRepo>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct BranchRepo {
    pub full_name: String,
}

/// A pull request or an issue, as its page shows it.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Detail {
    #[serde(default)]
    pub node_id: String,
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    /// `open` or `closed`.
    pub state: String,
    #[serde(default)]
    pub state_reason: Option<String>,
    #[serde(default)]
    pub user: User,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub assignees: Vec<User>,
    #[serde(default)]
    pub milestone: Option<Milestone>,
    #[serde(default)]
    pub comments: u64,
    pub created_at: String,
    #[serde(default)]
    pub locked: bool,
    // Pull requests only.
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub merged_by: Option<User>,
    #[serde(default)]
    pub mergeable: Option<bool>,
    /// `clean`, `blocked`, `behind`, `dirty`, `unstable`, `draft` or `unknown`.
    #[serde(default)]
    pub mergeable_state: Option<String>,
    #[serde(default)]
    pub requested_reviewers: Vec<User>,
    #[serde(default)]
    pub head: Option<Branch>,
    #[serde(default)]
    pub base: Option<Branch>,
    #[serde(default)]
    pub commits: u64,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    #[serde(default)]
    pub changed_files: u64,
}

impl Detail {
    pub fn is_pull(&self) -> bool {
        self.head.is_some()
    }
}

/// What the signed-in user may do in the repository, and how it merges.
#[derive(Clone, Debug, Default)]
pub struct RepoInfo {
    pub can_push: bool,
    pub merge_commit: bool,
    pub squash: bool,
    pub rebase: bool,
    pub delete_branch_on_merge: bool,
}

/// One entry of a conversation.
#[derive(Clone, Debug)]
pub enum Event {
    Comment {
        id: u64,
        author: User,
        body: String,
        at: String,
    },
    Review {
        id: u64,
        author: User,
        /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED` or `DISMISSED`.
        state: String,
        body: String,
        at: String,
        /// Comments on lines of the diff, with their file.
        comments: Vec<LineComment>,
    },
    Commit {
        sha: String,
        message: String,
        author: String,
        at: String,
    },
    /// Anything else, told in a line like "added the bug label".
    Note {
        /// GitHub's name for what happened, like `labeled`.
        kind: String,
        actor: User,
        text: String,
        at: String,
        /// For labels: the label, drawn as a chip after the text.
        label: Option<Label>,
    },
}

#[derive(Clone, Debug, Deserialize)]
pub struct LineComment {
    #[serde(default)]
    pub pull_request_review_id: Option<u64>,
    pub path: String,
    #[serde(default)]
    pub line: Option<u64>,
    pub body: String,
    #[serde(default)]
    pub user: User,
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub avatar: Option<String>,
    pub at: String,
}

#[derive(Clone, Debug)]
pub struct Check {
    pub name: String,
    /// `success`, `failure`, `pending`, `skipped`, `neutral` or `cancelled`.
    pub result: String,
    pub detail: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub fn label(self) -> &'static str {
        match self {
            MergeMethod::Merge => "Create a merge commit",
            MergeMethod::Squash => "Squash and merge",
            MergeMethod::Rebase => "Rebase and merge",
        }
    }

    fn api(self) -> &'static str {
        match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}

impl ReviewEvent {
    fn api(self) -> &'static str {
        match self {
            ReviewEvent::Comment => "COMMENT",
            ReviewEvent::Approve => "APPROVE",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
        }
    }
}

#[derive(Clone)]
pub struct GitHub {
    http: reqwest::Client,
    token: Arc<Zeroizing<String>>,
}

/// Where GitHub's API is. UI check builds may point it at a fake on this computer.
fn api_base() -> String {
    #[cfg(feature = "snapshot")]
    if let Some(fake) = fake_api() {
        return fake;
    }
    API.to_owned()
}

/// `SOMME_GITHUB_API`, when it is a server on this computer. UI checks only.
#[cfg(feature = "snapshot")]
pub fn fake_api() -> Option<String> {
    std::env::var("SOMME_GITHUB_API").ok().filter(|url| url.starts_with("http://127.0.0.1:"))
}

fn failed(err: impl std::fmt::Display) -> ApiError {
    ApiError::Failed(err.to_string())
}

impl GitHub {
    pub fn new(token: Zeroizing<String>) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("somme/", env!("CARGO_PKG_VERSION")))
            .timeout(TIMEOUT)
            .build()
            .map_err(failed)?;
        Ok(Self {
            http,
            token: Arc::new(token),
        })
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.request_as(method, path, "application/vnd.github+json")
    }

    /// A request for `accept`. Only one Accept header goes out, or GitHub may pick JSON.
    fn request_as(&self, method: Method, path: &str, accept: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", api_base()))
            .bearer_auth(self.token.as_str())
            .header(header::ACCEPT, accept)
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    /// Sends the request and turns GitHub's errors into plain words.
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response, ApiError> {
        let response = request.send().await.map_err(|err| ApiError::Failed(format!("Can't reach GitHub: {err}")))?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized);
        }
        let sso = response.headers().contains_key("x-github-sso");
        let message = response
            .json::<Value>()
            .await
            .ok()
            .and_then(|body| body["message"].as_str().map(str::to_owned))
            .unwrap_or_else(|| status.to_string());
        Err(ApiError::Failed(if sso {
            "This organization needs you to allow Somme for single sign-on on github.com first.".to_owned()
        } else if status == StatusCode::NOT_FOUND {
            "GitHub can't find it, or Somme was not given access to this repository.".to_owned()
        } else {
            message
        }))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        self.send(self.request(Method::GET, path)).await?.json().await.map_err(failed)
    }

    async fn write(&self, method: Method, path: &str, body: Value) -> Result<Value, ApiError> {
        let response = self.send(self.request(method, path).json(&body)).await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        Ok(response.json().await.unwrap_or(Value::Null))
    }

    async fn graphql(&self, query: &str, variables: Value) -> Result<Value, ApiError> {
        let body: Value = self
            .send(self.request(Method::POST, "/graphql").json(&json!({ "query": query, "variables": variables })))
            .await?
            .json()
            .await
            .map_err(failed)?;
        if let Some(message) = body["errors"][0]["message"].as_str() {
            return Err(ApiError::Failed(message.to_owned()));
        }
        Ok(body["data"].clone())
    }

    /// A page of pull requests or issues, newest first, and how many are
    /// open and closed. `query` is GitHub search text, like `label:bug`.
    pub async fn list(
        &self,
        repo: &Repo,
        kind: Kind,
        state: State,
        query: &str,
        after: Option<String>,
    ) -> Result<Page, ApiError> {
        const QUERY: &str = r#"query($items: String!, $open: String!, $closed: String!, $after: String, $first: Int!) {
  open: search(query: $open, type: ISSUE, first: 1) { issueCount }
  closed: search(query: $closed, type: ISSUE, first: 1) { issueCount }
  items: search(query: $items, type: ISSUE, first: $first, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes {
      ... on Issue {
        number title state stateReason createdAt closedAt
        author { login avatarUrl }
        labels(first: 10) { nodes { name color } }
        comments { totalCount }
      }
      ... on PullRequest {
        number title state isDraft createdAt closedAt mergedAt reviewDecision
        author { login avatarUrl }
        labels(first: 10) { nodes { name color } }
        comments { totalCount }
        commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
      }
    }
  }
}"#;
        let base = format!(
            "repo:{} {} {}",
            repo.full_name(),
            if kind == Kind::Pulls { "is:pr" } else { "is:issue" },
            query.trim()
        );
        let base = base.trim();
        let state_word = if state == State::Open { "is:open" } else { "is:closed" };
        let data = self
            .graphql(
                QUERY,
                json!({
                    "items": format!("{base} {state_word} sort:created-desc"),
                    "open": format!("{base} is:open"),
                    "closed": format!("{base} is:closed"),
                    "after": after,
                    "first": PAGE,
                }),
            )
            .await?;
        Ok(page_from(&data))
    }

    pub async fn detail(&self, repo: &Repo, kind: Kind, number: u64) -> Result<Detail, ApiError> {
        let what = if kind == Kind::Pulls { "pulls" } else { "issues" };
        self.get(&format!("/repos/{}/{what}/{number}", repo.full_name())).await
    }

    pub async fn repo_info(&self, repo: &Repo) -> Result<RepoInfo, ApiError> {
        let body: Value = self.get(&format!("/repos/{}", repo.full_name())).await?;
        let flag = |key: &str| body[key].as_bool().unwrap_or(false);
        Ok(RepoInfo {
            can_push: body["permissions"]["push"].as_bool().unwrap_or(false),
            merge_commit: flag("allow_merge_commit"),
            squash: flag("allow_squash_merge"),
            rebase: flag("allow_rebase_merge"),
            delete_branch_on_merge: flag("delete_branch_on_merge"),
        })
    }

    /// The conversation, oldest first. For a pull request, comments on lines
    /// of the diff are put under their review.
    pub async fn timeline(&self, repo: &Repo, number: u64, pull: bool) -> Result<Vec<Event>, ApiError> {
        let mut raw = Vec::new();
        for page in 1..=TIMELINE_PAGES {
            let events: Vec<Value> = self
                .get(&format!("/repos/{}/issues/{number}/timeline?per_page=100&page={page}", repo.full_name()))
                .await?;
            let done = events.len() < 100;
            raw.extend(events);
            if done {
                break;
            }
        }
        let line_comments: Vec<LineComment> = if pull {
            self.get(&format!("/repos/{}/pulls/{number}/comments?per_page=100", repo.full_name()))
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(events_from(&raw, &line_comments))
    }

    /// The whole change as a unified diff.
    pub async fn diff(&self, repo: &Repo, number: u64) -> Result<String, ApiError> {
        let request = self.request_as(
            Method::GET,
            &format!("/repos/{}/pulls/{number}", repo.full_name()),
            "application/vnd.github.diff",
        );
        let bytes = self.send(request).await?.bytes().await.map_err(failed)?;
        if bytes.len() > MAX_DIFF {
            return Err(ApiError::Failed(format!(
                "The diff is {} MB, too big to show here. Open it on GitHub.",
                bytes.len() / 1_048_576
            )));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub async fn commits(&self, repo: &Repo, number: u64) -> Result<Vec<Commit>, ApiError> {
        let raw: Vec<Value> = self
            .get(&format!("/repos/{}/pulls/{number}/commits?per_page=100", repo.full_name()))
            .await?;
        Ok(raw
            .iter()
            .map(|commit| Commit {
                sha: text(&commit["sha"]),
                message: text(&commit["commit"]["message"]),
                author: commit["author"]["login"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| text(&commit["commit"]["author"]["name"])),
                avatar: commit["author"]["avatar_url"].as_str().map(str::to_owned),
                at: text(&commit["commit"]["author"]["date"]),
            })
            .collect())
    }

    /// Check runs and commit statuses for the commit `sha`.
    pub async fn checks(&self, repo: &Repo, sha: &str) -> Result<Vec<Check>, ApiError> {
        let runs: Value = self
            .get(&format!("/repos/{}/commits/{sha}/check-runs?per_page=100", repo.full_name()))
            .await?;
        let statuses: Value = self.get(&format!("/repos/{}/commits/{sha}/status", repo.full_name())).await?;
        let mut checks: Vec<Check> = runs["check_runs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|run| {
                let result = match (run["status"].as_str(), run["conclusion"].as_str()) {
                    (Some("completed"), Some(conclusion)) => match conclusion {
                        "success" => "success",
                        "skipped" => "skipped",
                        "neutral" => "neutral",
                        "cancelled" => "cancelled",
                        _ => "failure",
                    },
                    _ => "pending",
                };
                Check {
                    name: text(&run["name"]),
                    result: result.to_owned(),
                    detail: run["app"]["name"].as_str().map(str::to_owned),
                    url: run["html_url"].as_str().map(str::to_owned),
                }
            })
            .collect();
        checks.extend(statuses["statuses"].as_array().into_iter().flatten().map(|status| Check {
            name: text(&status["context"]),
            result: match status["state"].as_str() {
                Some("success") => "success",
                Some("pending") => "pending",
                _ => "failure",
            }
            .to_owned(),
            detail: status["description"].as_str().map(str::to_owned),
            url: status["target_url"].as_str().map(str::to_owned),
        }));
        Ok(checks)
    }

    pub async fn comment(&self, repo: &Repo, number: u64, body: &str) -> Result<(), ApiError> {
        self.write(Method::POST, &format!("/repos/{}/issues/{number}/comments", repo.full_name()), json!({ "body": body }))
            .await
            .map(drop)
    }

    /// Closes or reopens. An issue closes as done, or as not planned.
    pub async fn set_open(&self, repo: &Repo, number: u64, open: bool, not_planned: bool) -> Result<(), ApiError> {
        let mut body = json!({ "state": if open { "open" } else { "closed" } });
        if !open {
            body["state_reason"] = json!(if not_planned { "not_planned" } else { "completed" });
        }
        self.write(Method::PATCH, &format!("/repos/{}/issues/{number}", repo.full_name()), body).await.map(drop)
    }

    pub async fn rename(&self, repo: &Repo, number: u64, title: &str) -> Result<(), ApiError> {
        self.write(Method::PATCH, &format!("/repos/{}/issues/{number}", repo.full_name()), json!({ "title": title }))
            .await
            .map(drop)
    }

    pub async fn merge(&self, repo: &Repo, number: u64, method: MergeMethod, sha: &str) -> Result<(), ApiError> {
        self.write(
            Method::PUT,
            &format!("/repos/{}/pulls/{number}/merge", repo.full_name()),
            json!({ "merge_method": method.api(), "sha": sha }),
        )
        .await
        .map(drop)
    }

    /// Takes a pull request out of draft.
    pub async fn ready_for_review(&self, node_id: &str) -> Result<(), ApiError> {
        const QUERY: &str = "mutation($id: ID!) { markPullRequestReadyForReview(input: { pullRequestId: $id }) { clientMutationId } }";
        self.graphql(QUERY, json!({ "id": node_id })).await.map(drop)
    }

    pub async fn delete_branch(&self, repo: &Repo, branch: &str) -> Result<(), ApiError> {
        let request = self.request(Method::DELETE, &format!("/repos/{}/git/refs/heads/{branch}", repo.full_name()));
        self.send(request).await.map(drop)
    }

    pub async fn review(&self, repo: &Repo, number: u64, event: ReviewEvent, body: &str) -> Result<(), ApiError> {
        let mut request = json!({ "event": event.api() });
        if !body.trim().is_empty() {
            request["body"] = json!(body);
        }
        self.write(Method::POST, &format!("/repos/{}/pulls/{number}/reviews", repo.full_name()), request)
            .await
            .map(drop)
    }

    pub async fn labels(&self, repo: &Repo) -> Result<Vec<Label>, ApiError> {
        self.get(&format!("/repos/{}/labels?per_page=100", repo.full_name())).await
    }

    /// Replaces the labels on a pull request or issue.
    pub async fn set_labels(&self, repo: &Repo, number: u64, names: &[String]) -> Result<(), ApiError> {
        self.write(Method::PUT, &format!("/repos/{}/issues/{number}/labels", repo.full_name()), json!({ "labels": names }))
            .await
            .map(drop)
    }

    /// Opens a new issue and gives its number.
    pub async fn create_issue(&self, repo: &Repo, title: &str, body: &str) -> Result<u64, ApiError> {
        let created = self
            .write(Method::POST, &format!("/repos/{}/issues", repo.full_name()), json!({ "title": title, "body": body }))
            .await?;
        created["number"].as_u64().ok_or_else(|| failed("GitHub did not say which issue it opened"))
    }
}

/// A profile picture, small. Only GitHub's picture host is asked, and
/// without the token.
pub async fn avatar(url: &str) -> Result<Vec<u8>, ApiError> {
    static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent(concat!("somme/", env!("CARGO_PKG_VERSION")))
            .timeout(TIMEOUT)
            .build()
            .unwrap_or_default()
    });
    let mut url = reqwest::Url::parse(url).map_err(failed)?;
    if url.scheme() != "https" || url.host_str() != Some(AVATAR_HOST) {
        return Err(failed("not a GitHub picture"));
    }
    url.query_pairs_mut().append_pair("s", "64");
    let bytes = HTTP
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(failed)?
        .bytes()
        .await
        .map_err(failed)?;
    if bytes.len() > MAX_AVATAR {
        return Err(failed("picture too big"));
    }
    Ok(bytes.to_vec())
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

fn user(value: &Value) -> User {
    User {
        login: value["login"].as_str().unwrap_or("ghost").to_owned(),
        avatar_url: value["avatar_url"].as_str().or(value["avatarUrl"].as_str()).map(str::to_owned),
    }
}

fn label(value: &Value) -> Label {
    Label {
        name: text(&value["name"]),
        color: text(&value["color"]),
    }
}

fn page_from(data: &Value) -> Page {
    let items = data["items"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| node["number"].is_u64())
        .map(|node| Item {
            number: node["number"].as_u64().unwrap_or_default(),
            title: text(&node["title"]),
            state: text(&node["state"]),
            state_reason: node["stateReason"].as_str().map(str::to_owned),
            draft: node["isDraft"].as_bool().unwrap_or(false),
            author: user(&node["author"]),
            created_at: text(&node["createdAt"]),
            closed_at: node["mergedAt"].as_str().or(node["closedAt"].as_str()).map(str::to_owned),
            labels: node["labels"]["nodes"].as_array().into_iter().flatten().map(label).collect(),
            comments: node["comments"]["totalCount"].as_u64().unwrap_or_default(),
            review: node["reviewDecision"].as_str().map(str::to_owned),
            checks: node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"].as_str().map(str::to_owned),
        })
        .collect();
    let info = &data["items"]["pageInfo"];
    Page {
        items,
        open: data["open"]["issueCount"].as_u64().unwrap_or_default(),
        closed: data["closed"]["issueCount"].as_u64().unwrap_or_default(),
        next: info["hasNextPage"].as_bool().unwrap_or(false).then(|| text(&info["endCursor"])),
    }
}

/// Turns timeline events into conversation entries. Events nobody reads,
/// like subscriptions, are left out.
fn events_from(raw: &[Value], line_comments: &[LineComment]) -> Vec<Event> {
    let mut events = Vec::new();
    for event in raw {
        let kind = event["event"].as_str().unwrap_or_default();
        let actor = || user(if event["actor"].is_object() { &event["actor"] } else { &event["user"] });
        let at = || text(if event["created_at"].is_string() { &event["created_at"] } else { &event["submitted_at"] });
        let note = |text: String| Event::Note {
            kind: kind.to_owned(),
            actor: actor(),
            text,
            at: at(),
            label: None,
        };
        let entry = match kind {
            "commented" => Event::Comment {
                id: event["id"].as_u64().unwrap_or_default(),
                author: user(&event["user"]),
                body: text(&event["body"]),
                at: at(),
            },
            "reviewed" => {
                let id = event["id"].as_u64().unwrap_or_default();
                let comments: Vec<LineComment> =
                    line_comments.iter().filter(|c| c.pull_request_review_id == Some(id)).cloned().collect();
                let body = text(&event["body"]);
                let state = text(&event["state"]).to_uppercase();
                // A reply to a line comment is its own empty review; the reply shows with the first one.
                if state == "COMMENTED" && body.is_empty() && comments.is_empty() {
                    continue;
                }
                Event::Review {
                    id,
                    author: user(&event["user"]),
                    state,
                    body,
                    at: at(),
                    comments,
                }
            }
            "committed" => Event::Commit {
                sha: text(&event["sha"]),
                message: text(&event["message"]),
                author: text(&event["author"]["name"]),
                at: text(&event["author"]["date"]),
            },
            "labeled" | "unlabeled" => Event::Note {
                kind: kind.to_owned(),
                actor: actor(),
                text: if kind == "labeled" { "added".into() } else { "removed".into() },
                at: at(),
                label: Some(label(&event["label"])),
            },
            "assigned" => note(format!("assigned {}", text(&event["assignee"]["login"]))),
            "unassigned" => note(format!("unassigned {}", text(&event["assignee"]["login"]))),
            "closed" => note(match event["state_reason"].as_str() {
                Some("not_planned") => "closed this as not planned".into(),
                _ => "closed this".into(),
            }),
            "reopened" => note("reopened this".into()),
            "merged" => note(format!("merged commit {}", short_sha(&text(&event["commit_id"])))),
            "renamed" => note(format!(
                "changed the title from \"{}\" to \"{}\"",
                text(&event["rename"]["from"]),
                text(&event["rename"]["to"])
            )),
            "review_requested" => note(format!(
                "requested a review from {}",
                event["requested_reviewer"]["login"]
                    .as_str()
                    .or(event["requested_team"]["name"].as_str())
                    .unwrap_or("someone")
            )),
            "review_request_removed" => note("removed a review request".into()),
            "head_ref_force_pushed" => note("force-pushed the branch".into()),
            "head_ref_deleted" => note("deleted the branch".into()),
            "head_ref_restored" => note("restored the branch".into()),
            "ready_for_review" => note("marked this ready for review".into()),
            "convert_to_draft" => note("marked this as a draft".into()),
            "milestoned" => note(format!("added this to the {} milestone", text(&event["milestone"]["title"]))),
            "demilestoned" => note(format!("removed this from the {} milestone", text(&event["milestone"]["title"]))),
            "locked" => note("locked the conversation".into()),
            "unlocked" => note("unlocked the conversation".into()),
            "cross-referenced" => {
                let source = &event["source"]["issue"];
                note(format!("mentioned this in #{} {}", source["number"], text(&source["title"])))
            }
            "referenced" if event["commit_id"].is_string() => {
                note(format!("referenced this in commit {}", short_sha(&text(&event["commit_id"]))))
            }
            _ => continue,
        };
        events.push(entry);
    }
    events
}

pub fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_search_page() {
        let data = json!({
            "open": { "issueCount": 12 },
            "closed": { "issueCount": 40 },
            "items": {
                "pageInfo": { "hasNextPage": true, "endCursor": "Y3Vyc29y" },
                "nodes": [
                    {
                        "number": 7, "title": "Fix login", "state": "OPEN", "isDraft": true,
                        "createdAt": "2026-10-01T10:00:00Z", "reviewDecision": "APPROVED",
                        "author": { "login": "ann", "avatarUrl": "https://avatars.githubusercontent.com/u/1" },
                        "labels": { "nodes": [{ "name": "bug", "color": "d73a4a" }] },
                        "comments": { "totalCount": 3 },
                        "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "FAILURE" } } }] }
                    },
                    {}
                ]
            }
        });
        let page = page_from(&data);
        assert_eq!((page.open, page.closed), (12, 40));
        assert_eq!(page.next.as_deref(), Some("Y3Vyc29y"));
        assert_eq!(page.items.len(), 1, "an empty node is skipped");
        let item = &page.items[0];
        assert!(item.draft);
        assert_eq!(item.author.login, "ann");
        assert_eq!(item.labels[0].name, "bug");
        assert_eq!(item.review.as_deref(), Some("APPROVED"));
        assert_eq!(item.checks.as_deref(), Some("FAILURE"));
    }

    #[test]
    fn tells_the_conversation() {
        let raw = vec![
            json!({ "event": "commented", "id": 1, "user": { "login": "bob" }, "body": "Looks good", "created_at": "2026-10-02T00:00:00Z" }),
            json!({ "event": "labeled", "actor": { "login": "ann" }, "label": { "name": "bug", "color": "d73a4a" }, "created_at": "2026-10-02T01:00:00Z" }),
            json!({ "event": "reviewed", "id": 9, "state": "approved", "user": { "login": "cy" }, "body": "", "submitted_at": "2026-10-03T00:00:00Z" }),
            json!({ "event": "reviewed", "id": 10, "state": "commented", "user": { "login": "cy" }, "body": "", "submitted_at": "2026-10-03T00:00:00Z" }),
            json!({ "event": "subscribed", "actor": { "login": "ann" } }),
            json!({ "event": "closed", "actor": { "login": "ann" }, "state_reason": "not_planned", "created_at": "2026-10-04T00:00:00Z" }),
        ];
        let lines = vec![LineComment {
            pull_request_review_id: Some(9),
            path: "src/main.rs".into(),
            line: Some(4),
            body: "nit".into(),
            user: User::default(),
        }];
        let events = events_from(&raw, &lines);
        assert_eq!(events.len(), 4);
        assert!(matches!(&events[0], Event::Comment { body, .. } if body == "Looks good"));
        assert!(matches!(&events[1], Event::Note { label: Some(label), .. } if label.name == "bug"));
        assert!(matches!(&events[2], Event::Review { state, comments, .. } if state == "APPROVED" && comments.len() == 1));
        assert!(matches!(&events[3], Event::Note { text, .. } if text == "closed this as not planned"));
    }
}
