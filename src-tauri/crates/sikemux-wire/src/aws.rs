//! What the AWS plugin's views hand a pane: an account's costs by month, and its clusters,
//! services, tasks, instances, functions, queues and buckets with what each one's state means.

use serde::{Deserialize, Serialize};

/// An account's costs over the last months, worked out for drawing: each month's charges and
/// credits, and how the month in progress is heading.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Billing {
    /// The currency every amount is in, such as `USD`.
    pub unit: String,
    /// Oldest first.
    pub months: Vec<BillingMonth>,
    /// The place in `months` of the month in progress, else of the newest.
    pub current: usize,
    /// What the current month comes to at the pace it has kept; its charges so far once closed.
    pub forecast: f64,
    /// The place in `months` of the newest closed month.
    pub last_closed: Option<usize>,
    /// How far the forecast is above or below the last closed month's charges, in percent.
    pub change_pct: Option<f64>,
    /// The closed months' charges on average, and that over the tallest month's.
    pub average: Option<f64>,
    pub average_ratio: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BillingMonth {
    /// `YYYY-MM-01`.
    pub period_start: String,
    /// `Oct 26`.
    pub label: String,
    /// `October`.
    pub name: String,
    /// `October 2026`.
    pub long_label: String,
    pub in_progress: bool,
    /// For the month in progress, Cost Explorer's day (UTC) and the month's days.
    pub day: Option<u32>,
    pub days: u32,
    /// Its charges before credits, its credits (negative), and the two together.
    pub gross: f64,
    pub credits: f64,
    pub net: f64,
    /// Its charges over the tallest month's.
    pub ratio: f64,
    /// The month before, which each charge's change is against.
    pub compared_to: Option<String>,
    /// Largest first.
    pub charges: Vec<BillingCharge>,
    /// Largest first.
    pub credit_lines: Vec<BillingCharge>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BillingCharge {
    pub service: String,
    /// The service without its `Amazon` or `AWS` in front.
    pub short_name: String,
    pub amount: f64,
    /// Its amount over the month's largest charge.
    pub ratio: f64,
    pub change: Option<BillingChange>,
}

/// How a service's charge moved from the month before; for the month in progress, at its pace.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BillingChange {
    /// Nothing was charged for it the month before.
    New,
    /// Within a percent either way.
    Flat,
    Up {
        pct: f64,
    },
    Down {
        pct: f64,
    },
}

/// How a resource is doing, as a pane colours it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Health {
    Ok,
    Warn,
    Fail,
    Off,
}

/// The EC2 instances in the profile's region.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ec2 {
    pub region: String,
    pub running: u32,
    pub instances: Vec<Ec2Instance>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ec2Instance {
    pub id: String,
    pub name: Option<String>,
    pub state: Option<String>,
    /// `Running`, `Starting`, `Stopped` and the like.
    pub state_label: String,
    pub health: Health,
    /// Stopped or terminated, so a pane draws it faded.
    pub idle: bool,
    pub instance_type: Option<String>,
    pub private_ip: Option<String>,
    pub public_ip: Option<String>,
    pub launched_at: Option<String>,
    pub console_url: String,
}

