use std::sync::Arc;

use sikemux_plugin_api::{Plugin, PluginError};

pub fn plugins() -> Vec<Arc<dyn Plugin>> {
    let compiled_in: Vec<Result<Arc<dyn Plugin>, PluginError>> = vec![
        #[cfg(feature = "aws")]
        sikemux_plugin_aws::plugin(),
        #[cfg(feature = "bitbucket")]
        sikemux_plugin_bitbucket::plugin(),
        #[cfg(feature = "bruno")]
        sikemux_plugin_bruno::plugin(),
        #[cfg(feature = "database")]
        sikemux_plugin_database::plugin(),
        #[cfg(feature = "github")]
        sikemux_plugin_github::plugin(),
        #[cfg(feature = "gitlab")]
        sikemux_plugin_gitlab::plugin(),
        #[cfg(feature = "jira")]
        sikemux_plugin_jira::plugin(),
        #[cfg(feature = "rundeck")]
        sikemux_plugin_rundeck::plugin(),
        #[cfg(feature = "signoz")]
        sikemux_plugin_signoz::plugin(),
    ];
    compiled_in
        .into_iter()
        .filter_map(|plugin| {
            plugin
                .inspect_err(|error| eprintln!("a built-in plugin failed to load: {error}"))
                .ok()
        })
        .collect()
}
