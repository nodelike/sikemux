use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use portable_pty::ExitStatus;
use serde::{Deserialize, Serialize};
use sikemux_pty::agent_detection::{
    AgentDetection, AgentDetectionState, AgentKind, DetectionConfidence, DetectionExplain,
    DetectionInput, ManifestRegistry,
};

use crate::protocol::{AgentStateEvent, Event};

use super::session::Session;
use super::{now_ms, Core, CoreResult};

const ACTIVITY_SETTLE: Duration = Duration::from_secs(2);
const UNKNOWN: u8 = 0;
const IDLE: u8 = 1;
const WORKING: u8 = 2;
const BLOCKED: u8 = 3;
const STOPPED: u8 = 4;

fn state_label(state: u8) -> &'static str {
    match state {
        IDLE => "idle",
        WORKING => "working",
        BLOCKED => "blocked",
        STOPPED => "stopped",
        _ => "unknown",
    }
}

fn sequence() -> &'static AtomicU64 {
    static NEXT: OnceLock<AtomicU64> = OnceLock::new();
    NEXT.get_or_init(|| {
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_micros() as u64)
            .unwrap_or(1);
        AtomicU64::new(micros.max(1))
    })
}

/// Every agent's events share one ordering, so a late event from a replaced
/// terminal never overwrites a newer one. It starts from the clock so a core
/// that restarts under a running app keeps counting upwards.
fn next_sequence() -> u64 {
    sequence().fetch_add(1, Ordering::AcqRel)
}

pub(crate) fn sequence_mark() -> u64 {
    sequence().load(Ordering::Acquire)
}

/// Carries on from the sequence an earlier core reached.
pub(crate) fn continue_sequence(mark: u64) {
    sequence().fetch_max(mark, Ordering::AcqRel);
}

/// The part of an agent's activity a replacement core carries on from.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRecord {
    armed: bool,
    state: u8,
    silenced: bool,
    last_published: u64,
    idle_confirmations: u8,
}

/// What the core infers about an agent running in a terminal, so its state is
/// known even while no client is looking at it.
pub(crate) struct AgentActivity {
    agent_id: String,
    kind: Option<AgentKind>,
    armed: AtomicBool,
    state: AtomicU8,
    /// Set by an explicit kill: a replacement may already be running, so this
    /// one must not report anything more.
    silenced: AtomicBool,
    last_published: AtomicU64,
    idle_confirmations: AtomicU8,
    /// Advances whenever submitted input or output changes what the screen
    /// may say, so detection runs only when there is something new to read.
    revision: AtomicU64,
    last_detection_fingerprint: AtomicU64,
    last_detection_revision: AtomicU64,
}

impl AgentActivity {
    pub(crate) fn new(
        agent_id: Option<&str>,
        agent_type: Option<&str>,
        initial_prompt_submitted: bool,
    ) -> Option<Self> {
        let agent_id = agent_id.filter(|id| !id.is_empty())?;
        Some(Self {
            agent_id: agent_id.to_string(),
            kind: agent_type.and_then(AgentKind::from_label),
            armed: AtomicBool::new(initial_prompt_submitted),
            state: AtomicU8::new(UNKNOWN),
            silenced: AtomicBool::new(false),
            last_published: AtomicU64::new(0),
            idle_confirmations: AtomicU8::new(0),
            revision: AtomicU64::new(0),
            last_detection_fingerprint: AtomicU64::new(0),
            last_detection_revision: AtomicU64::new(0),
        })
    }

    pub(crate) fn record(&self) -> AgentRecord {
        AgentRecord {
            armed: self.armed.load(Ordering::Acquire),
            state: self.state.load(Ordering::Acquire),
            silenced: self.silenced.load(Ordering::Acquire),
            last_published: self.last_published.load(Ordering::Acquire),
            idle_confirmations: self.idle_confirmations.load(Ordering::Acquire),
        }
    }

    /// The screen is read again on the next poll, as the rules may differ.
    pub(crate) fn restored(
        agent_id: Option<&str>,
        agent_type: Option<&str>,
        record: &AgentRecord,
    ) -> Option<Self> {
        let activity = Self::new(agent_id, agent_type, record.armed)?;
        activity.state.store(record.state, Ordering::Release);
        activity.silenced.store(record.silenced, Ordering::Release);
        activity
            .last_published
            .store(record.last_published, Ordering::Release);
        activity
            .idle_confirmations
            .store(record.idle_confirmations, Ordering::Release);
        activity.revision.store(1, Ordering::Release);
        Some(activity)
    }

    pub(crate) fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub(crate) fn kind(&self) -> Option<AgentKind> {
        self.kind
    }

