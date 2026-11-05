use std::sync::Arc;

use sikemux_plugin_api::Plugin;

pub fn plugins() -> Vec<Arc<dyn Plugin>> {
    vec![
        #[cfg(feature = "aws")]
        sikemux_plugin_aws::plugin(),
        #[cfg(feature = "bitbucket")]
        sikemux_plugin_bitbucket::plugin(),
        #[cfg(feature = "database")]
        sikemux_plugin_database::plugin(),
        #[cfg(feature = "github")]
        sikemux_plugin_github::plugin(),
        #[cfg(feature = "gitlab")]
        sikemux_plugin_gitlab::plugin(),
        #[cfg(feature = "jira")]
        sikemux_plugin_jira::plugin(),
        #[cfg(feature = "linear")]
        sikemux_plugin_linear::plugin(),
        #[cfg(feature = "rundeck")]
        sikemux_plugin_rundeck::plugin(),
        #[cfg(feature = "signoz")]
        sikemux_plugin_signoz::plugin(),
        #[cfg(feature = "slack")]
        sikemux_plugin_slack::plugin(),
        #[cfg(feature = "vercel")]
        sikemux_plugin_vercel::plugin(),
    ]
}
