use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use somme_domain::{
    DomainError, Group, Host, Id, Identity, KnownHost, PortForward, Repository, Sealed, Snippet,
    SshKey, VaultRepository,
};

use crate::dto::{AccountStatus, MergeReport, SyncStatusView};
use crate::error::{AppError, AppResult};
use crate::ports::{
    CloudAccounts, CloudSession, EventPublisher, MasterKey, SessionStore, SyncGateway, SyncOutcome, VaultData,
};
use crate::services::VaultService;

const MIN_PASSWORD_LEN: usize = 10;

pub struct SyncService {
    gateway: Arc<dyn SyncGateway>,
    cloud: Arc<dyn CloudAccounts>,
    sessions: Arc<dyn SessionStore>,
    vault: Arc<VaultService>,
    events: Arc<dyn EventPublisher>,
    data: Arc<dyn VaultData>,
    previous_key: Mutex<Option<MasterKey>>,
}

impl SyncService {
    pub fn new(
        gateway: Arc<dyn SyncGateway>,
        cloud: Arc<dyn CloudAccounts>,
        sessions: Arc<dyn SessionStore>,
        vault: Arc<VaultService>,
        events: Arc<dyn EventPublisher>,
        data: Arc<dyn VaultData>,
    ) -> Self {
        Self {
            gateway,
            cloud,
            sessions,
            vault,
            events,
            data,
            previous_key: Mutex::new(None),
        }
    }

    pub fn status(&self) -> SyncStatusView {
        self.gateway.state().into()
    }

    pub fn account(&self) -> AccountStatus {
        let session = self.sessions.load();
        AccountStatus {
            signed_in: session.is_some(),
            email: session.map(|s| s.email),
        }
    }

    pub async fn register(&self, email: &str, password: &str) -> AppResult<AccountStatus> {
        let (email, password) = validate(email, password)?;
        let session = self.cloud.register(&email, password).await?;
        self.start(session).await
    }

    pub async fn login(&self, email: &str, password: &str) -> AppResult<AccountStatus> {
        let (email, password) = validate(email, password)?;
        let session = self.cloud.login(&email, password).await?;
        self.start(session).await
    }

    pub async fn logout(&self) -> AppResult<AccountStatus> {
        self.gateway.disconnect().await?;
        if let Some(session) = self.sessions.load()
            && let Err(err) = self.cloud.logout(&session).await {
                tracing::warn!(%err, "remote sign out failed");
            }
        self.sessions.clear()?;
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

fn validate<'a>(email: &str, password: &'a str) -> AppResult<(String, &'a str)> {
    let email = email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(DomainError::Invalid("enter a valid email address".into()).into());
    }
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(DomainError::Invalid(format!(
            "password must have at least {MIN_PASSWORD_LEN} characters"
        ))
        .into());
    }
    Ok((email, password))
}
