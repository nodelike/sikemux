//! Pure, manifest-driven coding-agent screen detection.
//!
//! This module deliberately knows nothing about PTYs, terminal parsers, Tauri,
//! or frontend state. Callers freeze the live bottom-buffer text (and any OSC
//! title/progress metadata) and pass that snapshot to [`ManifestRegistry`].
//! Keeping this boundary pure makes the matching policy deterministic and
//! fixture-testable while the PTY remains free to choose its locking strategy.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

const MANIFEST_SCHEMA_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_RULES: usize = 128;
const MAX_GATE_DEPTH: usize = 6;
const MAX_TOTAL_MATCHERS: usize = 512;
const MAX_PATTERN_BYTES: usize = 512;
const REGION_PREVIEW_CHARS: usize = 500;
pub const DEFAULT_KNOWN_AGENT_IDLE_FALLBACK: &str = "default_known_agent_idle_fallback";

const BUNDLED_MANIFESTS: &[(AgentKind, &str)] = &[
    (AgentKind::Claude, include_str!("manifests/claude.json")),
    (AgentKind::Codex, include_str!("manifests/codex.json")),
    (AgentKind::Hermes, include_str!("manifests/hermes.json")),
    (AgentKind::Pi, include_str!("manifests/pi.json")),
    (AgentKind::OpenCode, include_str!("manifests/opencode.json")),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Hermes,
    Pi,
    OpenCode,
}

