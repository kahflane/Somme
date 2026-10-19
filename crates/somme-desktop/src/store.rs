use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Image, ImageFormat};
use somme_application::AppResult;
use somme_application::dto::{
    AccountStatus, AutomationView, GroupView, HostView, IdentityView, KeyView, KnownHostView, PortForwardView, SnippetView,
    SyncStatusView, VaultStatus,
};
use somme_domain::Id;

use crate::runtime;
use crate::services::Services;

/// Everything the screens show, loaded from the backend and kept fresh.
pub struct Store {
    pub services: Arc<Services>,
    pub vault: Option<VaultStatus>,
    pub groups: Vec<GroupView>,
    pub hosts: Vec<HostView>,
    pub identities: Vec<IdentityView>,
    pub keys: Vec<KeyView>,
    pub snippets: Vec<SnippetView>,
    /// Opened with the vault, so empty while it is locked.
    pub automations: Vec<AutomationView>,
    pub forwards: Vec<PortForwardView>,
    pub active_forwards: HashSet<Id>,
    pub known_hosts: Vec<KnownHostView>,
    pub sync: SyncStatusView,
    pub account: AccountStatus,
    /// The signed-in user's GitHub picture, once downloaded.
    pub avatar: Option<Arc<Image>>,
    /// A GitHub sign-in is waiting on the browser.
    pub signing_in: bool,
    /// Set once the vault and sign-in have been read from the keychain.
    keychain_asked: bool,
    pub load_error: Option<String>,
    refresh_generation: usize,
}

pub enum StoreEvent {
    VaultChanged,
}

impl EventEmitter<StoreEvent> for Store {}

struct GlobalStore(Entity<Store>);

impl Global for GlobalStore {}

struct Lists {
    groups: Vec<GroupView>,
    hosts: Vec<HostView>,
    identities: Vec<IdentityView>,
    keys: Vec<KeyView>,
    snippets: Vec<SnippetView>,
    automations: Vec<AutomationView>,
    forwards: Vec<PortForwardView>,
    known_hosts: Vec<KnownHostView>,
}

async fn load_lists(services: Arc<Services>) -> AppResult<Lists> {
    Ok(Lists {
        groups: services.groups.list().await?,
        hosts: services.hosts.list().await?,
        identities: services.identities.list().await?,
        keys: services.keys.list().await?,
        snippets: services.snippets.list().await?,
        automations: services.automations.list().await?,
        forwards: services.forwards.list().await?,
        known_hosts: services.known_hosts.list().await?,
    })
}

impl Store {
    pub fn init(services: Arc<Services>, cx: &mut App) -> Entity<Self> {
        let sync = services.sync.status();
        let account = AccountStatus {
            signed_in: false,
            login: None,
            name: None,
            avatar_url: None,
        };
        let store = cx.new(|cx| {
            let mut store = Self {
                services,
                vault: None,
                groups: Vec::new(),
                hosts: Vec::new(),
                identities: Vec::new(),
                keys: Vec::new(),
                snippets: Vec::new(),
                automations: Vec::new(),
                forwards: Vec::new(),
                active_forwards: HashSet::new(),
                known_hosts: Vec::new(),
                sync,
                account,
                avatar: None,
                signing_in: false,
                keychain_asked: false,
                load_error: None,
                refresh_generation: 0,
            };
            store.load_account(cx);
            store.refresh(cx);
            store
        });
        cx.set_global(GlobalStore(store.clone()));
        store
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalStore>().0.clone()
    }

    pub fn services(cx: &App) -> Arc<Services> {
        Self::global(cx).read(cx).services.clone()
    }

    pub fn is_unlocked(&self) -> bool {
        self.vault.is_some_and(|v| v.unlocked)
    }

