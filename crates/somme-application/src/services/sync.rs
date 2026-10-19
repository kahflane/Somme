use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use somme_domain::{
    Automation, Group, Host, Id, Identity, KnownHost, PortForward, Repository, Sealed, Snippet,
    SshKey, VaultRepository,
};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

use crate::dto::{AccountStatus, MergeReport, SyncStatusView};
use crate::error::{AppError, AppResult};
use crate::ports::{
    CloudAccounts, CloudError, CloudSession, EventPublisher, GatewayError, MasterKey, SecretStore, SessionStore,
    SyncGateway, SyncOutcome, VaultData,
};
use crate::services::VaultService;

pub struct SyncService {
    gateway: Arc<dyn SyncGateway>,
    cloud: Arc<dyn CloudAccounts>,
    sessions: Arc<dyn SessionStore>,
    /// The GitHub token for pull requests and issues.
    github: Arc<dyn SecretStore>,
    vault: Arc<VaultService>,
    events: Arc<dyn EventPublisher>,
    data: Arc<dyn VaultData>,
    previous_key: Mutex<Option<MasterKey>>,
    /// Stops the GitHub sign-in that is waiting on the browser.
    cancel_sign_in: Mutex<Option<oneshot::Sender<()>>>,
}

impl SyncService {
    pub fn new(
        gateway: Arc<dyn SyncGateway>,
        cloud: Arc<dyn CloudAccounts>,
        sessions: Arc<dyn SessionStore>,
        github: Arc<dyn SecretStore>,
        vault: Arc<VaultService>,
        events: Arc<dyn EventPublisher>,
        data: Arc<dyn VaultData>,
    ) -> Self {
        Self {
            gateway,
            cloud,
            sessions,
            github,
            vault,
            events,
            data,
            previous_key: Mutex::new(None),
            cancel_sign_in: Mutex::new(None),
        }
    }

    pub fn status(&self) -> SyncStatusView {
        self.gateway.state().into()
    }

    /// Who is signed in, without reading the sign-in itself when that can wait.
    pub fn account(&self) -> AccountStatus {
        let profile = self.sessions.profile();
        AccountStatus {
            signed_in: profile.is_some(),
            login: profile.as_ref().map(|p| p.login.clone()),
            name: profile.as_ref().and_then(|p| p.name.clone()),
            avatar_url: profile.and_then(|p| p.avatar_url),
        }
    }

    /// Reads the saved sign-in, which can ask the system keychain, and
    /// returns who it belongs to.
    pub fn load_session(&self) -> AccountStatus {
        let _ = self.sessions.load();
        self.account()
    }

    /// Signs in with GitHub in the browser. Starting another sign-in, or
    /// `cancel_sign_in`, stops this one.
    pub async fn sign_in_with_github(&self) -> AppResult<AccountStatus> {
        let (cancel, cancelled) = oneshot::channel();
        *self.cancel_sign_in.lock().unwrap_or_else(PoisonError::into_inner) = Some(cancel);
        let session = tokio::select! {
            session = self.cloud.sign_in_with_github() => session,
            _ = cancelled => return Err(GatewayError::Cancelled.into()),
        };
        self.cancel_sign_in.lock().unwrap_or_else(PoisonError::into_inner).take();
        self.start(session?).await
    }

    /// The saved GitHub token for repositories, if the user gave one. This
    /// can ask the system keychain.
    pub fn github_token(&self) -> Option<Zeroizing<String>> {
        self.github.load()
    }

    /// Asks GitHub in the browser to let Somme use the user's repositories,
    /// and saves the token. Needs a Somme sign-in. `cancel_sign_in` stops it.
    pub async fn connect_github(&self) -> AppResult<Zeroizing<String>> {
        let session = self.sessions.load().ok_or(CloudError::SessionExpired)?;
        let (cancel, cancelled) = oneshot::channel();
        *self.cancel_sign_in.lock().unwrap_or_else(PoisonError::into_inner) = Some(cancel);
        let access = tokio::select! {
            access = self.cloud.github_access(&session) => access,
            _ = cancelled => return Err(GatewayError::Cancelled.into()),
        };
        self.cancel_sign_in.lock().unwrap_or_else(PoisonError::into_inner).take();
        let access = access?;
        if !access.scope.split(',').any(|scope| scope.trim() == "repo") {
            return Err(CloudError::Rejected("GitHub did not give access to your repositories".into()).into());
        }
        self.github.save(&access.token)?;
        Ok(access.token)
    }

    /// Forgets the GitHub token, as when GitHub stops taking it.
    /// Where on github.com the user can let Somme see more organizations.
    pub async fn github_access_page(&self) -> AppResult<String> {
        Ok(self.cloud.github_access_page().await?)
    }

    pub fn forget_github(&self) -> AppResult<()> {
        Ok(self.github.clear()?)
    }

