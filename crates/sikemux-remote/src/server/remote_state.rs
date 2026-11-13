//! What the core keeps of pairing, the account, the vault, sharing and notifying, and how the rest reads it.

use super::{account, join, notify, remote, share, vault, Core};

pub struct RemoteFields {
    pub remote: remote::Remote,
    /// The account's vault of sign-ins, as this host holds it.
    pub vault: vault::Vault,
    pub notify: notify::Notifier,
    pub shares: share::Shares,
    /// Who this host is signed in to Sikemux as.
    pub account: account::Account,
    pub joins: join::Joins,
    pub remote_above: super::remote_above::RemoteAbove,
}

pub trait RemoteParts {
    fn remote(&self) -> &remote::Remote;
    fn vault(&self) -> &vault::Vault;
    fn notify(&self) -> &notify::Notifier;
    fn shares(&self) -> &share::Shares;
    fn account(&self) -> &account::Account;
    fn joins(&self) -> &join::Joins;
    fn remote_above(&self) -> &super::remote_above::RemoteAbove;
}

impl RemoteParts for Core {
    fn remote(&self) -> &remote::Remote {
        &self.kept::<RemoteFields>().remote
    }

    fn vault(&self) -> &vault::Vault {
        &self.kept::<RemoteFields>().vault
    }

    fn notify(&self) -> &notify::Notifier {
        &self.kept::<RemoteFields>().notify
    }

    fn shares(&self) -> &share::Shares {
        &self.kept::<RemoteFields>().shares
    }

    fn account(&self) -> &account::Account {
        &self.kept::<RemoteFields>().account
    }

    fn joins(&self) -> &join::Joins {
        &self.kept::<RemoteFields>().joins
    }

    fn remote_above(&self) -> &super::remote_above::RemoteAbove {
        &self.kept::<RemoteFields>().remote_above
    }
}