    /// Opens what the SSH section needs. The keychain can stop and ask the
    /// user, so nothing reads it before this: the vault key unlocks the vault,
    /// and the GitHub sign-in lets sync start.
    pub fn open_vault(&mut self, cx: &mut Context<Self>) {
        if self.keychain_asked {
            return;
        }
        self.keychain_asked = true;
        self.check_vault(cx);
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let account = runtime::io(async move {
                services.start_sync();
                services.sync.load_session()
            })
            .await;
            let _ = this.update(cx, |store, cx| store.set_account(account, cx));
        })
        .detach();
    }

    pub fn check_vault(&mut self, cx: &mut Context<Self>) {
        let vault = self.services.vault.clone();
        cx.spawn(async move |this, cx| {
            let status = runtime::io(async move { vault.status().await }).await;
            let _ = this.update(cx, |store, cx| match status {
                Ok(status) => store.set_vault(status, cx),
                Err(err) => {
                    store.load_error = Some(err.to_string());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn set_vault(&mut self, status: VaultStatus, cx: &mut Context<Self>) {
        let was_unlocked = self.is_unlocked();
        self.vault = Some(status);
        if was_unlocked && !status.unlocked {
            self.clear_lists();
        }
        self.refresh(cx);
        cx.emit(StoreEvent::VaultChanged);
        cx.notify();
    }

    /// Snippets stay: they are not secret, and local terminals use them
    /// without the master password.
    fn clear_lists(&mut self) {
        self.groups.clear();
        self.hosts.clear();
        self.identities.clear();
        self.keys.clear();
        self.automations.clear();
        self.forwards.clear();
        self.known_hosts.clear();
    }

    /// Reloads every list, or only snippets while the vault is locked. Older
    /// reloads that finish late are ignored.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.is_unlocked() {
            self.refresh_snippets(cx);
            return;
        }
        self.refresh_generation += 1;
        let generation = self.refresh_generation;
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let lists = runtime::io(load_lists(services.clone())).await;
            let active = services.forwards.active();
            let _ = this.update(cx, |store, cx| {
                if store.refresh_generation != generation {
                    return;
                }
                match lists {
                    Ok(lists) => {
                        store.groups = lists.groups;
                        store.hosts = lists.hosts;
                        store.identities = lists.identities;
                        store.keys = lists.keys;
                        store.snippets = lists.snippets;
                        store.automations = lists.automations;
                        store.forwards = lists.forwards;
                        store.known_hosts = lists.known_hosts;
                        store.load_error = None;
                    }
                    Err(err) => store.load_error = Some(err.to_string()),
                }
                store.active_forwards = active.into_iter().collect();
                store.sync = store.services.sync.status();
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_snippets(&mut self, cx: &mut Context<Self>) {
        self.refresh_generation += 1;
        let generation = self.refresh_generation;
        let snippets = self.services.snippets.clone();
        cx.spawn(async move |this, cx| {
            let list = runtime::io(async move { snippets.list().await }).await;
            let _ = this.update(cx, |store, cx| {
                if store.refresh_generation != generation {
                    return;
                }
                match list {
                    Ok(list) => store.snippets = list,
                    Err(err) => store.load_error = Some(err.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows who is signed in at start. This reads the saved profile, not the
    /// keychain.
    fn load_account(&mut self, cx: &mut Context<Self>) {
        let sync = self.services.sync.clone();
        cx.spawn(async move |this, cx| {
            let account = runtime::io(async move { sync.account() }).await;
            let _ = this.update(cx, |store, cx| store.set_account(account, cx));
        })
        .detach();
    }

    /// Re-reads the signed-in account, after signing in or out.
    pub fn refresh_account(&mut self, cx: &mut Context<Self>) {
        let account = self.services.sync.account();
        self.set_account(account, cx);
    }

    fn set_account(&mut self, account: AccountStatus, cx: &mut Context<Self>) {
        let new_picture = account.avatar_url != self.account.avatar_url;
        self.account = account;
        if new_picture {
            self.avatar = None;
            self.load_avatar(cx);
        }
        cx.notify();
    }

    fn load_avatar(&mut self, cx: &mut Context<Self>) {
        if self.account.avatar_url.is_none() {
            return;
        }
        let sync = self.services.sync.clone();
        cx.spawn(async move |this, cx| match runtime::io(async move { sync.avatar().await }).await {
            Ok(Some(bytes)) => {
                let avatar = image_format(&bytes).map(|format| Arc::new(Image::from_bytes(format, bytes)));
                let _ = this.update(cx, |store, cx| {
                    store.avatar = avatar;
                    cx.notify();
                });
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(%err, "could not load the GitHub picture"),
        })
        .detach();
    }

    pub fn lock(&mut self, cx: &mut Context<Self>) {
        let vault = self.services.vault.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(status) = runtime::io(async move { vault.lock().await }).await {
                let _ = this.update(cx, |store, cx| store.set_vault(status, cx));
            }
        })
        .detach();
    }

    pub fn refresh_forwards(&mut self, cx: &mut Context<Self>) {
        self.active_forwards = self.services.forwards.active().into_iter().collect();
        cx.notify();
    }

    pub fn host(&self, id: Id) -> Option<&HostView> {
        self.hosts.iter().find(|h| h.id == id)
    }

    pub fn group(&self, id: Id) -> Option<&GroupView> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn key(&self, id: Id) -> Option<&KeyView> {
        self.keys.iter().find(|k| k.id == id)
    }

    pub fn identity(&self, id: Id) -> Option<&IdentityView> {
        self.identities.iter().find(|i| i.id == id)
    }

    /// Every tag used by any host, sorted.
    pub fn tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = self.hosts.iter().flat_map(|h| h.tags.iter().cloned()).collect();
        tags.sort_by_key(|t| t.to_lowercase());
        tags.dedup();
        tags
    }
}

pub(crate) fn image_format(bytes: &[u8]) -> Option<ImageFormat> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some(ImageFormat::Png),
        [0xFF, 0xD8, 0xFF, ..] => Some(ImageFormat::Jpeg),
        [b'G', b'I', b'F', b'8', ..] => Some(ImageFormat::Gif),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some(ImageFormat::Webp),
        _ => None,
    }
}

/// Runs a backend call on the IO runtime, then hands the result back on the UI thread.
/// Every list is reloaded afterwards so all screens see the change.
pub fn mutate<V, T, F>(
    window: &mut gpui_kit::Window,
    cx: &mut Context<V>,
    call: impl FnOnce(Arc<Services>) -> F,
    done: impl FnOnce(&mut V, AppResult<T>, &mut gpui_kit::Window, &mut Context<V>) + 'static,
) where
    V: 'static,
    T: Send + 'static,
    F: Future<Output = AppResult<T>> + Send + 'static,
{
    let future = call(Store::services(cx));
    cx.spawn_in(window, async move |this, cx| {
        let result = runtime::io(future).await;
        let _ = cx.update(|_, cx| Store::global(cx).update(cx, |store, cx| store.refresh(cx)));
        let _ = this.update_in(cx, |view, window, cx| done(view, result, window, cx));
    })
    .detach();
}

/// `mutate` for code that only has a weak handle to the view, like dialog buttons.
pub fn mutate_weak<V, T, F>(
    view: &gpui_kit::WeakEntity<V>,
    window: &mut gpui_kit::Window,
    cx: &mut App,
    call: impl FnOnce(Arc<Services>) -> F,
    done: impl FnOnce(&mut V, AppResult<T>, &mut gpui_kit::Window, &mut Context<V>) + 'static,
) where
    V: 'static,
    T: Send + 'static,
    F: Future<Output = AppResult<T>> + Send + 'static,
{
    let _ = view.update(cx, |_, cx| mutate(window, cx, call, done));
}

/// A `done` handler for `mutate` that only shows errors.
pub fn report<V, T>(
    title: &'static str,
) -> impl FnOnce(&mut V, AppResult<T>, &mut gpui_kit::Window, &mut Context<V>) + 'static {
    move |_, result, window, cx| {
        if let Err(err) = result {
            crate::ui::common::notify_error(window, cx, title, err);
        }
    }
}