impl AgentKind {
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Hermes,
        Self::Pi,
        Self::OpenCode,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Hermes => "hermes",
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "hermes" | "hermes-agent" => Some(Self::Hermes),
            "pi" => Some(Self::Pi),
            "opencode" | "open-code" => Some(Self::OpenCode),
            _ => None,
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentDetectionState {
    Unknown,
    Idle,
    Working,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionSource {
    ScreenManifest,
    Activity,
    Process,
    ProcessExit,
    Hook,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DetectionConfidence {
    Authoritative,
    Strong,
    Fallback,
}

#[derive(Debug, Clone, Copy)]
pub struct DetectionInput<'a> {
    pub recent_screen: &'a str,
    pub osc_title: &'a str,
    pub osc_progress: &'a str,
}

impl<'a> DetectionInput<'a> {
    pub const fn screen(recent_screen: &'a str) -> Self {
        Self {
            recent_screen,
            osc_title: "",
            osc_progress: "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectionEvidence {
    pub visible_idle: bool,
    pub visible_blocker: bool,
    pub visible_working: bool,
    pub region: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDetection {
    pub agent: AgentKind,
    pub state: AgentDetectionState,
    pub source: DetectionSource,
    pub confidence: DetectionConfidence,
    pub evidence: DetectionEvidence,
    pub matched_rule: Option<String>,
    pub skip_state_update: bool,
    pub fallback_reason: Option<String>,
    pub manifest_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestSourceKind {
    Bundled,
    LocalOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSourceInfo {
    pub kind: ManifestSourceKind,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatcherEvidence {
    pub pattern: String,
    pub matched: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleEvidence {
    pub contains: Vec<MatcherEvidence>,
    pub regex: Vec<MatcherEvidence>,
    pub line_regex: Vec<MatcherEvidence>,
    pub all_gate_count: usize,
    pub all_gate_matches: usize,
    pub any_gate_count: usize,
    pub any_gate_matches: usize,
    pub not_gate_count: usize,
    pub not_gate_matches: usize,
    pub region_bytes: usize,
    pub region_preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluatedRule {
    pub id: String,
    pub priority: i32,
    pub region: String,
    pub state: AgentDetectionState,
    pub matched: bool,
    pub evidence: RuleEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectionExplain {
    pub agent: AgentKind,
    pub state: AgentDetectionState,
    pub source: ManifestSourceInfo,
    pub confidence: DetectionConfidence,
    pub matched_rule: Option<String>,
    pub screen_detection_skipped: bool,
    pub visible_idle: bool,
    pub visible_blocker: bool,
    pub visible_working: bool,
    pub skip_state_update: bool,
    pub fallback_reason: Option<String>,
    pub evaluated_rules: Vec<EvaluatedRule>,
    pub warning: Option<String>,
    pub manifest_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSummary {
    pub agent: AgentKind,
    pub version: String,
    pub source: ManifestSourceInfo,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestReloadReport {
    pub manifests: Vec<ManifestSummary>,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum ManifestError {
    Json(String),
    Invalid(String),
    Io { path: PathBuf, error: String },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "manifest JSON: {error}"),
            Self::Invalid(error) => write!(f, "invalid manifest: {error}"),
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for ManifestError {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentManifest {
    schema_version: u32,
    id: AgentKind,
    version: String,
    rules: Vec<ManifestRule>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestRule {
    id: String,
    state: AgentDetectionState,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_region")]
    region: String,
    #[serde(default)]
    visible_idle: bool,
    #[serde(default)]
    visible_blocker: bool,
    #[serde(default)]
    visible_working: bool,
    #[serde(default)]
    skip_state_update: bool,
    gate: ManifestGate,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestGate {
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    regex: Vec<String>,
    #[serde(default)]
    line_regex: Vec<String>,
    #[serde(default)]
    all: Vec<ManifestGate>,
    #[serde(default)]
    any: Vec<ManifestGate>,
    #[serde(default, rename = "not")]
    not_gate: Vec<ManifestGate>,
}

fn default_region() -> String {
    "wholeRecent".to_string()
}

#[derive(Debug, Clone)]
enum Region {
    WholeRecent,
    BottomNonEmptyLines(usize),
    AfterLastHorizontalRule,
    AfterLastPromptMarker,
    PromptBoxBody,
    OscTitle,
    OscProgress,
}

impl Region {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "wholeRecent" => Ok(Self::WholeRecent),
            "afterLastHorizontalRule" => Ok(Self::AfterLastHorizontalRule),
            "afterLastPromptMarker" => Ok(Self::AfterLastPromptMarker),
            "promptBoxBody" => Ok(Self::PromptBoxBody),
            "oscTitle" => Ok(Self::OscTitle),
            "oscProgress" => Ok(Self::OscProgress),
            _ => {
                let Some(inner) = value
                    .strip_prefix("bottomNonEmptyLines(")
                    .and_then(|rest| rest.strip_suffix(')'))
                else {
                    return Err(format!("unknown region {value:?}"));
                };
                let lines = inner
                    .parse::<usize>()
                    .map_err(|_| format!("invalid bottomNonEmptyLines count {inner:?}"))?;
                if !(1..=200).contains(&lines) {
                    return Err("bottomNonEmptyLines count must be between 1 and 200".to_string());
                }
                Ok(Self::BottomNonEmptyLines(lines))
            }
        }
    }

    fn select<'a>(&self, input: DetectionInput<'a>) -> String {
        match self {
            Self::WholeRecent => input.recent_screen.to_string(),
            Self::BottomNonEmptyLines(count) => bottom_non_empty_lines(input.recent_screen, *count),
            Self::AfterLastHorizontalRule => after_last_horizontal_rule(input.recent_screen),
            Self::AfterLastPromptMarker => after_last_prompt_marker(input.recent_screen),
            Self::PromptBoxBody => prompt_box_body(input.recent_screen),
            Self::OscTitle => input.osc_title.to_string(),
            Self::OscProgress => input.osc_progress.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct CompiledGate {
    contains: Vec<(String, String)>,
    regex: Vec<(String, Regex)>,
    line_regex: Vec<(String, Regex)>,
    all: Vec<CompiledGate>,
    any: Vec<CompiledGate>,
    not_gate: Vec<CompiledGate>,
}

#[derive(Debug, Clone)]
struct CompiledRule {
    id: String,
    state: AgentDetectionState,
    priority: i32,
    region_label: String,
    region: Region,
    visible_idle: bool,
    visible_blocker: bool,
    visible_working: bool,
    skip_state_update: bool,
    gate: CompiledGate,
}

#[derive(Debug, Clone)]
struct LoadedManifest {
    id: AgentKind,
    version: String,
    rules: Vec<CompiledRule>,
    source: ManifestSourceInfo,
    warning: Option<String>,
}

#[derive(Debug)]
pub struct ManifestRegistry {
    manifests: HashMap<AgentKind, LoadedManifest>,
    override_dir: Option<PathBuf>,
}

impl ManifestRegistry {
    pub fn bundled() -> Result<Self, ManifestError> {
        Self::load(None)
    }

    pub fn with_override_dir(path: impl Into<PathBuf>) -> Result<Self, ManifestError> {
        Self::load(Some(path.into()))
    }

    fn load(override_dir: Option<PathBuf>) -> Result<Self, ManifestError> {
        let mut manifests = HashMap::new();
        for (agent, json) in BUNDLED_MANIFESTS {
            let loaded = compile_manifest(json, *agent, bundled_source())?;
            manifests.insert(*agent, loaded);
        }
        let mut registry = Self {
            manifests,
            override_dir,
        };
        registry.apply_overrides();
        Ok(registry)
    }

    /// Reloads bundled rules and then atomically replaces each agent's rules
    /// with a valid `<agent>.json` local override, when present. Invalid or
    /// unreadable overrides leave the bundled manifest active and are surfaced
    /// as warnings; one broken override never disables the other agents.
    pub fn reload(&mut self) -> Result<ManifestReloadReport, ManifestError> {
        let replacement = Self::load(self.override_dir.clone())?;
        let report = replacement.report();
        *self = replacement;
        Ok(report)
    }

    pub fn report(&self) -> ManifestReloadReport {
        let manifests = self.summaries();
        let warnings = manifests
            .iter()
            .filter_map(|summary| summary.warning.clone())
            .collect();
        ManifestReloadReport {
            manifests,
            warnings,
        }
    }

    pub fn summaries(&self) -> Vec<ManifestSummary> {
        AgentKind::ALL
            .iter()
            .filter_map(|agent| self.manifests.get(agent))
            .map(|manifest| ManifestSummary {
                agent: manifest.id,
                version: manifest.version.clone(),
                source: manifest.source.clone(),
                warning: manifest.warning.clone(),
            })
            .collect()
    }

    pub fn detect(&self, agent: AgentKind, input: DetectionInput<'_>) -> AgentDetection {
        let explain = self.explain(agent, input);
        AgentDetection {
            agent,
            state: explain.state,
            source: if explain.fallback_reason.is_some() {
                DetectionSource::Fallback
            } else {
                DetectionSource::ScreenManifest
            },
            confidence: explain.confidence,
            evidence: DetectionEvidence {
                visible_idle: explain.visible_idle,
                visible_blocker: explain.visible_blocker,
                visible_working: explain.visible_working,
                region: explain
                    .evaluated_rules
                    .iter()
                    .find(|rule| rule.matched)
                    .map(|rule| rule.region.clone()),
            },
            matched_rule: explain.matched_rule,
            skip_state_update: explain.skip_state_update,
            fallback_reason: explain.fallback_reason,
            manifest_version: explain.manifest_version,
        }
    }

    pub fn explain(&self, agent: AgentKind, input: DetectionInput<'_>) -> DetectionExplain {
        let Some(manifest) = self.manifests.get(&agent) else {
            return missing_manifest_explain(agent);
        };

        let mut evaluated_rules = Vec::with_capacity(manifest.rules.len());
        let mut selected: Option<(&CompiledRule, bool)> = None;
        for rule in &manifest.rules {
            let region = rule.region.select(input);
            let (matched, evidence) = rule.gate.evaluate(&region);
            evaluated_rules.push(EvaluatedRule {
                id: rule.id.clone(),
                priority: rule.priority,
                region: rule.region_label.clone(),
                state: rule.state,
                matched,
                evidence,
            });
            if matched && selected.is_none() {
                selected = Some((rule, true));
            }
        }

        if let Some((rule, _)) = selected {
            return DetectionExplain {
                agent,
                state: rule.state,
                source: manifest.source.clone(),
                confidence: if rule.skip_state_update {
                    DetectionConfidence::Fallback
                } else {
                    DetectionConfidence::Strong
                },
                matched_rule: Some(rule.id.clone()),
                screen_detection_skipped: rule.skip_state_update,
                visible_idle: rule.visible_idle,
                visible_blocker: rule.visible_blocker,
                visible_working: rule.visible_working,
                skip_state_update: rule.skip_state_update,
                fallback_reason: None,
                evaluated_rules,
                warning: manifest.warning.clone(),
                manifest_version: manifest.version.clone(),
            };
        }

        DetectionExplain {
            agent,
            state: AgentDetectionState::Idle,
            source: manifest.source.clone(),
            confidence: DetectionConfidence::Fallback,
            matched_rule: None,
            screen_detection_skipped: false,
            visible_idle: false,
            visible_blocker: false,
            visible_working: false,
            skip_state_update: false,
            fallback_reason: Some(DEFAULT_KNOWN_AGENT_IDLE_FALLBACK.to_string()),
            evaluated_rules,
            warning: manifest.warning.clone(),
            manifest_version: manifest.version.clone(),
        }
    }

    fn apply_overrides(&mut self) {
        let Some(dir) = self.override_dir.as_ref() else {
            return;
        };
        for agent in AgentKind::ALL {
            let path = dir.join(format!("{}.json", agent.label()));
            match read_limited_manifest(&path)
                .and_then(|json| compile_manifest(&json, agent, override_source(&path)))
            {
                Ok(override_manifest) => {
                    self.manifests.insert(agent, override_manifest);
                }
                Err(ManifestError::Io { error, .. }) if error == "not found" => {}
                Err(error) => {
                    if let Some(manifest) = self.manifests.get_mut(&agent) {
                        manifest.warning = Some(format!(
                            "ignored local override {}: {error}",
                            path.display()
                        ));
                    }
                }
            }
        }
    }
}

fn bundled_source() -> ManifestSourceInfo {
    ManifestSourceInfo {
        kind: ManifestSourceKind::Bundled,
        path: None,
    }
}

fn override_source(path: &Path) -> ManifestSourceInfo {
    ManifestSourceInfo {
        kind: ManifestSourceKind::LocalOverride,
        path: Some(path.display().to_string()),
    }
}

fn read_limited_manifest(path: &Path) -> Result<String, ManifestError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ManifestError::Io {
                path: path.to_path_buf(),
                error: "not found".to_string(),
            });
        }
        Err(error) => {
            return Err(ManifestError::Io {
                path: path.to_path_buf(),
                error: error.to_string(),
            });
        }
    };
    if metadata.len() as usize > MAX_MANIFEST_BYTES {
        return Err(ManifestError::Invalid(format!(
            "manifest is {} bytes; maximum is {MAX_MANIFEST_BYTES}",
            metadata.len()
        )));
    }
    fs::read_to_string(path).map_err(|error| ManifestError::Io {
        path: path.to_path_buf(),
        error: error.to_string(),
    })
}

fn compile_manifest(
    json: &str,
    expected: AgentKind,
    source: ManifestSourceInfo,
) -> Result<LoadedManifest, ManifestError> {
    if json.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::Invalid(format!(
            "manifest is {} bytes; maximum is {MAX_MANIFEST_BYTES}",
            json.len()
        )));
    }
    let parsed: AgentManifest =
        serde_json::from_str(json).map_err(|error| ManifestError::Json(error.to_string()))?;
    validate_manifest(&parsed, expected)?;

    let mut rules = parsed
        .rules
        .into_iter()
        .enumerate()
        .map(|(order, rule)| compile_rule(rule).map(|compiled| (order, compiled)))
        .collect::<Result<Vec<_>, _>>()?;
    rules.sort_by(|(left_order, left), (right_order, right)| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left_order.cmp(right_order))
    });

    Ok(LoadedManifest {
        id: parsed.id,
        version: parsed.version,
        rules: rules.into_iter().map(|(_, rule)| rule).collect(),
        source,
        warning: None,
    })
}

fn validate_manifest(manifest: &AgentManifest, expected: AgentKind) -> Result<(), ManifestError> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(ManifestError::Invalid(format!(
            "schemaVersion {} is unsupported; expected {MANIFEST_SCHEMA_VERSION}",
            manifest.schema_version
        )));
    }
    if manifest.id != expected {
        return Err(ManifestError::Invalid(format!(
            "manifest id {} does not match expected {expected}",
            manifest.id
        )));
    }
    if manifest.version.trim().is_empty() || manifest.version.len() > 64 {
        return Err(ManifestError::Invalid(
            "version must contain 1-64 characters".to_string(),
        ));
    }
    if manifest.rules.is_empty() || manifest.rules.len() > MAX_RULES {
        return Err(ManifestError::Invalid(format!(
            "rules must contain 1-{MAX_RULES} entries"
        )));
    }

    let mut ids = HashSet::new();
    let mut matcher_count = 0usize;
    for rule in &manifest.rules {
        if rule.id.trim().is_empty() || rule.id.len() > 96 {
            return Err(ManifestError::Invalid(
                "rule id must contain 1-96 characters".to_string(),
            ));
        }
        if !ids.insert(rule.id.as_str()) {
            return Err(ManifestError::Invalid(format!(
                "duplicate rule id {:?}",
                rule.id
            )));
        }
        Region::parse(&rule.region).map_err(ManifestError::Invalid)?;
        if rule.skip_state_update {
            if rule.state != AgentDetectionState::Unknown {
                return Err(ManifestError::Invalid(format!(
                    "rule {:?} uses skipStateUpdate without state unknown",
                    rule.id
                )));
            }
            if rule.visible_idle || rule.visible_blocker || rule.visible_working {
                return Err(ManifestError::Invalid(format!(
                    "rule {:?} combines skipStateUpdate with visible-state evidence",
                    rule.id
                )));
            }
        }
        validate_gate(&rule.gate, 0, &mut matcher_count)?;
    }
    Ok(())
}

fn validate_gate(
    gate: &ManifestGate,
    depth: usize,
    count: &mut usize,
) -> Result<(), ManifestError> {
    if depth > MAX_GATE_DEPTH {
        return Err(ManifestError::Invalid(format!(
            "gate nesting exceeds {MAX_GATE_DEPTH}"
        )));
    }
    let direct = gate.contains.len() + gate.regex.len() + gate.line_regex.len();
    *count += direct;
    if *count > MAX_TOTAL_MATCHERS {
        return Err(ManifestError::Invalid(format!(
            "manifest exceeds {MAX_TOTAL_MATCHERS} matchers"
        )));
    }
    if direct == 0 && gate.all.is_empty() && gate.any.is_empty() {
        return Err(ManifestError::Invalid(
            "every positive gate needs at least one matcher".to_string(),
        ));
    }
    for pattern in gate
        .contains
        .iter()
        .chain(gate.regex.iter())
        .chain(gate.line_regex.iter())
    {
        if pattern.is_empty() || pattern.len() > MAX_PATTERN_BYTES {
            return Err(ManifestError::Invalid(format!(
                "matcher patterns must contain 1-{MAX_PATTERN_BYTES} bytes"
            )));
        }
    }
    for nested in gate.all.iter().chain(gate.any.iter()) {
        validate_gate(nested, depth + 1, count)?;
    }
    for nested in &gate.not_gate {
        if nested.contains.is_empty()
            && nested.regex.is_empty()
            && nested.line_regex.is_empty()
            && nested.all.is_empty()
            && nested.any.is_empty()
            && nested.not_gate.is_empty()
        {
            return Err(ManifestError::Invalid(
                "not gate must not be empty".to_string(),
            ));
        }
        validate_negative_gate(nested, depth + 1, count)?;
    }
    Ok(())
}

fn validate_negative_gate(
    gate: &ManifestGate,
    depth: usize,
    count: &mut usize,
) -> Result<(), ManifestError> {
    if depth > MAX_GATE_DEPTH {
        return Err(ManifestError::Invalid(format!(
            "gate nesting exceeds {MAX_GATE_DEPTH}"
        )));
    }
    let direct = gate.contains.len() + gate.regex.len() + gate.line_regex.len();
    *count += direct;
    if *count > MAX_TOTAL_MATCHERS {
        return Err(ManifestError::Invalid(format!(
            "manifest exceeds {MAX_TOTAL_MATCHERS} matchers"
        )));
    }
    for pattern in gate
        .contains
        .iter()
        .chain(gate.regex.iter())
        .chain(gate.line_regex.iter())
    {
        if pattern.is_empty() || pattern.len() > MAX_PATTERN_BYTES {
            return Err(ManifestError::Invalid(format!(
                "matcher patterns must contain 1-{MAX_PATTERN_BYTES} bytes"
            )));
        }
    }
    for nested in gate
        .all
        .iter()
        .chain(gate.any.iter())
        .chain(gate.not_gate.iter())
    {
        validate_negative_gate(nested, depth + 1, count)?;
    }
    Ok(())
}

fn compile_rule(rule: ManifestRule) -> Result<CompiledRule, ManifestError> {
    let region = Region::parse(&rule.region).map_err(ManifestError::Invalid)?;
    Ok(CompiledRule {
        id: rule.id,
        state: rule.state,
        priority: rule.priority,
        region_label: rule.region,
        region,
        visible_idle: rule.visible_idle,
        visible_blocker: rule.visible_blocker,
        visible_working: rule.visible_working,
        skip_state_update: rule.skip_state_update,
        gate: compile_gate(rule.gate)?,
    })
}

fn compile_gate(gate: ManifestGate) -> Result<CompiledGate, ManifestError> {
    let compile_regexes = |patterns: Vec<String>| {
        patterns
            .into_iter()
            .map(|pattern| {
                Regex::new(&pattern)
                    .map(|regex| (pattern.clone(), regex))
                    .map_err(|error| {
                        ManifestError::Invalid(format!("invalid regex {pattern:?}: {error}"))
                    })
            })
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(CompiledGate {
        contains: gate
            .contains
            .into_iter()
            .map(|pattern| {
                let normalized = pattern.to_lowercase();
                (pattern, normalized)
            })
            .collect(),
        regex: compile_regexes(gate.regex)?,
        line_regex: compile_regexes(gate.line_regex)?,
        all: gate
            .all
            .into_iter()
            .map(compile_gate)
            .collect::<Result<_, _>>()?,
        any: gate
            .any
            .into_iter()
            .map(compile_gate)
            .collect::<Result<_, _>>()?,
        not_gate: gate
            .not_gate
            .into_iter()
            .map(compile_gate)
            .collect::<Result<_, _>>()?,
    })
}

impl CompiledGate {
    fn matches(&self, text: &str, normalized: &str) -> bool {
        let direct_matches = self
            .contains
            .iter()
            .all(|(_, pattern)| normalized.contains(pattern))
            && self.regex.iter().all(|(_, regex)| regex.is_match(text))
            && self
                .line_regex
                .iter()
                .all(|(_, regex)| text.lines().any(|line| regex.is_match(line)));
        direct_matches
            && self.all.iter().all(|gate| gate.matches(text, normalized))
            && (self.any.is_empty() || self.any.iter().any(|gate| gate.matches(text, normalized)))
            && self
                .not_gate
                .iter()
                .all(|gate| !gate.matches(text, normalized))
    }

    fn evaluate(&self, text: &str) -> (bool, RuleEvidence) {
        let normalized = text.to_lowercase();
        let matched = self.matches(text, &normalized);
        let contains = self
            .contains
            .iter()
            .map(|(pattern, normalized_pattern)| MatcherEvidence {
                pattern: pattern.clone(),
                matched: normalized.contains(normalized_pattern),
            })
            .collect();
        let regex = self
            .regex
            .iter()
            .map(|(pattern, regex)| MatcherEvidence {
                pattern: pattern.clone(),
                matched: regex.is_match(text),
            })
            .collect();
        let line_regex = self
            .line_regex
            .iter()
            .map(|(pattern, regex)| MatcherEvidence {
                pattern: pattern.clone(),
                matched: text.lines().any(|line| regex.is_match(line)),
            })
            .collect();
        let all_gate_matches = self
            .all
            .iter()
            .filter(|gate| gate.matches(text, &normalized))
            .count();
        let any_gate_matches = self
            .any
            .iter()
            .filter(|gate| gate.matches(text, &normalized))
            .count();
        let not_gate_matches = self
            .not_gate
            .iter()
            .filter(|gate| gate.matches(text, &normalized))
            .count();
        (
            matched,
            RuleEvidence {
                contains,
                regex,
                line_regex,
                all_gate_count: self.all.len(),
                all_gate_matches,
                any_gate_count: self.any.len(),
                any_gate_matches,
                not_gate_count: self.not_gate.len(),
                not_gate_matches,
                region_bytes: text.len(),
                region_preview: bounded_preview(text),
            },
        )
    }
}

fn missing_manifest_explain(agent: AgentKind) -> DetectionExplain {
    DetectionExplain {
        agent,
        state: AgentDetectionState::Unknown,
        source: bundled_source(),
        confidence: DetectionConfidence::Fallback,
        matched_rule: None,
        screen_detection_skipped: false,
        visible_idle: false,
        visible_blocker: false,
        visible_working: false,
        skip_state_update: false,
        fallback_reason: Some("manifest_missing".to_string()),
        evaluated_rules: Vec::new(),
        warning: Some(format!("no loaded manifest for {agent}")),
        manifest_version: "unknown".to_string(),
    }
}

fn bounded_preview(text: &str) -> String {
    let count = text.chars().count();
    if count <= REGION_PREVIEW_CHARS {
        return text.to_string();
    }
    let start = count - REGION_PREVIEW_CHARS;
    format!("…{}", text.chars().skip(start).collect::<String>())
}

fn bottom_non_empty_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

fn after_last_horizontal_rule(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .rposition(|line| is_horizontal_rule(line))
        .map_or(0, |index| index + 1);
    lines[start..].join("\n")
}

fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().count() >= 4
        && trimmed
            .chars()
            .all(|ch| matches!(ch, '-' | '─' | '━' | '═' | '╌' | '╍' | ' '))
}

fn after_last_prompt_marker(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .rposition(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with('›') || trimmed.starts_with('❯') || trimmed.starts_with('>')
        })
        .unwrap_or(0);
    lines[start..].join("\n")
}

fn prompt_box_body(text: &str) -> String {
    let after_rule = after_last_horizontal_rule(text);
    after_last_prompt_marker(&after_rule)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ManifestRegistry {
        ManifestRegistry::bundled().expect("bundled manifests")
    }

    fn manifest(rules: &str) -> String {
        format!(
            r#"{{
                "schemaVersion": 1,
                "id": "codex",
                "version": "test.1",
                "rules": [{rules}]
            }}"#
        )
    }

    #[test]
    fn all_bundled_manifests_compile() {
        let registry = registry();
        assert_eq!(registry.summaries().len(), AgentKind::ALL.len());
        assert!(registry
            .summaries()
            .iter()
            .all(|summary| summary.warning.is_none()));
    }

    #[test]
    fn bundled_fixtures_cover_every_state_family() {
        let registry = registry();
        assert_eq!(
            registry
                .detect(
                    AgentKind::Claude,
                    DetectionInput::screen("Do you want to proceed?\n1. Yes\nEsc to cancel")
                )
                .state,
            AgentDetectionState::Blocked
        );
        assert_eq!(
            registry
                .detect(
                    AgentKind::Codex,
                    DetectionInput::screen("• Working (12s · esc to interrupt)")
                )
                .state,
            AgentDetectionState::Working
        );
        assert_eq!(
            registry
                .detect(AgentKind::Hermes, DetectionInput::screen("✓ Ready\n> "))
                .state,
            AgentDetectionState::Idle
        );
        assert_eq!(
            registry
                .detect(AgentKind::Pi, DetectionInput::screen("Working..."))
                .state,
            AgentDetectionState::Working
        );
        assert_eq!(
            registry
                .detect(
                    AgentKind::OpenCode,
                    DetectionInput::screen("△ Permission required\n↑↓ select · enter confirm")
                )
                .state,
            AgentDetectionState::Blocked
        );
    }

    #[test]
    fn no_match_is_an_explicit_idle_fallback() {
        let detection = registry().detect(
            AgentKind::Codex,
            DetectionInput::screen("unrecognized live UI"),
        );
        assert_eq!(detection.state, AgentDetectionState::Idle);
        assert_eq!(detection.confidence, DetectionConfidence::Fallback);
        assert_eq!(detection.source, DetectionSource::Fallback);
        assert_eq!(
            detection.fallback_reason.as_deref(),
            Some(DEFAULT_KNOWN_AGENT_IDLE_FALLBACK)
        );
    }

    #[test]
    fn priority_and_nested_gate_semantics_are_deterministic() {
        let json = manifest(
            r#"
                {
                    "id": "low",
                    "state": "idle",
                    "priority": 1,
                    "gate": { "contains": ["match"] }
                },
                {
                    "id": "high",
                    "state": "working",
                    "priority": 20,
                    "gate": {
                        "contains": ["match"],
                        "all": [{ "any": [{ "regex": ["w[io]n"] }, { "contains": ["fallback"] }] }],
                        "not": [{ "contains": ["blocked"] }]
                    }
                }
            "#,
        );
        let loaded = compile_manifest(&json, AgentKind::Codex, bundled_source()).expect("compile");
        let registry = ManifestRegistry {
            manifests: HashMap::from([(AgentKind::Codex, loaded)]),
            override_dir: None,
        };
        assert_eq!(
            registry
                .detect(AgentKind::Codex, DetectionInput::screen("MATCH win"))
                .matched_rule
                .as_deref(),
            Some("high")
        );
        assert_eq!(
            registry
                .detect(
                    AgentKind::Codex,
                    DetectionInput::screen("match win blocked")
                )
                .matched_rule
                .as_deref(),
            Some("low")
        );
    }

    #[test]
    fn line_regex_matches_one_complete_line() {
        let json = manifest(
            r#"{
                "id": "line",
                "state": "blocked",
                "gate": { "lineRegex": ["^exact line$"] }
            }"#,
        );
        let loaded = compile_manifest(&json, AgentKind::Codex, bundled_source()).expect("compile");
        let registry = ManifestRegistry {
            manifests: HashMap::from([(AgentKind::Codex, loaded)]),
            override_dir: None,
        };
        assert_eq!(
            registry
                .detect(
                    AgentKind::Codex,
                    DetectionInput::screen("before\nexact line\nafter")
                )
                .state,
            AgentDetectionState::Blocked
        );
    }

    #[test]
    fn regions_keep_incidental_scrollback_out_of_blocker_rules() {
        let json = manifest(
            r#"{
                "id": "bottom",
                "state": "blocked",
                "region": "bottomNonEmptyLines(2)",
                "gate": { "contains": ["allow command?"] }
            }"#,
        );
        let loaded = compile_manifest(&json, AgentKind::Codex, bundled_source()).expect("compile");
        let registry = ManifestRegistry {
            manifests: HashMap::from([(AgentKind::Codex, loaded)]),
            override_dir: None,
        };
        let screen = "old transcript: Allow command?\nline\ncurrent prompt\nready";
        assert_eq!(
            registry
                .detect(AgentKind::Codex, DetectionInput::screen(screen))
                .state,
            AgentDetectionState::Idle
        );
    }

    #[test]
    fn osc_regions_are_first_class_evidence() {
        let detection = registry().detect(
            AgentKind::Codex,
            DetectionInput {
                recent_screen: "",
                osc_title: "Action Required — Codex",
                osc_progress: "",
            },
        );
        assert_eq!(detection.state, AgentDetectionState::Blocked);
        assert_eq!(detection.evidence.region.as_deref(), Some("oscTitle"));
    }

    #[test]
    fn skip_state_update_is_explicit_and_cannot_claim_visible_evidence() {
        let good = manifest(
            r#"{
                "id": "viewer",
                "state": "unknown",
                "skipStateUpdate": true,
                "gate": { "contains": ["transcript viewer"] }
            }"#,
        );
        let loaded = compile_manifest(&good, AgentKind::Codex, bundled_source()).expect("compile");
        let registry = ManifestRegistry {
            manifests: HashMap::from([(AgentKind::Codex, loaded)]),
            override_dir: None,
        };
        assert!(
            registry
                .detect(
                    AgentKind::Codex,
                    DetectionInput::screen("transcript viewer")
                )
                .skip_state_update
        );

        let bad = manifest(
            r#"{
                "id": "viewer",
                "state": "unknown",
                "skipStateUpdate": true,
                "visibleIdle": true,
                "gate": { "contains": ["transcript viewer"] }
            }"#,
        );
        assert!(compile_manifest(&bad, AgentKind::Codex, bundled_source()).is_err());
    }

