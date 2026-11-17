//! Why a linked host is out of reach, or why this one is, as one chain every client draws:
//! this host's remote access, its network, its account, then the path to the other host,
//! that host's core and whether it takes this one as linked. Steps after the first that
//! failed are unchecked, since each needs the one before.

use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use iroh::Watcher;
pub use sikemux_calls::diagnosis::DiagnosisParams;
use sikemux_view::host_stats::{DiagnosisStep, HostDiagnosis, ReachStep, StepState, StepWhy};
use sikemux_wire::protocol::AccountLinkState;

use super::hosts::{DialFailure, DialState};
use super::Core;

pub(crate) fn diagnose(core: &Core, params: DiagnosisParams) -> HostDiagnosis {
    let status = core.remote().status();
    let endpoint = core.remote().endpoint();
    let relay_up = endpoint.as_ref().is_some_and(|endpoint| {
        (endpoint.home_relay_status().get().iter()).any(|relay| relay.is_connected())
    });
    let account = status.account.as_ref().map(|link| link.state);
    let mut steps = vec![
        match status.enabled && endpoint.is_some() {
            true => passed(ReachStep::RemoteAccess),
            false => failed(ReachStep::RemoteAccess, StepWhy::Off),
        },
        match relay_up || account == Some(AccountLinkState::Live) {
            true => passed(ReachStep::Online),
            false => failed(ReachStep::Online, StepWhy::NoRelay),
        },
        match (status.owner.as_ref(), account) {
            (None, _) => failed(ReachStep::Account, StepWhy::SignedOut),
            (_, Some(AccountLinkState::Live)) => passed(ReachStep::Account),
            (_, Some(AccountLinkState::Removed)) => {
                failed(ReachStep::Account, StepWhy::RemovedFromAccount)
            }
            (_, Some(AccountLinkState::Offline)) => {
                failed(ReachStep::Account, StepWhy::AccountOffline)
            }
            _ => unchecked(ReachStep::Account),
        },
    ];
    let (mut since, mut tries) = (None, 0);
    if let Some(host) = params.host.as_deref() {
        let dialed = core.hosts().dial_state(host);
        if let DialState::Down {
            since: down_since,
            failures,
            ..
        } = &dialed
        {
            since = Some(*down_since);
            tries = *failures;
        }
        steps.extend(other_host(&dialed));
    }
    chain(&mut steps);
    HostDiagnosis {
        host: params.host,
        steps,
        since,
        tries,
    }
}

/// The steps on the way to another host, from how this host last fared dialing it.
fn other_host(dialed: &DialState) -> [DiagnosisStep; 3] {
    let path = |relay: bool| DiagnosisStep {
        relay: Some(relay),
        ..passed(ReachStep::Path)
    };
    match dialed {
        DialState::Connected { relay } => [
            path(*relay),
            passed(ReachStep::Core),
            passed(ReachStep::Accepted),
        ],
        DialState::Down {
            last: Some(answered @ (DialFailure::NotLinked | DialFailure::Closed)),
            ..
        } => [
            DiagnosisStep {
                relay: None,
                ..passed(ReachStep::Path)
            },
            passed(ReachStep::Core),
            failed(
                ReachStep::Accepted,
                match answered {
                    DialFailure::NotLinked => StepWhy::NotLinked,
                    _ => StepWhy::Closed,
                },
            ),
        ],
        DialState::Down {
            last: Some(failure @ (DialFailure::NoAddress | DialFailure::NoAnswer)),
            ..
        } => [
            failed(
                ReachStep::Path,
                match failure {
                    DialFailure::NoAddress => StepWhy::NoAddress,
                    _ => StepWhy::NoAnswer,
                },
            ),
            unchecked(ReachStep::Core),
            unchecked(ReachStep::Accepted),
        ],
        _ => [
            unchecked(ReachStep::Path),
            unchecked(ReachStep::Core),
            unchecked(ReachStep::Accepted),
        ],
    }
}

/// Leaves every step after the first failed one unchecked, since each needs the one before.
fn chain(steps: &mut [DiagnosisStep]) {
    let Some(first) = steps
        .iter()
        .position(|step| step.state == StepState::Failed)
    else {
        return;
    };
    for step in &mut steps[first + 1..] {
        *step = unchecked(step.step);
    }
}

fn passed(step: ReachStep) -> DiagnosisStep {
    DiagnosisStep {
        step,
        state: StepState::Passed,
        relay: None,
        why: None,
    }
}

fn failed(step: ReachStep, why: StepWhy) -> DiagnosisStep {
    DiagnosisStep {
        step,
        state: StepState::Failed,
        relay: None,
        why: Some(why),
    }
}

fn unchecked(step: ReachStep) -> DiagnosisStep {
    DiagnosisStep {
        step,
        state: StepState::Unchecked,
        relay: None,
        why: None,
    }
}
