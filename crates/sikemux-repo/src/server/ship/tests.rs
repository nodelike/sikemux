use super::gate::github_write;
use super::tools::{cloudflare_zone_target, rundeck_target, vercel_redeploy_target};

/// @Yanagi's writes to GitHub each asked "Change writes on GitHub's API with this work?",
/// five cards the person couldn't tell apart.
#[test]
fn a_github_write_says_what_it_writes() {
    for (what, said) in [
        (
            "gh api repos/anomalyco/opencode/pulls/63648/comments/4558641882/replies -f body=Done: shortened it --jq .html_url",
            "Comment on anomalyco/opencode#63648",
        ),
        (
            "gh api -X PUT repos/nodelike/awesome-hermes-agents/contents/README.md -f message=Add Sikemux -f branch=main",
            "Update README.md in nodelike/awesome-hermes-agents",
        ),
        (
            "gh api repos/nodelike/awesome-opencode/git/refs -f ref=refs/heads/add-sikemux -f sha=c125252",
            "Create a branch in nodelike/awesome-opencode",
        ),
        (
            "gh api -X PATCH repos/nodelike/awesome-opencode/git/refs/heads/add-sikemux -f sha=c125252 -F force=true",
            "Move branch add-sikemux in nodelike/awesome-opencode",
        ),
        ("gh api -X POST repos/owner/repo/forks", "Fork owner/repo"),
        (
            "gh api -X POST repos/{owner}/{repo}/pages -f build_type=workflow",
            "Turn on GitHub Pages for nodelike/sikemux",
        ),
        (
            "gh api -X PUT repos/acme/web/rulesets/24037079 --input - --jq .conditions",
            "Change who may push what to acme/web",
        ),
        (
            "gh api -X POST repos/acme/web/deployments/77/statuses -f state=success",
            "Change acme/web on GitHub as you",
        ),
    ] {
        let here = || Some("nodelike/sikemux".to_owned());
        assert_eq!(github_write(what, here).as_deref(), Some(said), "{what}");
    }
}

#[test]
fn connector_lookup_errors_fail_closed_and_vercel_target_is_case_insensitive() {
    use crate::ship::Target;
    use serde_json::json;

    assert!(rundeck_target(Err("timeout".into())).is_err());
    assert!(rundeck_target(Ok(json!({ "path": "prod/deploy" }))).is_err());
    assert_eq!(
        rundeck_target(Ok(json!({ "path": "prod/deploy", "production": true }))),
        Ok(Target::RundeckJob {
            path: "prod/deploy".into(),
            production: true,
        })
    );

    assert!(vercel_redeploy_target(None, || Err("api error".into())).is_err());
    assert!(vercel_redeploy_target(None, || Ok(json!({}))).is_err());
    assert_eq!(
        vercel_redeploy_target(Some("PRODUCTION"), || Err("unused".into())),
        Ok(Target::Vercel { production: true })
    );
    assert_eq!(
        vercel_redeploy_target(Some("Production"), || Err("unused".into())),
        Ok(Target::Vercel { production: true })
    );
    assert_eq!(
        vercel_redeploy_target(Some("preview"), || Err("unused".into())),
        Ok(Target::Vercel { production: false })
    );
    assert_eq!(
        vercel_redeploy_target(None, || Ok(json!({ "target": "PRODUCTION" }))),
        Ok(Target::Vercel { production: true })
    );
    assert_eq!(
        vercel_redeploy_target(None, || Ok(json!({ "target": "preview" }))),
        Ok(Target::Vercel { production: false })
    );

    assert!(cloudflare_zone_target(None, || Err("unused".into())).is_err());
    assert!(
        cloudflare_zone_target(Some("023e105f4ecef8ad9ca31a8372d0c353"), || Err(
            "api error".into()
        ))
        .is_err()
    );
    assert!(
        cloudflare_zone_target(Some("023e105f4ecef8ad9ca31a8372d0c353"), || Ok(json!({}))).is_err()
    );
    assert_eq!(
        cloudflare_zone_target(Some("023e105f4ecef8ad9ca31a8372d0c353"), || Ok(
            json!({ "name": "Sikemux.COM." })
        )),
        Ok(Target::CloudflareZone {
            zone: "sikemux.com".into(),
        })
    );
    assert_eq!(
        cloudflare_zone_target(Some("Sikemux.COM."), || Err("unused".into())),
        Ok(Target::CloudflareZone {
            zone: "sikemux.com".into(),
        })
    );
}
