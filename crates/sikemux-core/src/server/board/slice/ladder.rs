//! The ladders a slice climbs, and the one table of what each step needs, who passes it and what it is called.

use super::*;

/// The ladder a card takes once it makes a promise, in a project with no environments. Whether
/// the work looks right is asked once for its whole goal, not of each slice.
pub(crate) const LADDER: [Gate; 2] = [Gate::Built, Gate::Landed];

/// The ladder a project gives its slices: its environments, when it has them, take the place of
/// what follows Built.
pub(crate) fn ladder_of(settings: &BoardSettings) -> Vec<Step> {
    if (settings.environments.iter()).any(|environment| !environment.no_step) {
        return ladder_steps(settings.landing == Landing::Person, &settings.environments);
    }
    LADDER.iter().copied().map(Step::Gate).collect()
}

/// What a step must have before it can pass, and what it is called. An
/// environment's step is called and named by the environment's name.
pub(crate) struct Rule<'a> {
    pub needs: &'static [Need],
    pub words: &'a str,
    /// What ids and agents name it by.
    pub key: &'a str,
}

pub(crate) fn rule(step: &Step) -> Rule<'_> {
    match step {
        Step::Gate(Gate::Built) => Rule {
            needs: &[Need::PassingCheck, Need::Assertions, Need::Changes],
            words: "Built",
            key: "built",
        },
        Step::Gate(Gate::LooksRight) => Rule {
            needs: &[],
            words: "Looks right",
            key: "looksRight",
        },
        Step::Gate(Gate::Landed) => Rule {
            needs: &[Need::OnBranch],
            words: "Landed",
            key: "landed",
        },
        Step::Environment(name) => Rule {
            needs: &[Need::OnBranch],
            words: name,
            key: name,
        },
    }
}

/// Who passes `step` in a project set as `settings`: the one table of who
/// passes what. An environment it no longer has is the person's, and so is
/// Looks right, which a goal waits at.
pub(crate) fn passer(step: &Step, settings: &BoardSettings) -> Passer {
    match step {
        Step::Gate(Gate::Built) => Passer::Owner,
        Step::Gate(Gate::LooksRight) => Passer::Person,
        Step::Gate(Gate::Landed) => match settings.landing {
            Landing::Person => Passer::Person,
            Landing::Core => Passer::Core,
        },
        Step::Environment(name) => match settings.passes(name) {
            Some(Passes::Nobody | Passes::Check) => Passer::Core,
            Some(Passes::Person | Passes::Production) | None => Passer::Person,
        },
    }
}

pub(crate) fn gate_words(step: &Step) -> &str {
    rule(step).words
}

pub(crate) fn gate_key(step: &Step) -> &str {
    rule(step).key
}

/// What `step` needs in a project set as `settings`: its rule's, and the
/// check on top for an environment that goes once the check passes.
pub(crate) fn needs_of(step: &Step, settings: &BoardSettings) -> Vec<Need> {
    let mut needs = rule(step).needs.to_vec();
    if let Step::Environment(name) = step {
        if settings.passes(name) == Some(Passes::Check) {
            needs.push(Need::PassingCheck);
        }
    }
    needs
}

/// The step a piece of evidence was collected toward.
pub(crate) fn toward(evidence: &Evidence) -> Step {
    Step::named(evidence.gate, evidence.environment.as_deref())
}

pub(crate) fn need_words(need: Need) -> &'static str {
    match need {
        Need::PassingCheck => "the project's check to pass on its latest commit",
        Need::OnBranch => "its commits on the project's branch",
        Need::Assertions => "evidence on its latest commit for each assertion it fulfils",
        Need::Changes => "each change the person asked for ticked off",
    }
}