    #[test]
    fn strict_schema_rejects_unknown_fields_and_duplicate_ids() {
        let unknown = r#"{
            "schemaVersion": 1,
            "id": "codex",
            "version": "test",
            "surprise": true,
            "rules": [{"id":"one","state":"idle","gate":{"contains":["x"]}}]
        }"#;
        assert!(compile_manifest(unknown, AgentKind::Codex, bundled_source()).is_err());

        let duplicate = manifest(
            r#"
                {"id":"same","state":"idle","gate":{"contains":["x"]}},
                {"id":"same","state":"working","gate":{"contains":["y"]}}
            "#,
        );
        assert!(compile_manifest(&duplicate, AgentKind::Codex, bundled_source()).is_err());
    }

    #[test]
    fn invalid_regex_and_unbounded_regions_are_rejected() {
        let regex = manifest(r#"{"id":"bad","state":"idle","gate":{"regex":["("]}}"#);
        assert!(compile_manifest(&regex, AgentKind::Codex, bundled_source()).is_err());
        let region = manifest(
            r#"{"id":"bad","state":"idle","region":"bottomNonEmptyLines(5000)","gate":{"contains":["x"]}}"#,
        );
        assert!(compile_manifest(&region, AgentKind::Codex, bundled_source()).is_err());
    }

    #[test]
    fn valid_local_override_wins_and_reload_is_atomic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("codex.json");
        fs::write(
            &path,
            manifest(r#"{"id":"override","state":"blocked","gate":{"contains":["local-ready"]}}"#),
        )
        .expect("write override");

        let mut registry = ManifestRegistry::with_override_dir(dir.path()).expect("registry");
        assert_eq!(
            registry
                .detect(AgentKind::Codex, DetectionInput::screen("local-ready"))
                .matched_rule
                .as_deref(),
            Some("override")
        );
        assert_eq!(
            registry.summaries()[1].source.kind,
            ManifestSourceKind::LocalOverride
        );

        fs::write(&path, "not json").expect("break override");
        let report = registry.reload().expect("reload");
        assert!(!report.warnings.is_empty());
        assert_eq!(
            registry.summaries()[1].source.kind,
            ManifestSourceKind::Bundled
        );
    }

    #[test]
    fn invalid_override_falls_back_without_affecting_other_agents() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("claude.json"), "{}").expect("write invalid override");
        let registry = ManifestRegistry::with_override_dir(dir.path()).expect("registry");
        let claude = registry
            .summaries()
            .into_iter()
            .find(|summary| summary.agent == AgentKind::Claude)
            .expect("claude");
        assert_eq!(claude.source.kind, ManifestSourceKind::Bundled);
        assert!(claude.warning.is_some());
        assert_eq!(
            registry
                .detect(
                    AgentKind::Codex,
                    DetectionInput::screen("• Working (1s · esc to interrupt)")
                )
                .state,
            AgentDetectionState::Working
        );
    }

    #[test]
    fn explain_reports_rule_evidence_and_bounds_preview() {
        let screen = format!("{}\nAllow command?", "x".repeat(REGION_PREVIEW_CHARS + 100));
        let explain = registry().explain(AgentKind::Codex, DetectionInput::screen(&screen));
        let blocker = explain
            .evaluated_rules
            .iter()
            .find(|rule| rule.id == "command_approval")
            .expect("blocker rule");
        assert!(blocker.matched);
        assert!(blocker.evidence.region_preview.chars().count() <= REGION_PREVIEW_CHARS + 1);
        assert!(blocker.evidence.any_gate_matches > 0);
    }

    #[test]
    fn dto_serialization_uses_frontend_friendly_names() {
        let value = serde_json::to_value(registry().detect(
            AgentKind::Claude,
            DetectionInput::screen("Working (esc to interrupt)"),
        ))
        .expect("serialize");
        assert_eq!(value["source"], "screen_manifest");
        assert!(value.get("matchedRule").is_some());
        assert!(value["evidence"].get("visibleWorking").is_some());
    }
}