/// The Lambda functions in the profile's region.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Lambda {
    pub region: String,
    /// The runtimes the functions are written in, the most used first.
    pub families: Vec<RuntimeFamily>,
    pub functions: Vec<LambdaFunction>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeFamily {
    /// `nodejs`, `python`, `java`, `go`, `dotnet`, `ruby`, `provided` or `other`.
    pub key: String,
    /// `Node`, `Python`, `.NET`, `Custom` and the like.
    pub label: String,
    pub count: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LambdaFunction {
    pub name: String,
    pub runtime: Option<String>,
    /// The key of its runtime's family.
    pub family: String,
    /// Its runtime is one AWS no longer supports.
    pub deprecated: bool,
    pub memory_mb: Option<u32>,
    /// `512 MB`, `1.5 GB`.
    pub memory_label: String,
    /// Its memory over the most any function has, and never under a gigabyte's worth.
    pub memory_ratio: f64,
    pub timeout_secs: Option<u32>,
    pub handler: Option<String>,
    pub modified_at: Option<String>,
    pub log_group: String,
    pub console_url: String,
}

/// The SQS queues in the profile's region.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sqs {
    pub region: String,
    pub dead_letters: u32,
    pub queues: Vec<SqsQueue>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SqsQueue {
    pub name: String,
    pub url: String,
    pub fifo: bool,
    /// Named as a dead-letter queue, `-dlq` or `-dead-letter` at its end.
    pub dead_letter: bool,
    pub available: Option<u64>,
    pub in_flight: Option<u64>,
    pub delayed: Option<u64>,
    /// How full its bar is beside the fullest queue's: on a square-root scale, so small queues
    /// still show, and never under a sliver while it holds anything.
    pub ratio: f64,
    /// A dead-letter queue holding messages, which someone should look at.
    pub needs_look: bool,
    pub console_url: String,
}

/// The account's S3 buckets.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct S3 {
    pub region: String,
    pub buckets: Vec<S3Bucket>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct S3Bucket {
    pub name: String,
    pub created_at: Option<String>,
    /// `s3://name`.
    pub uri: String,
    pub arn: String,
    pub console_url: String,
}

/// The ECS clusters in the profile's region, read as `<product>-<env>-cluster`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EcsClusters {
    pub region: String,
    /// The environments the clusters' names end in, earliest stage first, `other` last.
    pub envs: Vec<EcsEnv>,
    pub clusters: Vec<EcsClusterRow>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EcsEnv {
    /// `prod`, `staging`, or `other` for names that follow no environment.
    pub key: String,
    /// `Prod`, `Staging`, `Other`.
    pub label: String,
    pub count: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EcsClusterRow {
    pub name: String,
    /// Its name without `-cluster` at the end.
    pub short_name: String,
    pub arn: String,
    pub product: String,
    /// The key of its environment.
    pub env: String,
    /// What its environment chip reads.
    pub env_label: String,
    pub production: bool,
    pub services: u32,
    pub running: u32,
    pub pending: u32,
    pub health: Health,
    pub console_url: String,
}

/// A cluster's services.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EcsServices {
    pub region: String,
    pub cluster: String,
    pub services: Vec<EcsServiceRow>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EcsServiceRow {
    pub name: String,
    pub arn: String,
    pub desired: u32,
    pub running: u32,
    pub pending: u32,
    pub health: Health,
    /// `Steady`, `Deploying`, `Degraded` or `Scaled to 0`.
    pub state_label: String,
    /// When its newest deployment last moved.
    pub deployed_at: Option<String>,
    pub console_url: String,
}

/// A service's running tasks.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EcsTasks {
    pub tasks: Vec<EcsTaskRow>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EcsTaskRow {
    pub arn: String,
    pub id: String,
    /// Its id's first twelve characters, which tell tasks apart, and the eight after them.
    pub short_id: String,
    pub id_rest: String,
    pub health: Health,
    /// `Running`, `Pending`, `Stopped`.
    pub status_label: String,
    /// Its health check's answer, when it has one.
    pub check: Option<Health>,
    /// `Healthy`, `Unhealthy`, `Unknown`.
    pub check_label: String,
    pub private_ip: Option<String>,
    pub zone: Option<String>,
    /// `0.5 vCPU · 1 GB`.
    pub size: Option<String>,
    pub started_at: Option<String>,
}

/// The profile agents start on as `AWS_PROFILE`, for one project and for every other, and the
/// profiles there are to pick from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfiles {
    /// What the person picked for the project, and for every other one.
    pub project: Option<String>,
    pub default: Option<String>,
    /// What the project's agents get, all picks and the shell taken together.
    pub effective: Option<String>,
    /// The person's shell sets the profile or keys itself, which Sikemux never overrides: what
    /// a pane says of it instead of offering picks, such as `prod, from your shell`.
    pub from_shell: Option<String>,
    /// What leaving each pick unset reads as, such as `Default (prod)` and `Automatic (prod)`.
    pub project_unset: String,
    pub default_unset: String,
    pub profiles: Vec<String>,
}

/// A line of a log group's live tail.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// When CloudWatch took it, in UTC: `2026-10-23` and `09:30:12`.
    pub day: Option<String>,
    pub clock: Option<String>,
    pub text: String,
    pub level: Option<LogLevel>,
}

/// How a log line reads: an error, such as an `ERROR` or `FATAL` in it or the CLI's own
/// complaint, or a warning.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Error,
    Warn,
}

/// The profile the pane shows, and the profiles there are to switch to.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub profile: Option<String>,
    pub profiles: Vec<AccountProfile>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountProfile {
    pub name: String,
    /// Its name without the account id SSO profiles often end in, such as `admin` for
    /// `admin-123456789012`.
    pub title: String,
    /// `1234-5678-9012`.
    pub account: Option<String>,
    pub region: Option<String>,
}
