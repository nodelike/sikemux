//! ConfigMaps and Secrets by the names of their keys. kubectl is asked through a
//! template that prints only key names and sizes, so no value ever reaches Sikemux.

use serde::{Deserialize, Serialize};

use crate::error::KubeResult;
use crate::kubectl::{self, Scope};

/// One line per object: namespace, name, type, then each key with its size.
const TEMPLATE: &str = r#"--template={{range .items}}{{.metadata.namespace}}	{{.metadata.name}}	{{if .type}}{{.type}}{{else}}-{{end}}{{range $key, $value := .data}}	{{$key}}={{len $value}}{{end}}{{range $key, $value := .binaryData}}	{{$key}}={{len $value}}{{end}}
{{end}}"#;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    pub name: String,
    pub namespace: String,
    /// For a Secret, its type such as Opaque or kubernetes.io/tls; none for a ConfigMap.
    pub kind: Option<String>,
    pub keys: Vec<Key>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Key {
    pub name: String,
    /// The value's length as stored; a Secret's is its base64 text.
    pub size: usize,
}

pub fn stores_of(output: &str) -> Vec<Store> {
    let mut stores: Vec<Store> = output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let namespace = fields.next()?.to_string();
            let name = fields.next()?.to_string();
            let kind = fields
                .next()
                .filter(|kind| *kind != "-")
                .map(str::to_string);
            let keys = fields
                .filter_map(|field| {
                    let (key, size) = field.rsplit_once('=')?;
                    Some(Key {
                        name: key.to_string(),
                        size: size.parse().unwrap_or(0),
                    })
                })
                .collect();
            (!name.is_empty()).then_some(Store {
                name,
                namespace,
                kind,
                keys,
            })
        })
        .collect();
    stores.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    stores
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub config_maps: Vec<Store>,
    pub secrets: Vec<Store>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ListConfig {
    #[serde(flatten)]
    pub scope: Scope,
    pub all_namespaces: bool,
    /// Leave out the service account tokens and root certificates every namespace has.
    pub include_system: bool,
}

fn system(store: &Store) -> bool {
    store.name == "kube-root-ca.crt"
        || store.kind.as_deref() == Some("kubernetes.io/service-account-token")
        || store
            .kind
            .as_deref()
            .is_some_and(|kind| kind.starts_with("helm.sh/"))
}

pub async fn list(input: ListConfig) -> KubeResult<Config> {
    let mut maps = vec!["get", "configmaps", "--output=go-template", TEMPLATE];
    let mut secrets = vec!["get", "secrets", "--output=go-template", TEMPLATE];
    if input.all_namespaces {
        maps.push("--all-namespaces");
        secrets.push("--all-namespaces");
    }
    let (maps, secrets) = tokio::join!(
        kubectl::run(&input.scope, &maps),
        kubectl::run(&input.scope, &secrets)
    );
    let keep = |stores: Vec<Store>| -> Vec<Store> {
        stores
            .into_iter()
            .filter(|store| input.include_system || !system(store))
            .collect()
    };
    Ok(Config {
        config_maps: keep(stores_of(&maps?)),
        secrets: keep(stores_of(&secrets?)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_read_their_key_names_and_sizes() {
        let output = "shop\tweb-secrets\tOpaque\tDATABASE_PASSWORD=16\tSTRIPE_KEY=28\nshop\tweb-config\t-\tAPI_URL=21\tLOG_LEVEL=4\n";
        let stores = stores_of(output);
        assert_eq!(stores[0].name, "web-config");
        assert_eq!(stores[0].kind, None);
        assert_eq!(stores[1].kind.as_deref(), Some("Opaque"));
        assert_eq!(
            stores[1].keys,
            [
                Key {
                    name: "DATABASE_PASSWORD".into(),
                    size: 16
                },
                Key {
                    name: "STRIPE_KEY".into(),
                    size: 28
                }
            ]
        );
    }

    #[test]
    fn the_template_asks_for_key_names_and_lengths_only() {
        assert!(TEMPLATE.contains("{{$key}}={{len $value}}"));
        assert!(!TEMPLATE.contains("{{$value}}"));
        assert!(system(&Store {
            name: "kube-root-ca.crt".into(),
            namespace: "shop".into(),
            kind: None,
            keys: vec![]
        }));
        assert!(system(&Store {
            name: "sh.helm.release.v1.web.v3".into(),
            namespace: "shop".into(),
            kind: Some("helm.sh/release.v1".into()),
            keys: vec![]
        }));
    }
}
