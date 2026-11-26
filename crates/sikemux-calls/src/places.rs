use serde::{Deserialize, Serialize};
use sikemux_view::places::Level;
use sikemux_view::places::Place as Shown;
use sikemux_view::places::Recognise;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacesParams {
    pub project: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceLevelParams {
    pub project: String,
    pub place: String,
    pub level: Level,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddPlaceParams {
    pub project: String,
    pub name: String,
    pub production: bool,
    pub recognise: Vec<Recognise>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenParams {
    pub project: String,
    pub seen: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    #[serde(flatten)]
    pub shown: Shown,
    pub recognise: Vec<Recognise>,
    /// An address that answers once a deploy there is healthy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    /// The step it gives the project's slices, named by the place's name; none for no step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<StepPass>,
    /// What changes it is done again and again, such as a nightly release, so its call offers
    /// to stop asking.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recurs: bool,
    /// The person said on a call to just tell them from now on, which goes for a plain push or
    /// merge to a repository in `just_tell` alone: anything else there still waits for their yes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub routine_only: bool,
    /// The repositories the person said that about.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub just_tell: Vec<String>,
    /// The person said on a call to always allow changes there, which goes for ordinary changes
    /// alone: a tag, a release or a push that does more than send commits still waits for them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ordinary_only: bool,
}

/// What a slice needs at a place's step besides a deploy there that went well.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepPass {
    Deploy,
    /// Released settings still carry it; a place with it goes like any other.
    Check,
}

impl Place {
    pub fn name(&self) -> &str {
        &self.shown.name
    }

    /// What agents may do there now: a production place is at `mine` until the person
    /// confirms the list, or sets this place's level themselves.
    pub fn level(&self, confirmed: bool) -> Level {
        self.level_for(confirmed, None, true)
    }

    /// [`Self::level`] for one change there, through the repository `routine` when it is a
    /// routine one. `ordinary` says it is no tag, release or push that does more than send
    /// commits.
    pub fn level_for(&self, confirmed: bool, routine: Option<&str>, ordinary: bool) -> Level {
        let unseen = !confirmed && self.shown.said_by.is_none();
        let said = routine.is_some_and(|to| self.just_tell.iter().any(|said| said == to));
        let beyond = (self.routine_only && !said) || (self.ordinary_only && !ordinary);
        match self.shown.production && (unseen || beyond) {
            true => Level::Mine,
            false => self.shown.level,
        }
    }

    /// Whether a change there waits for the person's yes.
    pub fn asks(&self, confirmed: bool) -> bool {
        self.shown.production && self.level(confirmed) == Level::Mine
    }
}
