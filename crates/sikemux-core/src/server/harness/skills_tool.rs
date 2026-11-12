//! The `skills` tool.

use super::*;

pub(super) const SKILL_LOADS: &str =
    "Start a new agent session to load it. Claude lists it as /sikemux:<name>.";

pub(super) async fn skills_call(core: &Core, request: &HarnessRequest) -> Result<Value, String> {
    let data_dir = core
        .config()
        .data_dir
        .clone()
        .ok_or("Skills need Sikemux's data folder, and this core was started without one")?;
    let params = request.params.clone();
    blocking(move || Ok(skills_answer(&data_dir, &params)))
        .await
        .map_err(|error| error.to_string())?
}

pub(super) fn skills_answer(data_dir: &Path, params: &Value) -> Result<Value, String> {
    match required_text(params, "action")? {
        "list" => Ok(json!({
            "library": skills::library(data_dir),
            "skills": skills::list(data_dir),
        })),
        "create" => {
            let body = match params.get("body") {
                None | Some(Value::Null) => "",
                Some(Value::String(body)) => body.as_str(),
                Some(_) => return Err("body must be text".into()),
            };
            let skill = skills::create(
                data_dir,
                required_text(params, "name")?,
                required_text(params, "description")?,
                body,
            )?;
            Ok(json!({ "skill": skill, "note": SKILL_LOADS }))
        }
        "install" => {
            let source = required_text(params, "source")?;
            let skill = if source.starts_with('/') {
                skills::install_folder(data_dir, Path::new(source))?
            } else {
                skills::install_git(data_dir, source, text(params, "subfolder")?)?
            };
            Ok(json!({ "skill": skill, "note": SKILL_LOADS }))
        }
        "remove" => {
            let name = required_text(params, "name")?;
            skills::remove(data_dir, name)?;
            Ok(json!({ "removed": name }))
        }
        other => Err(format!(
            "action must be list, create, install or remove, not {other}"
        )),
    }
}