    pub fn cancel_sign_in(&self) {
        if let Some(cancel) = self.cancel_sign_in.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let _ = cancel.send(());
        }
    }

    /// The signed-in user's GitHub picture.
    pub async fn avatar(&self) -> AppResult<Option<Vec<u8>>> {
        let Some(url) = self.sessions.profile().and_then(|p| p.avatar_url) else {
            return Ok(None);
        };
        Ok(Some(self.cloud.avatar(&url).await?))
    }

    pub async fn logout(&self) -> AppResult<AccountStatus> {
        self.gateway.disconnect().await?;
        if let Some(session) = self.sessions.load()
            && let Err(err) = self.cloud.logout(&session).await {
                tracing::warn!(%err, "remote sign out failed");
            }
        self.sessions.clear()?;
        self.github.clear()?;
        Ok(self.account())
    }

    pub async fn sync_now(&self) -> AppResult<SyncStatusView> {
        self.gateway.sync_now().await?;
        Ok(self.status())
    }

    async fn start(&self, session: CloudSession) -> AppResult<AccountStatus> {
        self.sessions.save(&session)?;
        let credentials = match self.cloud.credentials(&session).await {
            Ok(credentials) => credentials,
            Err(err) => {
                self.sessions.clear()?;
                return Err(err.into());
            }
        };
        let key = self.vault.key();
        if self.gateway.connect(&credentials.url).await? == SyncOutcome::RemoteKept {
            *self.previous_key.lock().unwrap_or_else(PoisonError::into_inner) = key;
        }
        self.vault.revalidate().await?;
        self.events.data_changed();
        Ok(self.account())
    }

    pub async fn merge_previous(&self, password: &str) -> AppResult<MergeReport> {
        let Some(old) = self.gateway.previous_data().await? else {
            return Ok(MergeReport::default());
        };
        if !self.vault.is_unlocked() {
            return Err(AppError::VaultLocked);
        }
        let remembered = self.previous_key.lock().unwrap_or_else(PoisonError::into_inner).take();
        let key = VaultRepository::load(&*old).await?.and_then(|meta| {
            remembered
                .filter(|key| self.vault.opens(key, &meta))
                .or_else(|| self.vault.key_for(&meta, password))
        });
        let reseal = |sealed: &Sealed| key.as_ref().and_then(|key| self.vault.reseal(key, sealed).ok());
        let (old, new) = (&*old, &*self.data);
        let mut lost = 0;
        let mut added = copy_missing(old, new, |g: &Group| g.id, Some).await?;
        added += copy_missing(old, new, |k: &SshKey| k.id, |mut k: SshKey| match reseal(&k.private_key) {
            Some(private_key) => {
                k.private_key = private_key;
                k.passphrase = k.passphrase.as_ref().and_then(reseal);
                Some(k)
            }
            None => {
                lost += 1;
                None
            }
        })
        .await?;
        added += copy_missing(old, new, |i: &Identity| i.id, |mut i: Identity| {
            if let Some(password) = i.password.take() {
                i.password = reseal(&password);
                lost += usize::from(i.password.is_none());
            }
            Some(i)
        })
        .await?;
        added += copy_missing(old, new, |s: &Snippet| s.id, Some).await?;
        added += copy_missing(old, new, |a: &Automation| a.id, |mut a: Automation| match reseal(&a.document) {
            Some(document) => {
                a.document = document;
                Some(a)
            }
            None => {
                lost += 1;
                None
            }
        })
        .await?;
        added += copy_missing(old, new, |h: &Host| h.id, |mut h: Host| {
            if let Some(password) = h.password.take() {
                h.password = reseal(&password);
                lost += usize::from(h.password.is_none());
            }
            Some(h)
        })
        .await?;
        added += copy_missing(old, new, |f: &PortForward| f.id, Some).await?;
        for known in Repository::<KnownHost>::list(old).await? {
            if new.find(&known.host, known.port).await?.is_none() {
                Repository::<KnownHost>::save(new, &known).await?;
                added += 1;
            }
        }
        self.gateway.forget_previous_data().await?;
        self.events.data_changed();
        Ok(MergeReport {
            added,
            secrets_lost: lost,
        })
    }
}

async fn copy_missing<T, R>(
    from: &R,
    into: &R,
    id: impl Fn(&T) -> Id,
    mut adapt: impl FnMut(T) -> Option<T>,
) -> AppResult<usize>
where
    R: Repository<T> + ?Sized,
{
    let have: HashSet<Id> = Repository::<T>::list(into).await?.iter().map(&id).collect();
    let mut added = 0;
    for item in Repository::<T>::list(from).await? {
        if have.contains(&id(&item)) {
            continue;
        }
        if let Some(item) = adapt(item) {
            Repository::<T>::save(into, &item).await?;
            added += 1;
        }
    }
    Ok(added)
}