    pub(crate) fn state_label(&self) -> Option<&'static str> {
        self.kind
            .map(|_| state_label(self.state.load(Ordering::Acquire)))
    }

    pub(crate) fn silence(&self) {
        self.silenced.store(true, Ordering::Release);
    }

    pub(crate) fn note_parsed(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn invalidate_detection(&self) {
        self.last_detection_fingerprint.store(0, Ordering::Release);
        self.last_detection_revision.store(0, Ordering::Release);
    }

    fn publish(
        &self,
        core: &Core,
        next: u8,
        source: &str,
        confidence: &str,
        reason: impl Into<String>,
        matched_rule: Option<String>,
    ) {
        if self.silenced.load(Ordering::Acquire) {
            return;
        }
        let reason = reason.into();
        let label = state_label(next);
        let fingerprint = event_fingerprint(
            next,
            label,
            source,
            confidence,
            &reason,
            matched_rule.as_deref(),
        );
        if self.last_published.swap(fingerprint, Ordering::AcqRel) == fingerprint {
            return;
        }
        self.state.store(next, Ordering::Release);
        core.broadcast_event(&Event::AgentState(AgentStateEvent {
            agent_id: self.agent_id.clone(),
            state: label.into(),
            sequence: next_sequence(),
            source: source.into(),
            confidence: confidence.into(),
            reason,
            matched_rule,
        }));
    }
}

pub(crate) fn publish_start(core: &Core, agent: &AgentActivity) {
    if agent.kind.is_none() {
        return;
    }
    if agent.armed.load(Ordering::Acquire) {
        agent.publish(
            core,
            WORKING,
            "activity",
            "high",
            "initial prompt submitted",
            None,
        );
    } else {
        agent.publish(
            core,
            IDLE,
            "process",
            "high",
            "agent ready; no prompt submitted",
            None,
        );
    }
}

pub(crate) fn submits_line(bytes: &[u8]) -> bool {
    bytes.iter().any(|byte| matches!(byte, b'\r' | b'\n'))
}

pub(crate) fn note_input(core: &Core, session: &Session, bytes: &[u8]) {
    let Some(agent) = session.agent.as_ref() else {
        return;
    };
    if !submits_line(bytes) {
        return;
    }
    agent.armed.store(true, Ordering::Release);
    session.last_activity_ms.store(now_ms(), Ordering::Relaxed);
    agent.idle_confirmations.store(0, Ordering::Release);
    agent.revision.fetch_add(1, Ordering::AcqRel);
    agent.publish(core, WORKING, "activity", "high", "command submitted", None);
}

/// Startup banners and the first paint are output but not work. An agent
/// stays ready until a submitted line arms it.
pub(crate) fn note_output(core: &Core, agent: &AgentActivity) {
    if agent.kind.is_some() && agent.armed.load(Ordering::Acquire) {
        agent.idle_confirmations.store(0, Ordering::Release);
        agent.publish(
            core,
            WORKING,
            "activity",
            "medium",
            "agent produced output",
            None,
        );
    }
}

pub(crate) fn note_exit(core: &Core, agent: &AgentActivity, status: Option<&ExitStatus>) {
    if agent.kind.is_none() {
        return;
    }
    let reason = match status {
        None => "agent process stopped; exit status unavailable".to_string(),
        Some(status) if status.success() => "agent process stopped".to_string(),
        Some(status) => status.signal().map_or_else(
            || format!("agent process stopped with code {}", status.exit_code()),
            |signal| format!("agent process stopped from signal {signal}"),
        ),
    };
    agent.publish(core, STOPPED, "process", "high", reason, None);
}

fn detection_input<'a>(recent: &'a str, title: &'a str) -> DetectionInput<'a> {
    if title.is_empty() {
        DetectionInput::screen(recent)
    } else {
        DetectionInput {
            recent_screen: recent,
            osc_title: title,
            osc_progress: "",
        }
    }
}

fn screen_text(session: &Session) -> Option<(String, String)> {
    let parser = session.parser.lock().ok()?;
    Some((
        parser.screen().contents(),
        parser.callbacks().window_title.clone(),
    ))
}

/// Reads a settled agent screen and publishes what it shows. Runs a few times
/// a second for every session.
pub(crate) fn poll(core: &Core, session: &Session, now: u64) {
    let Some(agent) = session.agent.as_ref() else {
        return;
    };
    let Some(kind) = agent.kind else {
        return;
    };
    if !agent.armed.load(Ordering::Acquire)
        || now.saturating_sub(session.last_activity_ms.load(Ordering::Relaxed))
            < ACTIVITY_SETTLE.as_millis() as u64
    {
        return;
    }
    let revision = agent.revision.load(Ordering::Acquire);
    if agent.last_detection_revision.load(Ordering::Acquire) == revision {
        return;
    }
    let Some((recent, title)) = screen_text(session) else {
        return;
    };
    let fingerprint = semantic_fingerprint(revision, &recent, &title);
    if agent.last_detection_fingerprint.load(Ordering::Acquire) == fingerprint {
        return;
    }
    let detection = match core.detection.read() {
        Ok(registry) => registry.detect(kind, detection_input(&recent, &title)),
        Err(_) => return,
    };
    if detection.skip_state_update {
        agent
            .last_detection_fingerprint
            .store(fingerprint, Ordering::Release);
        agent
            .last_detection_revision
            .store(revision, Ordering::Release);
        return;
    }
    let next = match detection.state {
        AgentDetectionState::Unknown => UNKNOWN,
        AgentDetectionState::Idle => IDLE,
        AgentDetectionState::Working => WORKING,
        AgentDetectionState::Blocked => BLOCKED,
    };
    if next == IDLE {
        let confirmations = agent.idle_confirmations.fetch_add(1, Ordering::AcqRel) + 1;
        if confirmations < 2 {
            return;
        }
    } else {
        agent.idle_confirmations.store(0, Ordering::Release);
    }
    let source = if detection.fallback_reason.is_some() {
        "fallback"
    } else {
        "screen"
    };
    let confidence = match detection.confidence {
        DetectionConfidence::Authoritative | DetectionConfidence::Strong => "high",
        DetectionConfidence::Fallback => "low",
    };
    let reason = detection_reason(&detection);
    agent.publish(
        core,
        next,
        source,
        confidence,
        reason,
        detection.matched_rule,
    );
    agent
        .last_detection_fingerprint
        .store(fingerprint, Ordering::Release);
    agent
        .last_detection_revision
        .store(revision, Ordering::Release);
}

