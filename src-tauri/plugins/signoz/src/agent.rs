//! Agents reach several of the pane's methods through one tool, so fewer
//! schemas ride on every request. What the agent passes picks the method.

use serde_json::Value;
use sikemux_plugin_api::PluginError;

pub fn route(method: &str, mut input: Value) -> Result<(&str, Value), PluginError> {
    let given = |key: &str| input.get(key).is_some_and(|value| !value.is_null());
    let routed = match method {
        "agentServices" if !given("service") => "services",
        "agentServices" => match input.get("show").and_then(Value::as_str) {
            Some("endpoints") => "operations",
            Some("errors") => "errorGroups",
            _ => "serviceOverview",
        },
        "agentTraces" if given("traceId") => "trace",
        "agentTraces" => "searchTraces",
        "agentDashboards" if given("panelId") => {
            let Some(id) = input.get("id").filter(|id| !id.is_null()).cloned() else {
                return Err(PluginError::new(
                    "bad-params",
                    "panelId needs the dashboard's id as well",
                ));
            };
            if let Some(fields) = input.as_object_mut() {
                fields.insert("dashboardId".into(), id);
            }
            "dashboardPanel"
        }
        "agentDashboards" if given("id") => "dashboard",
        "agentDashboards" => "dashboards",
        "agentFields" if given("name") => "fieldValues",
        "agentFields" => "fieldKeys",
        other => other,
    };
    Ok((routed, input))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn method(tool: &str, input: Value) -> String {
        route(tool, input).expect("routes").0.to_owned()
    }

    #[test]
    fn what_an_agent_passes_picks_the_method() {
        assert_eq!(method("agentServices", json!({})), "services");
        assert_eq!(
            method("agentServices", json!({ "service": "api" })),
            "serviceOverview"
        );
        assert_eq!(
            method(
                "agentServices",
                json!({ "service": "api", "show": "errors" })
            ),
            "errorGroups"
        );
        assert_eq!(
            method("agentTraces", json!({ "service": "api" })),
            "searchTraces"
        );
        assert_eq!(method("agentTraces", json!({ "traceId": "abc" })), "trace");
        assert_eq!(method("agentDashboards", json!({})), "dashboards");
        assert_eq!(
            method("agentDashboards", json!({ "id": "d1" })),
            "dashboard"
        );
        assert_eq!(
            method("agentFields", json!({ "name": "http.route" })),
            "fieldValues"
        );
        assert_eq!(
            method("agentFields", json!({ "search": "http" })),
            "fieldKeys"
        );
        assert_eq!(method("searchLogs", json!({})), "searchLogs");
    }

    #[test]
    fn a_panel_is_found_inside_the_dashboard_named_by_id() {
        let (routed, input) =
            route("agentDashboards", json!({ "id": "d1", "panelId": "p1" })).expect("routes");
        assert_eq!(routed, "dashboardPanel");
        assert_eq!(input["dashboardId"], "d1");
        assert!(route("agentDashboards", json!({ "panelId": "p1" })).is_err());
    }
}
