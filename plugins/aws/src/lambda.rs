use serde::{Deserialize, Serialize};

use crate::error::AwsResult;

use crate::common::{aws_json, Target};

#[derive(Serialize, Clone)]
pub struct LambdaFn {
    pub(crate) name: String,
    pub(crate) runtime: Option<String>,
    pub(crate) last_modified: Option<String>,
    pub(crate) memory_size: Option<i64>,
    pub(crate) timeout: Option<i64>,
    pub(crate) handler: Option<String>,
}

pub(crate) async fn functions(target: &Target) -> AwsResult<Vec<LambdaFn>> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(rename = "Functions")]
        functions: Vec<Fn0>,
    }
    #[derive(Deserialize)]
    struct Fn0 {
        #[serde(rename = "FunctionName")]
        name: String,
        #[serde(rename = "Runtime")]
        runtime: Option<String>,
        #[serde(rename = "LastModified")]
        last_modified: Option<String>,
        #[serde(rename = "MemorySize")]
        memory_size: Option<i64>,
        #[serde(rename = "Timeout")]
        timeout: Option<i64>,
        #[serde(rename = "Handler")]
        handler: Option<String>,
    }
    let resp: Resp = aws_json(target, &["lambda", "list-functions", "--output", "json"]).await?;
    let mut out: Vec<LambdaFn> = resp
        .functions
        .into_iter()
        .map(|f| LambdaFn {
            name: f.name,
            runtime: f.runtime,
            last_modified: f.last_modified,
            memory_size: f.memory_size,
            timeout: f.timeout,
            handler: f.handler,
        })
        .collect();
    out.sort_by_key(|function| function.name.to_lowercase());
    Ok(out)
}
