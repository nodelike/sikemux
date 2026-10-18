use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use crate::error::AwsResult;

use crate::common::{aws_json, Target};

// Counts need one CLI call per queue, so a handful run at once.
const ATTRIBUTE_CONCURRENCY: usize = 6;

#[derive(Serialize, Clone)]
pub struct SqsQueue {
    name: String,
    url: String,
    messages: Option<String>,
    in_flight: Option<String>,
    delayed: Option<String>,
}

#[derive(Deserialize, Default)]
struct Counts {
    #[serde(rename = "ApproximateNumberOfMessages")]
    messages: Option<String>,
    #[serde(rename = "ApproximateNumberOfMessagesNotVisible")]
    in_flight: Option<String>,
    #[serde(rename = "ApproximateNumberOfMessagesDelayed")]
    delayed: Option<String>,
}

async fn counts(target: &Target, url: &str) -> Counts {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(rename = "Attributes", default)]
        attributes: Counts,
    }
    aws_json::<Resp>(
        target,
        &[
            "sqs",
            "get-queue-attributes",
            "--queue-url",
            url,
            "--attribute-names",
            "ApproximateNumberOfMessages",
            "ApproximateNumberOfMessagesNotVisible",
            "ApproximateNumberOfMessagesDelayed",
            "--output",
            "json",
        ],
    )
    .await
    .map(|resp| resp.attributes)
    .unwrap_or_default()
}

pub(crate) async fn queues(target: &Target) -> AwsResult<Vec<SqsQueue>> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default, rename = "QueueUrls")]
        urls: Vec<String>,
    }
    let resp: Resp = aws_json(target, &["sqs", "list-queues", "--output", "json"])
        .await
        .unwrap_or(Resp { urls: vec![] });

    let mut out: Vec<SqsQueue> = stream::iter(resp.urls)
        .map(|url| async move {
            let counts = counts(target, &url).await;
            SqsQueue {
                name: url.rsplit('/').next().unwrap_or(&url).to_string(),
                url,
                messages: counts.messages,
                in_flight: counts.in_flight,
                delayed: counts.delayed,
            }
        })
        .buffered(ATTRIBUTE_CONCURRENCY)
        .collect()
        .await;
    out.sort_by_key(|queue| queue.name.to_lowercase());
    Ok(out)
}
