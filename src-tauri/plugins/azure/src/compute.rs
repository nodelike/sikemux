// Virtual machines: how each is doing, its size, disks and addresses, and
// starting, stopping and restarting one when the person asks.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client::{self, at, maybe, short, text};
use crate::error::{AzureError, AzureResult};

const COMPUTE_API: &str = "2024-11-01";
const NETWORK_API: &str = "2024-05-01";

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub group: String,
    pub location: String,
    /// running, deallocated, stopped or starting, as the portal shows it.
    pub power: String,
    pub size: String,
    pub os: String,
    /// The image it was made from, such as Ubuntu 24.04 LTS.
    pub image: Option<String>,
    pub private_ip: Option<String>,
    pub public_ip: Option<String>,
    pub zones: Vec<String>,
    pub tags: Vec<(String, String)>,
}

/// `PowerState/running` and `VM running` both read as `running`.
pub fn power_of(view: &Value) -> String {
    at(view, "/statuses")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|status| maybe(status, "/code"))
        .find_map(|code| code.strip_prefix("PowerState/").map(str::to_string))
        .or_else(|| {
            maybe(view, "/powerState/displayStatus")
                .map(|state| state.trim_start_matches("VM ").to_string())
        })
        .unwrap_or_else(|| "unknown".into())
}

fn image_of(machine: &Value) -> Option<String> {
    let reference = at(machine, "/properties/storageProfile/imageReference");
    let offer = maybe(reference, "/offer");
    let sku = maybe(reference, "/sku");
    match (offer, sku) {
        (Some(offer), Some(sku)) => Some(format!("{offer} {sku}")),
        (Some(offer), None) => Some(offer),
        _ => maybe(reference, "/id").map(|id| short(&id)),
    }
}

pub fn machine_of(machine: &Value) -> Option<Machine> {
    let id = maybe(machine, "/id")?;
    Some(Machine {
        name: maybe(machine, "/name").unwrap_or_else(|| short(&id)),
        group: client::group_of(&id),
        location: text(machine, "/location"),
        power: power_of(at(machine, "/properties/instanceView")),
        size: text(machine, "/properties/hardwareProfile/vmSize"),
        os: maybe(machine, "/properties/storageProfile/osDisk/osType").unwrap_or_default(),
        image: image_of(machine),
        private_ip: None,
        public_ip: None,
        zones: at(machine, "/zones")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|zone| zone.as_str().map(str::to_string))
            .collect(),
        tags: at(machine, "/tags")
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
            .collect(),
        id,
    })
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ListMachines {
    pub subscription: Option<String>,
    pub group: Option<String>,
}

async fn subscription_of(named: Option<&str>) -> AzureResult<String> {
    if let Some(named) = named.filter(|named| !named.is_empty()) {
        return client::segment("subscription", named);
    }
    crate::auth::status()
        .await
        .current
        .map(|subscription| subscription.id)
        .ok_or(AzureError::Unconfigured)
}

/// Every machine in the subscription, or in one group, with how each is doing.
pub async fn list(input: ListMachines) -> AzureResult<Vec<Machine>> {
    let subscription = subscription_of(input.subscription.as_deref()).await?;
    let path = match input.group.as_deref().filter(|group| !group.is_empty()) {
        Some(group) => format!(
            "/subscriptions/{subscription}/resourceGroups/{}/providers/Microsoft.Compute/virtualMachines",
            client::segment("resource group", group)?
        ),
        None => format!("/subscriptions/{subscription}/providers/Microsoft.Compute/virtualMachines"),
    };
    let listed = client::list(&path, COMPUTE_API, &[("statusOnly", "false".into())]).await?;
    let mut machines: Vec<Machine> = listed.iter().filter_map(machine_of).collect();
    if machines.iter().all(|machine| machine.power == "unknown") {
        let views = client::list(
            &format!("/subscriptions/{subscription}/providers/Microsoft.Compute/virtualMachines"),
            COMPUTE_API,
            &[("statusOnly", "true".into())],
        )
        .await
        .unwrap_or_default();
        for machine in &mut machines {
            if let Some(view) = views
                .iter()
                .find(|view| maybe(view, "/id").as_deref() == Some(&machine.id))
            {
                machine.power = power_of(at(view, "/properties/instanceView"));
            }
        }
    }
    machines.sort_by(|a, b| (&a.group, &a.name).cmp(&(&b.group, &b.name)));
    Ok(machines)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineRef {
    #[serde(default)]
    pub subscription: Option<String>,
    pub group: String,
    pub name: String,
}

impl MachineRef {
    async fn path(&self) -> AzureResult<String> {
        let subscription = subscription_of(self.subscription.as_deref()).await?;
        Ok(format!(
            "/subscriptions/{subscription}/resourceGroups/{}/providers/Microsoft.Compute/virtualMachines/{}",
            client::segment("resource group", &self.group)?,
            client::segment("virtual machine", &self.name)?
        ))
    }
}

/// One machine with its instance view, and the addresses its network interfaces carry.
pub async fn machine(input: MachineRef) -> AzureResult<Machine> {
    let path = input.path().await?;
    let found = client::get(&format!("{path}?$expand=instanceView"), COMPUTE_API).await;
    let found = match found {
        Ok(found) => found,
        Err(_) => client::get(&path, COMPUTE_API).await?,
    };
    let mut machine = machine_of(&found)
        .ok_or_else(|| AzureError::Response("Azure sent a machine without an id".into()))?;
    if machine.power == "unknown" {
        if let Ok(view) = client::get(&format!("{path}/instanceView"), COMPUTE_API).await {
            machine.power = power_of(&view);
        }
    }
    let interfaces: Vec<String> = at(&found, "/properties/networkProfile/networkInterfaces")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|interface| maybe(interface, "/id"))
        .collect();
    for id in interfaces.iter().take(2) {
        let Ok(interface) = client::get(id, NETWORK_API).await else {
            continue;
        };
        for configuration in at(&interface, "/properties/ipConfigurations")
            .as_array()
            .into_iter()
            .flatten()
        {
            if machine.private_ip.is_none() {
                machine.private_ip = maybe(configuration, "/properties/privateIPAddress");
            }
            if machine.public_ip.is_none() {
                if let Some(public) = maybe(configuration, "/properties/publicIPAddress/id") {
                    machine.public_ip = client::get(&public, NETWORK_API)
                        .await
                        .ok()
                        .and_then(|address| maybe(&address, "/properties/ipAddress"));
                }
            }
        }
    }
    Ok(machine)
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Start,
    /// Stop the machine and stop paying for its compute.
    Deallocate,
    /// Stop the operating system but keep the machine allocated.
    PowerOff,
    Restart,
}

