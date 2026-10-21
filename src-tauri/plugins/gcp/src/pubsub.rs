// Pub/Sub: topics and their subscriptions, publishing a message, and peeking
// at what waits in a subscription without taking it: pulled messages are
// handed straight back, so nothing is acknowledged or lost.

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::auth::Target;
use crate::client::{self, at, segment, short, text};
use crate::error::{GcpError, GcpResult};

const API: &str = "https://pubsub.googleapis.com/v1";
const MAX_ITEMS: usize = 1000;
const MAX_PEEK: u32 = 50;
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

pub fn topic(value: &Value) -> Value {
    json!({
        "name": short(&text(value, "/name")),
        "retention": at(value, "/messageRetentionDuration"),
        "labels": at(value, "/labels"),
    })
}

pub fn subscription(value: &Value) -> Value {
    let endpoint = text(value, "/pushConfig/pushEndpoint");
    json!({
        "name": short(&text(value, "/name")),
        "topic": short(&text(value, "/topic")),
        "kind": if !endpoint.is_empty() { "push" } else if value.get("bigqueryConfig").is_some() { "bigquery" } else if value.get("cloudStorageConfig").is_some() { "storage" } else { "pull" },
        "pushEndpoint": if endpoint.is_empty() { Value::Null } else { json!(endpoint) },
        "ackDeadlineSeconds": at(value, "/ackDeadlineSeconds"),
        "retention": at(value, "/messageRetentionDuration"),
        "deadLetterTopic": at(value, "/deadLetterPolicy/deadLetterTopic").as_str().map(short),
        "maxDeliveryAttempts": at(value, "/deadLetterPolicy/maxDeliveryAttempts"),
        "filter": at(value, "/filter"),
        "ordering": at(value, "/enableMessageOrdering").as_bool() == Some(true),
        "state": at(value, "/state"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
}

/// Every topic, each with the subscriptions that read it.
pub async fn topics(input: Listing) -> GcpResult<Value> {
    let project = input.target.project()?;
    let (topics, subscriptions) = futures::future::try_join(
        client::list(
            &input.target,
            &format!("{API}/projects/{project}/topics"),
            &[("pageSize", "500".into())],
            "topics",
            MAX_ITEMS,
        ),
        client::list(
            &input.target,
            &format!("{API}/projects/{project}/subscriptions"),
            &[("pageSize", "500".into())],
            "subscriptions",
            MAX_ITEMS,
        ),
    )
    .await?;
    let mut topics: Vec<Value> = topics.iter().map(topic).collect();
    topics.sort_by_key(|each| text(each, "/name"));
    let mut subscriptions: Vec<Value> = subscriptions.iter().map(subscription).collect();
    subscriptions.sort_by_key(|each| text(each, "/name"));
    Ok(json!({ "topics": topics, "subscriptions": subscriptions }))
}

#[derive(Deserialize)]
pub struct Publish {
    #[serde(flatten)]
    pub target: Target,
    pub topic: String,
    pub data: String,
    #[serde(default)]
    pub attributes: Map<String, Value>,
}

/// Publishes one message to a topic, answering with its id.
pub async fn publish(input: Publish) -> GcpResult<Value> {
    if input.data.len() > MAX_MESSAGE_BYTES {
        return Err(GcpError::BadArg("a message here is at most 1 MiB".into()));
    }
    let project = input.target.project()?;
    let url = format!(
        "{API}/projects/{project}/topics/{}:publish",
        segment("topic", &input.topic)?
    );
    let attributes: Map<String, Value> = input
        .attributes
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                json!(value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string())),
            )
        })
        .collect();
    let body = json!({ "messages": [{ "data": base64::engine::general_purpose::STANDARD.encode(input.data.as_bytes()), "attributes": attributes }] });
    let published = client::post(&input.target, &url, &body).await?;
    Ok(json!({ "messageId": at(&published, "/messageIds/0") }))
}

/// A pulled message as the pane and agents read it: its text, or its bytes in base64 when they are not text.
pub fn message(received: &Value) -> Value {
    let message = at(received, "/message");
    let raw = base64::engine::general_purpose::STANDARD
        .decode(text(message, "/data"))
        .unwrap_or_default();
    let data = match String::from_utf8(raw) {
        Ok(text) => json!({ "text": text }),
        Err(error) => {
            json!({ "base64": base64::engine::general_purpose::STANDARD.encode(error.into_bytes()) })
        }
    };
    json!({
        "id": at(message, "/messageId"),
        "publishedAt": at(message, "/publishTime"),
        "data": data,
        "attributes": at(message, "/attributes"),
        "orderingKey": at(message, "/orderingKey"),
        "deliveryAttempt": at(received, "/deliveryAttempt"),
    })
}

#[derive(Deserialize)]
pub struct Peek {
    #[serde(flatten)]
    pub target: Target,
    pub subscription: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Shows messages waiting in a subscription and gives them straight back, so its readers still get every one.
pub async fn peek(input: Peek) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let base = format!(
        "{API}/projects/{project}/subscriptions/{}",
        segment("subscription", &input.subscription)?
    );
    let pulled = client::post(
        &input.target,
        &format!("{base}:pull"),
        &json!({ "maxMessages": input.limit.unwrap_or(10).clamp(1, MAX_PEEK), "returnImmediately": true }),
    )
    .await?;
    let received = at(&pulled, "/receivedMessages")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let ack_ids: Vec<String> = received
        .iter()
        .map(|each| text(each, "/ackId"))
        .filter(|id| !id.is_empty())
        .collect();
    if !ack_ids.is_empty() {
        client::post(
            &input.target,
            &format!("{base}:modifyAckDeadline"),
            &json!({ "ackIds": ack_ids, "ackDeadlineSeconds": 0 }),
        )
        .await?;
    }
    Ok(received.iter().map(message).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscriptions_say_how_they_deliver() {
        let push = subscription(
            &json!({ "name": "projects/acme/subscriptions/orders-push", "topic": "projects/acme/topics/orders", "pushConfig": { "pushEndpoint": "https://api.acme.dev/hooks" }, "deadLetterPolicy": { "deadLetterTopic": "projects/acme/topics/orders-dead", "maxDeliveryAttempts": 5 } }),
        );
        assert_eq!(push["kind"], "push");
        assert_eq!(push["topic"], "orders");
        assert_eq!(push["deadLetterTopic"], "orders-dead");
        assert_eq!(
            subscription(&json!({ "name": "x", "pushConfig": {} }))["kind"],
            "pull"
        );
    }

    #[test]
    fn a_message_reads_as_text_or_base64() {
        let text_message = message(
            &json!({ "ackId": "a", "message": { "messageId": "1", "data": "eyJvcmRlciI6IDQyfQ==", "attributes": { "kind": "created" } }, "deliveryAttempt": 2 }),
        );
        assert_eq!(text_message["data"]["text"], "{\"order\": 42}");
        assert_eq!(text_message["deliveryAttempt"], 2);
        let binary = message(&json!({ "message": { "data": "/w==" } }));
        assert_eq!(binary["data"]["base64"], "/w==");
    }
}
