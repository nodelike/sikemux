use serde::{Deserialize, Serialize};

use crate::error::AwsResult;

use crate::common::{aws_json, Target};

#[derive(Serialize, Clone)]
pub struct Ec2Instance {
    pub(crate) instance_id: String,
    pub(crate) name: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) instance_type: Option<String>,
    pub(crate) private_ip: Option<String>,
    pub(crate) public_ip: Option<String>,
    pub(crate) launch_time: Option<String>,
}

impl Ec2Instance {
    /// Whether its name, id or an address contains `text`, which is lowercase.
    pub(crate) fn matches(&self, text: &str) -> bool {
        [
            self.name.as_deref(),
            Some(self.instance_id.as_str()),
            self.private_ip.as_deref(),
            self.public_ip.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|field| field.to_lowercase().contains(text))
    }
}

pub(crate) async fn instances(target: &Target) -> AwsResult<Vec<Ec2Instance>> {
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
        #[serde(rename = "Tags")]
        tags: Option<Vec<Tag>>,
    }
    #[derive(Deserialize)]
    struct InstState {
        #[serde(rename = "Name")]
        name: Option<String>,
    }
    #[derive(Deserialize)]
    struct Tag {
        #[serde(rename = "Key")]
        key: String,
        #[serde(rename = "Value")]
        value: String,
    }

    let resp: Resp = aws_json(target, &["ec2", "describe-instances", "--output", "json"]).await?;
    let mut out = Vec::new();
    for r in resp.reservations {
        for i in r.instances {
            let name = i
                .tags
                .as_ref()
                .and_then(|tags| tags.iter().find(|t| t.key == "Name"))
                .map(|t| t.value.clone());
            out.push(Ec2Instance {
                name,
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
