//! What the core keeps of repositories: edits, checks, places, shipping, the person's calls and the code host, and how the rest reads it.

use super::{
    checks, codehost, collisions, edits, landing, person_calls, places, repo_events, ship, Core,
};

pub struct RepoFields {
    pub repo_above: super::repo_above::RepoAbove,
    /// The plugins the binary that runs the core provides, none in tests.
    pub plugins: Option<super::plugged::Provided>,
    /// Agents' actions that leave this machine, held for the person.
    pub ship: ship::Ships,
    /// The decisions put to the person.
    pub person_calls: person_calls::PersonCalls,
    pub places: places::store::Store,
    pub repo_events: repo_events::RepoEvents,
    pub landing: landing::Histories,
    pub land_queue: edits::queue::LandQueue,
    /// The code-host lists clients look at, kept and read again while they do.
    pub code_host_lists: codehost::Lists,
    pub ci: codehost::ci::Ci,
    pub edits: edits::Edits,
    pub collisions: collisions::Collisions,
    pub checks: checks::ProjectChecks,
    pub ports: super::ports::Ports,
    pub build_outputs: super::worktree::build_output::BuildOutputs,
}

pub trait RepoParts {
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
    fn ports(&self) -> &super::ports::Ports;
    fn build_outputs(&self) -> &super::worktree::build_output::BuildOutputs;
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

    fn ports(&self) -> &super::ports::Ports {
        &self.kept::<RepoFields>().ports
    }

    fn build_outputs(&self) -> &super::worktree::build_output::BuildOutputs {
        &self.kept::<RepoFields>().build_outputs
    }
}
