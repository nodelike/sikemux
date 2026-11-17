use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::AwsResult;

use crate::common::{aws_json, Target};

/// What EC2 narrows a listing by, as `(filter name, value)`.
pub(crate) type Filters = Vec<(String, String)>;

/// `aws ec2 <command>`, narrowed by `filters`.
async fn describe<T: DeserializeOwned>(
    target: &Target,
    command: &str,
    filters: &Filters,
) -> AwsResult<T> {
    let mut args = vec!["ec2", command, "--output", "json"];
    let named = json!(filters
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Values": [value] }))
        .collect::<Vec<_>>())
    .to_string();
    if !filters.is_empty() {
        args.extend(["--filters", &named]);
    }
    aws_json(target, &args).await
}

#[derive(Deserialize)]
struct Tag {
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "Value")]
    value: String,
}

fn name_of(tags: Option<&[Tag]>) -> Option<String> {
    tags?
        .iter()
        .find(|tag| tag.key == "Name")
        .map(|tag| tag.value.clone())
}

#[derive(Serialize, Clone)]
pub struct Ec2Instance {
    pub(crate) instance_id: String,
    pub(crate) name: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) instance_type: Option<String>,
    pub(crate) private_ip: Option<String>,
    pub(crate) public_ip: Option<String>,
    pub(crate) launch_time: Option<String>,
    pub(crate) zone: Option<String>,
    pub(crate) vpc: Option<String>,
}

impl Ec2Instance {
    /// Whether `wanted` holds of its name, id or an address, lowercased.
    pub(crate) fn matches(&self, wanted: impl Fn(&str) -> bool) -> bool {
        [
            self.name.as_deref(),
            Some(self.instance_id.as_str()),
            self.private_ip.as_deref(),
            self.public_ip.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|field| wanted(&field.to_lowercase()))
    }
}

pub(crate) async fn instances(target: &Target, filters: &Filters) -> AwsResult<Vec<Ec2Instance>> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(rename = "Reservations")]
        reservations: Vec<Reservation>,
    }
    #[derive(Deserialize)]
    struct Reservation {
        #[serde(rename = "Instances")]
        instances: Vec<Inst>,
    }
    #[derive(Deserialize)]
    struct Inst {
        #[serde(rename = "InstanceId")]
        instance_id: String,
        #[serde(rename = "InstanceType")]
        instance_type: Option<String>,
        #[serde(rename = "State")]
        state: Option<InstState>,
        #[serde(rename = "PrivateIpAddress")]
        private_ip: Option<String>,
        #[serde(rename = "PublicIpAddress")]
        public_ip: Option<String>,
        #[serde(rename = "LaunchTime")]
        launch_time: Option<String>,
        #[serde(rename = "Placement")]
        placement: Option<Placement>,
        #[serde(rename = "VpcId")]
        vpc: Option<String>,
        #[serde(rename = "Tags")]
        tags: Option<Vec<Tag>>,
    }
    #[derive(Deserialize)]
    struct Placement {
        #[serde(rename = "AvailabilityZone")]
        zone: Option<String>,
    }
    #[derive(Deserialize)]
    struct InstState {
        #[serde(rename = "Name")]
        name: Option<String>,
    }
    let resp: Resp = describe(target, "describe-instances", filters).await?;
    let mut out = Vec::new();
    for r in resp.reservations {
        for i in r.instances {
            out.push(Ec2Instance {
                name: name_of(i.tags.as_deref()),
                zone: i.placement.and_then(|placement| placement.zone),
                vpc: i.vpc,
                state: i.state.and_then(|s| s.name),
                instance_type: i.instance_type,
                private_ip: i.private_ip,
                public_ip: i.public_ip,
                launch_time: i.launch_time,
                instance_id: i.instance_id,
            });
        }
    }
    out.sort_by(|a, b| {
        let na = a.name.as_deref().unwrap_or("").to_lowercase();
        let nb = b.name.as_deref().unwrap_or("").to_lowercase();
        na.cmp(&nb).then(a.instance_id.cmp(&b.instance_id))
    });
    Ok(out)
}

#[derive(Serialize)]
pub struct Volume {
    id: String,
    name: Option<String>,
    size_gib: Option<u64>,
    kind: Option<String>,
    state: Option<String>,
    iops: Option<u64>,
    encrypted: Option<bool>,
    zone: Option<String>,
    attached_to: Vec<String>,
    created: Option<String>,
}

impl Volume {
    /// Whether `wanted` holds of its name, id or an instance it is attached to, lowercased.
    pub(crate) fn matches(&self, wanted: impl Fn(&str) -> bool) -> bool {
        [self.name.as_deref(), Some(self.id.as_str())]
            .into_iter()
            .flatten()
            .chain(self.attached_to.iter().map(String::as_str))
            .any(|field| wanted(&field.to_lowercase()))
    }
}

/// EBS volumes, largest first, with the instances each is attached to.
pub(crate) async fn volumes(target: &Target, filters: &Filters) -> AwsResult<Vec<Volume>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Resp {
        volumes: Vec<Vol>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Vol {
        volume_id: String,
        size: Option<u64>,
        volume_type: Option<String>,
        state: Option<String>,
        iops: Option<u64>,
        encrypted: Option<bool>,
        availability_zone: Option<String>,
        create_time: Option<String>,
        #[serde(default)]
        attachments: Vec<Attachment>,
        tags: Option<Vec<Tag>>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Attachment {
        instance_id: Option<String>,
    }

    let resp: Resp = describe(target, "describe-volumes", filters).await?;
    let mut out: Vec<Volume> = resp
        .volumes
        .into_iter()
        .map(|v| Volume {
            name: name_of(v.tags.as_deref()),
            id: v.volume_id,
            size_gib: v.size,
            kind: v.volume_type,
            state: v.state,
            iops: v.iops,
            encrypted: v.encrypted,
            zone: v.availability_zone,
            attached_to: v
                .attachments
                .into_iter()
                .filter_map(|attachment| attachment.instance_id)
                .collect(),
            created: v.create_time,
        })
        .collect();
    out.sort_by(|a, b| b.size_gib.cmp(&a.size_gib).then(a.id.cmp(&b.id)));
    Ok(out)
}
