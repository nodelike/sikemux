use serde::{Deserialize, Serialize};

use crate::error::AwsResult;

use crate::common::{aws_json, Target};

#[derive(Serialize, Clone)]
pub struct S3Bucket {
    name: String,
    created_at: Option<String>,
}

pub(crate) async fn buckets(target: &Target) -> AwsResult<Vec<S3Bucket>> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(rename = "Buckets")]
        buckets: Vec<B>,
    }
    #[derive(Deserialize)]
    struct B {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "CreationDate")]
        created: Option<String>,
    }
    let resp: Resp = aws_json(target, &["s3api", "list-buckets", "--output", "json"]).await?;
    let mut out: Vec<S3Bucket> = resp
        .buckets
        .into_iter()
        .map(|b| S3Bucket {
            name: b.name,
            created_at: b.created,
        })
        .collect();
    out.sort_by_key(|bucket| bucket.name.to_lowercase());
    Ok(out)
}
