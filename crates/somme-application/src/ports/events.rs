use somme_domain::Id;

use crate::ports::SyncState;

pub trait EventPublisher: Send + Sync {
    fn data_changed(&self);
    fn sync_status(&self, state: SyncState);
    fn forward_stopped(&self, rule_id: Id, reason: Option<String>);
}