pub(crate) fn explain(
    registry: &ManifestRegistry,
    session: &Session,
) -> CoreResult<DetectionExplain> {
    let kind = session
        .agent
        .as_ref()
        .and_then(AgentActivity::kind)
        .ok_or("invalid argument: terminal has no known agent type")?;
    let (recent, title) = screen_text(session).ok_or("agent terminal parser lock poisoned")?;
    Ok(registry.explain(kind, detection_input(&recent, &title)))
}

fn semantic_fingerprint(revision: u64, screen: &str, title: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    revision.hash(&mut hasher);
    screen.hash(&mut hasher);
    title.hash(&mut hasher);
    // Zero means "never evaluated".
    hasher.finish().max(1)
}

fn event_fingerprint(
    state: u8,
    label: &str,
    source: &str,
    confidence: &str,
    reason: &str,
    matched_rule: Option<&str>,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    state.hash(&mut hasher);
    label.hash(&mut hasher);
    source.hash(&mut hasher);
    confidence.hash(&mut hasher);
    reason.hash(&mut hasher);
    matched_rule.hash(&mut hasher);
    hasher.finish().max(1)
}

fn detection_reason(detection: &AgentDetection) -> String {
    if let Some(fallback) = detection.fallback_reason.as_deref() {
        return format!("agent detection fallback: {fallback}");
    }
    let Some(rule) = detection.matched_rule.as_deref() else {
        return format!(
            "agent screen evaluated with manifest {}",
            detection.manifest_version
        );
    };
    let evidence = &detection.evidence;
    let visible = if evidence.visible_blocker {
        "visible blocker"
    } else if evidence.visible_working {
        "visible working status"
    } else if evidence.visible_idle {
        "visible idle prompt"
    } else {
        "screen evidence"
    };
    match evidence.region.as_deref() {
        Some(region) => format!("manifest rule {rule} matched {visible} in {region}"),
        None => format!("manifest rule {rule} matched {visible}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{event_fingerprint, next_sequence, semantic_fingerprint, submits_line};

    #[test]
    fn only_submitted_input_arms_agent_activity() {
        assert!(submits_line(b"ship it\r"));
        assert!(submits_line(b"first\nsecond"));
        assert!(!submits_line(b"still typing"));
        assert!(!submits_line(b"\x1b[A"));
    }

    #[test]
    fn semantic_fingerprint_changes_with_evidence_or_revision() {
        let base = semantic_fingerprint(1, "prompt", "Codex");
        assert_eq!(base, semantic_fingerprint(1, "prompt", "Codex"));
        assert_ne!(base, semantic_fingerprint(2, "prompt", "Codex"));
        assert_ne!(base, semantic_fingerprint(1, "working", "Codex"));
        assert_ne!(base, semantic_fingerprint(1, "prompt", "Action required"));
    }

    #[test]
    fn event_fingerprint_preserves_same_state_evidence_upgrades() {
        let activity =
            event_fingerprint(1, "working", "activity", "high", "command submitted", None);
        let screen = event_fingerprint(
            1,
            "working",
            "screen",
            "high",
            "manifest rule spinner matched visible working status",
            Some("spinner"),
        );
        let changed_reason = event_fingerprint(
            1,
            "working",
            "screen",
            "high",
            "manifest rule tool matched visible working status",
            Some("tool"),
        );
        assert_ne!(activity, screen);
        assert_ne!(screen, changed_reason);
        assert_eq!(
            screen,
            event_fingerprint(
                1,
                "working",
                "screen",
                "high",
                "manifest rule spinner matched visible working status",
                Some("spinner")
            )
        );
    }

    #[test]
    fn sequences_rise_and_stay_exact_in_a_javascript_number() {
        let first = next_sequence();
        let second = next_sequence();
        assert!(second > first);
        assert!(second < 1 << 53);
    }
}
