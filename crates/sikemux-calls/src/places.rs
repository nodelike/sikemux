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
}

/// What a slice needs at a place's step besides a deploy there that went well.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepPass {
    Deploy,
    /// The project's check passes first. Only a place that isn't production has it.
    Check,
}

/// Who passes a place's step on a slice's ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Passes {
    /// The core, once a deploy there went well.
    Nobody,
    /// The core, once the project's check passed and a deploy there went well.
    Check,
    /// The person.
    Person,
}

impl Place {
    pub fn name(&self) -> &str {
        &self.shown.name
    }

    /// What agents may do there now: a production place is at `mine` until the person
    /// confirms the list, or sets this place's level themselves.
    pub fn level(&self, confirmed: bool) -> Level {
        let unseen = !confirmed && self.shown.said_by.is_none();
        match self.shown.production && unseen {
            true => Level::Mine,
            false => self.shown.level,
        }
    }

    /// Whether a change there waits for the person's yes.
    pub fn asks(&self, confirmed: bool) -> bool {
        self.shown.production && self.level(confirmed) == Level::Mine
    }

    pub fn passes(&self, confirmed: bool) -> Passes {
        match (self.asks(confirmed), self.shown.production, self.step) {
            (true, _, _) => Passes::Person,
            (false, false, Some(StepPass::Check)) => Passes::Check,
            _ => Passes::Nobody,
        }
    }
}