#[derive(Deserialize)]
pub struct Act {
    #[serde(flatten)]
    pub machine: MachineRef,
    pub action: Action,
}

/// What each action asks Azure for, and what it did in words.
pub fn action_path(action: Action) -> (&'static str, &'static str) {
    match action {
        Action::Start => ("start", "starting"),
        Action::Deallocate => ("deallocate", "stopping and deallocating"),
        Action::PowerOff => ("powerOff", "stopping"),
        Action::Restart => ("restart", "restarting"),
    }
}

#[derive(Serialize)]
pub struct Done {
    pub message: String,
}

pub async fn act(input: Act) -> AzureResult<Done> {
    let (verb, saying) = action_path(input.action);
    let name = input.machine.name.clone();
    let path = input.machine.path().await?;
    client::post(&format!("{path}/{verb}"), COMPUTE_API, &json!({})).await?;
    Ok(Done {
        message: format!("{name} is {saying}"),
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn machine_json() -> Value {
        json!({
            "id": "/subscriptions/s1/resourceGroups/nine-mills/providers/Microsoft.Compute/virtualMachines/nine-mills",
            "name": "nine-mills", "location": "uaenorth", "zones": ["1"],
            "tags": { "env": "prod" },
            "properties": {
                "hardwareProfile": { "vmSize": "Standard_B1s" },
                "storageProfile": {
                    "osDisk": { "osType": "Linux" },
                    "imageReference": { "offer": "ubuntu-24_04-lts", "sku": "server" }
                },
                "instanceView": { "statuses": [
                    { "code": "ProvisioningState/succeeded" },
                    { "code": "PowerState/running", "displayStatus": "VM running" }
                ] }
            }
        })
    }

    #[test]
    fn a_machine_reads_its_power_size_and_image() {
        let machine = machine_of(&machine_json()).expect("reads");
        assert_eq!(machine.power, "running");
        assert_eq!(machine.size, "Standard_B1s");
        assert_eq!(machine.os, "Linux");
        assert_eq!(machine.image.as_deref(), Some("ubuntu-24_04-lts server"));
        assert_eq!(machine.group, "nine-mills");
        assert_eq!(machine.zones, ["1"]);
    }

    #[test]
    fn power_is_read_from_either_shape_azure_sends() {
        assert_eq!(
            power_of(&json!({ "statuses": [{ "code": "PowerState/deallocated" }] })),
            "deallocated"
        );
        assert_eq!(
            power_of(&json!({ "powerState": { "displayStatus": "VM stopped" } })),
            "stopped"
        );
        assert_eq!(power_of(&json!({})), "unknown");
    }

    #[test]
    fn each_action_asks_azure_for_just_that() {
        assert_eq!(action_path(Action::Start), ("start", "starting"));
        assert_eq!(action_path(Action::Deallocate).0, "deallocate");
        assert_eq!(action_path(Action::PowerOff).0, "powerOff");
        assert_eq!(action_path(Action::Restart).0, "restart");
    }

    #[tokio::test]
    async fn a_machine_name_cannot_reach_another_path() {
        let machine = MachineRef {
            subscription: Some("s1".into()),
            group: "g".into(),
            name: "../../other".into(),
        };
        assert!(machine.path().await.is_err());
    }
}
