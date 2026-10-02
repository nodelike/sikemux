use std::collections::HashMap;

use sikemux_process::user_environment::{command, provide, var, var_os, UserEnvironment};

fn user_path() -> String {
    format!(
        "/from/the/login/shell:{}",
        std::env::var("PATH").unwrap_or_default()
    )
}

fn source() -> UserEnvironment {
    UserEnvironment {
        variables: HashMap::from([
            ("PATH".to_string(), user_path()),
            ("SIKEMUX_TEST_TOKEN".to_string(), "from-rc".to_string()),
        ]),
    }
}

#[test]
fn children_and_readers_see_the_users_environment() {
    provide(source);

    let command = command("tool");
    let envs: HashMap<_, _> = command
        .get_envs()
        .filter_map(|(key, value)| Some((key.to_str()?, value?.to_str()?)))
        .collect();

    assert_eq!(envs.get("PATH").copied(), Some(user_path().as_str()));
    assert_eq!(var("SIKEMUX_TEST_TOKEN").as_deref(), Some("from-rc"));
    assert_eq!(var_os("PATH"), Some(user_path().into()));
    assert_eq!(var("HOME"), std::env::var("HOME").ok());
}
