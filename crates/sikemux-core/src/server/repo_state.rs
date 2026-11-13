//! What the core keeps of repositories: edits, checks, places, shipping, the person's calls and the code host, and how the rest reads it.

use super::{
    checks, codehost, collisions, edits, landing, person_calls, places, repo_events, ship, Core,
};

pub(crate) struct RepoFields {
    pub(crate) repo_above: super::repo_above::RepoAbove,
    /// The plugins the binary that runs the core provides, none in tests.
    pub(crate) plugins: Option<super::plugged::Provided>,
    /// Agents' actions that leave this machine, held for the person.
    pub(crate) ship: ship::Ships,
    /// The decisions put to the person.
    pub(crate) person_calls: person_calls::PersonCalls,
    pub(crate) places: places::store::Store,
    pub(crate) repo_events: repo_events::RepoEvents,
    pub(crate) landing: landing::Histories,
    pub(crate) land_queue: edits::queue::LandQueue,
    /// The code-host lists clients look at, kept and read again while they do.
    pub(crate) code_host_lists: codehost::Lists,
    pub(crate) ci: codehost::ci::Ci,
    pub(crate) edits: edits::Edits,
    pub(crate) collisions: collisions::Collisions,
    pub(crate) checks: checks::ProjectChecks,
}

pub(crate) trait RepoParts {
    fn repo_above(&self) -> &super::repo_above::RepoAbove;
    fn plugins(&self) -> Option<super::plugged::Provided>;
    fn ship(&self) -> &ship::Ships;
    fn person_calls(&self) -> &person_calls::PersonCalls;
    fn places(&self) -> &places::store::Store;
    fn repo_events(&self) -> &repo_events::RepoEvents;
    fn landing(&self) -> &landing::Histories;
    fn land_queue(&self) -> &edits::queue::LandQueue;
    fn code_host_lists(&self) -> &codehost::Lists;
    fn ci(&self) -> &codehost::ci::Ci;
    fn edits(&self) -> &edits::Edits;
    fn collisions(&self) -> &collisions::Collisions;
    fn checks(&self) -> &checks::ProjectChecks;
}

impl RepoParts for Core {
    fn plugins(&self) -> Option<super::plugged::Provided> {
        self.kept::<RepoFields>().plugins
    }

    fn repo_above(&self) -> &super::repo_above::RepoAbove {
        &self.kept::<RepoFields>().repo_above
    }

    fn ship(&self) -> &ship::Ships {
        &self.kept::<RepoFields>().ship
    }

    fn person_calls(&self) -> &person_calls::PersonCalls {
        &self.kept::<RepoFields>().person_calls
    }

    fn places(&self) -> &places::store::Store {
        &self.kept::<RepoFields>().places
    }

    fn repo_events(&self) -> &repo_events::RepoEvents {
        &self.kept::<RepoFields>().repo_events
    }

    fn landing(&self) -> &landing::Histories {
        &self.kept::<RepoFields>().landing
    }

    fn land_queue(&self) -> &edits::queue::LandQueue {
        &self.kept::<RepoFields>().land_queue
    }

    fn code_host_lists(&self) -> &codehost::Lists {
        &self.kept::<RepoFields>().code_host_lists
    }

    fn ci(&self) -> &codehost::ci::Ci {
        &self.kept::<RepoFields>().ci
    }

    fn edits(&self) -> &edits::Edits {
        &self.kept::<RepoFields>().edits
    }

    fn collisions(&self) -> &collisions::Collisions {
        &self.kept::<RepoFields>().collisions
    }

    fn checks(&self) -> &checks::ProjectChecks {
        &self.kept::<RepoFields>().checks
    }
}
