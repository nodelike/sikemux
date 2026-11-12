use std::path::Path;
use std::sync::Arc;

use sikemux_view::inbox::ShipKind;
use sikemux_view::places::Recognise;

use super::place::{new_place, StepPass};
use super::*;
use crate::ship::Target;

const PROJECT: &str = "/tmp/sikemux-places-test";

fn job(name: &str, production: bool, path: &str, step: StepPass) -> Place {
    let fact = Recognise::RundeckJob {
        path: path.to_owned(),
    };
    let mut place = new_place(name, production, vec![fact], None);
    place.step = Some(step);
    place
}

fn project() -> Vec<Place> {
    vec![
        job("Dev", false, "dev/*", StepPass::Deploy),
        job("Staging", false, "staging/*", StepPass::Check),
        job("Production", true, "prod/*", StepPass::Deploy),
    ]
}

fn core_set(places: Vec<Place>) -> Arc<Core> {
    let core = Core::new(Default::default(), None).unwrap();
    change(&core, PROJECT, |kept| {
        kept.places = places;
        kept.confirmed = Some(1);
        Ok(())
    })
    .unwrap();
    core
}

fn at(path: &str, production: bool) -> Option<Target> {
    Some(Target::RundeckJob {
        path: path.to_owned(),
        production,
    })
}

fn try_ship(core: &Core, target: Option<Target>) -> Result<bool, String> {
    let path = match &target {
        Some(Target::RundeckJob { path, .. }) => path.clone(),
        _ => String::new(),
    };
    let ship = Ship {
        kind: ShipKind::Deploy,
        what: format!("rundeck_run {{\"job\":\"{path}\"}}"),
        target,
    };
    crate::server::ship::check(core, "agent", ship, Path::new(PROJECT))
        .map(|deploy| deploy.is_some())
}

/// `rundeck_run` on a job under `dev/backend/` waited for Allow once just as production would.
#[test]
fn a_dev_job_is_not_held() {
    let core = core_set(project());
    assert_eq!(
        try_ship(&core, at("dev/backend/deploy-api", false)),
        Ok(true)
    );
    assert!(core.ship.waiting().is_empty());
}

#[test]
fn a_staging_job_waits_on_the_check_and_asks_nobody() {
    let core = core_set(project());
    let said = try_ship(&core, at("staging/deploy", false)).unwrap_err();
    assert!(said.contains("once the project's check passes"), "{said}");
    assert!(core.ship.waiting().is_empty());
}

/// What a project couldn't recognise held as production until 2026-11-12; a job in a
/// folder whose name says it isn't production now runs.
#[test]
fn production_holds_for_the_person_and_a_qa_job_runs() {
    let core = core_set(project());
    assert!(try_ship(&core, at("prod/deploy", false)).is_err());
    assert_eq!(try_ship(&core, at("qa/deploy", false)), Ok(false));
    let held: Vec<Option<String>> = (core.ship.waiting().into_iter())
        .map(|waiting| waiting.hold.environment)
        .collect();
    assert_eq!(held, [Some("Production".to_owned())]);
}

/// 47 `rundeck_run` holds on `dev/backend/*` in a project with no environments since
/// 2026-11-03, one after the person said "deploy on dev, I'm out". With nothing set, a job in a
/// folder that reads as dev runs, and one whose facts say production still waits. A job whose
/// own name says production waits by itself, and leaves the rest of its folder running.
#[test]
fn with_nothing_set_a_dev_job_runs_and_production_waits() {
    let core = core_set(Vec::new());
    for still_waits in [
        at("dev/backend/promote-to-prod", false),
        at("prod/deploy", false),
    ] {
        assert!(
            try_ship(&core, still_waits.clone()).is_err(),
            "{still_waits:?}"
        );
    }
    for runs in [
        at("dev/backend/deploy-api", false),
        at("dev/backend/reprocess", false),
    ] {
        assert_eq!(try_ship(&core, runs.clone()), Ok(false), "{runs:?}");
    }
    assert!(try_ship(&core, at("dev/backend/deploy-api", true)).is_err());
}

fn judge_line(places: &[Place], line: &str, cwd: &Path, env: &[(&str, &str)]) -> Goes {
    let env = |name: &str| {
        (env.iter())
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned())
    };
    let ships = crate::ship::of_command(line, &[], crate::ship::Looks::Every, cwd, &env);
    assert_eq!(ships.len(), 1, "{line}: {ships:?}");
    judge::judged(places, true, &ships[0], "Ada").goes
}

/// `make deploy` always allowed before places came, and `make deploy ENV=prod` then ran.
#[test]
fn a_production_word_wins_over_a_place_that_is_not_production() {
    let allowed = new_place(
        "Allowed",
        false,
        vec![Recognise::Command {
            words: "make deploy".into(),
        }],
        None,
    );
    let ship = Ship {
        kind: ShipKind::Deploy,
        what: "make deploy ENV=prod".into(),
        target: None,
    };
    assert_eq!(
        judge::judged(std::slice::from_ref(&allowed), true, &ship, "Ada").goes,
        Goes::Asks
    );
    let plain = Ship {
        what: "make deploy".into(),
        ..ship
    };
    assert_eq!(
        judge::judged(&[allowed], true, &plain, "Ada").goes,
        Goes::Freely
    );
}

#[test]
fn a_name_frees_a_target_only_when_nothing_else_says_production() {
    let cwd = Path::new("/nowhere");
    let asks = [
        ("aws s3 sync dist s3://acme-prod --profile dev", vec![]),
        (
            "terraform apply -var-file=prod.tfvars",
            vec![("TF_WORKSPACE", "dev")],
        ),
        (
            "kubectl config use-context prod && kubectl apply -f x",
            vec![],
        ),
        (
            "export AWS_PROFILE=prod; aws ecs update-service --service x",
            vec![],
        ),
        ("kubectl --context main apply -f x", vec![]),
        ("terraform apply", vec![("TF_WORKSPACE", "default")]),
        ("fly deploy -a acme", vec![]),
        (
            "aws ecs update-service --service x",
            vec![("AWS_ACCESS_KEY_ID", "x")],
        ),
    ];
    for (line, env) in asks {
        assert_eq!(judge_line(&[], line, cwd, &env), Goes::Asks, "{line}");
    }
    for line in [
        "kubectl --context staging apply -f x",
        "fly deploy -a acme-staging",
        "aws ecs update-service --service x --profile dev",
    ] {
        assert_eq!(judge_line(&[], line, cwd, &[]), Goes::Freely, "{line}");
    }
}

#[test]
fn a_ship_command_its_file_lists_is_production_whatever_its_words() {
    let project = tempfile::tempdir().unwrap();
    let config = r#"{ "version": 1, "ship": ["gh workflow run deploy.yml -f note=test"] }"#;
    std::fs::write(project.path().join("sikemux.json"), config).unwrap();
    let found = found::in_files(project.path());
    assert!(found.iter().all(|place| place.shown.production));
    let ship = Ship {
        kind: ShipKind::Deploy,
        what: "gh workflow run deploy.yml -f note=test".into(),
        target: None,
    };
    assert_eq!(judge::judged(&found, true, &ship, "Ada").goes, Goes::Asks);
}

use super::push::tests::git;

/// A checkout whose remote's main has `deploy.yml` deploying to production and
/// `preview.yml` deploying to staging, with `deploy.yml` edited here to say staging.
fn workflows() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let work = dir.path().join("work");
    let folder = work.join(".github/workflows");
    std::fs::create_dir_all(&folder).unwrap();
    git(
        dir.path(),
        &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
    );
    git(&work, &["init", "-b", "main"]);
    let job = |environment: &str| {
        format!("name: Ship\non: workflow_dispatch\njobs:\n  go:\n    environment: {environment}\n    runs-on: ubuntu-latest\n")
    };
    std::fs::write(folder.join("deploy.yml"), job("production")).unwrap();
    std::fs::write(folder.join("preview.yml"), job("staging")).unwrap();
    let pages = "name: Docs\non: workflow_dispatch\njobs:\n  go:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/deploy-pages@v4\n";
    std::fs::write(folder.join("docs.yml"), pages).unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "workflows"]);
    git(
        &work,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&work, &["push", "-u", "origin", "main"]);
    git(&work, &["remote", "set-head", "origin", "main"]);
    std::fs::write(folder.join("deploy.yml"), job("staging")).unwrap();
    dir
}

/// A workflow was judged from the checkout's own copy, which GitHub doesn't run.
#[test]
fn a_workflow_is_read_as_github_will_run_it() {
    let dir = workflows();
    let work = dir.path().join("work");
    let goes = |line: &str, env: &[(&str, &str)]| judge_line(&[], line, &work, env);
    assert_eq!(goes("gh workflow run deploy.yml", &[]), Goes::Asks);
    assert_eq!(goes("gh workflow run preview.yml", &[]), Goes::Unplaced);
    assert_eq!(goes("gh workflow run docs.yml", &[]), Goes::Asks);
    assert_eq!(goes("gh workflow run 4211", &[]), Goes::Unplaced);
    assert_eq!(
        goes("gh workflow run preview.yml -f target=prod", &[]),
        Goes::Asks
    );
    assert_eq!(
        goes("gh workflow run release.yml --repo acme/web", &[]),
        Goes::Asks
    );
    assert_eq!(
        goes("gh workflow run cd.yml", &[("GH_REPO", "acme/infra")]),
        Goes::Unplaced
    );
    assert_eq!(
        goes(
            "gh workflow run deploy-site.yml",
            &[("GH_REPO", "acme/web")]
        ),
        Goes::Asks
    );
}

#[test]
fn reading_the_places_writes_nothing() {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("vercel.json"),
        r#"{ "alias": ["acme.dev"] }"#,
    )
    .unwrap();
    let core = Core::new(Default::default(), None).unwrap();
    let path = project.path().to_string_lossy().into_owned();
    let read = of(&core, &path);
    assert_eq!(read.places.len(), 1);
    assert!(read.places[0].production);
    assert!(core.places.of(&path).places.is_empty());
}

#[test]
fn what_nothing_places_runs_and_is_listed_once() {
    let core = core_set(project());
    for _ in 0..2 {
        let ship = Ship {
            kind: ShipKind::Publish,
            what: "curl -d x https://hooks.slack.com/x".to_owned(),
            target: Some(Target::Site {
                origin: "https://hooks.slack.com".to_owned(),
            }),
        };
        let ran = crate::server::ship::check(&core, "agent", ship, Path::new(PROJECT));
        assert_eq!(ran.map(|deploy| deploy.is_some()), Ok(false));
    }
    let seen = core.places.of(PROJECT).unplaced;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].seen.times, 2);
}

/// A yes for one Rundeck job covered every job in its folder; outside a repository a yes
/// covers its exact action once, and a no holds however the action is spelled.
#[tokio::test(flavor = "multi_thread")]
async fn a_yes_covers_its_own_job_once() {
    use crate::server::access::Peer;
    let core = core_set(project());
    assert!(try_ship(&core, at("prod/deploy", false)).is_err());
    assert!(try_ship(&core, at("prod/deploy-again", false)).is_err());
    let calls = crate::server::person_calls::shown(&core);
    assert_eq!(calls.len(), 2);
    let of_job = |job: &str| {
        let action = format!("rundeck_run {{\"job\":\"{job}\"}}");
        let call = calls
            .iter()
            .find(|call| call.action.as_deref() == Some(action.as_str()));
        call.cloned().unwrap()
    };
    let (first, second) = (of_job("prod/deploy"), of_job("prod/deploy-again"));
    assert!(may_pass(&core, &first, &Peer::Local, Some("allow")).is_err());
    assert!(may_pass(&core, &first, &Peer::App, None).is_err());
    let answer = |id: &str, option: &'static str| {
        let (core, hold) = (
            core.clone(),
            crate::server::ship::HoldParams {
                hold: id.to_owned(),
            },
        );
        async move { crate::server::person_calls::answer_hold(&core, &Peer::App, hold, option).await }
    };
    answer(&first.id, "allow").await.unwrap();
    assert_eq!(try_ship(&core, at("prod/deploy", false)), Ok(true));
    assert!(try_ship(&core, at("prod/deploy", false)).is_err());
    assert!(try_ship(&core, at("prod/deploy-again", false)).is_err());
    answer(&second.id, "refuse").await.unwrap();
    let said = try_ship(&core, at("prod/deploy-again", false)).unwrap_err();
    assert!(said.contains("said no"), "{said}");
}

/// `gh api graphql` with a query that holds no mutation raised a production call.
#[test]
fn a_graphql_read_is_not_a_write() {
    let cwd = Path::new("/nowhere");
    let ships =
        |line: &str| crate::ship::of_command(line, &[], crate::ship::Looks::Every, cwd, &|_| None);
    assert!(ships("gh api graphql -f query='query { viewer { login } }'").is_empty());
    assert!(ships("gh api repos/acme/web/pulls -X GET -f state=open").is_empty());
    assert_eq!(
        judge_line(
            &[],
            "gh api graphql -f query='mutation { addStar(input: {}) { clientMutationId } }'",
            cwd,
            &[]
        ),
        Goes::Asks
    );
    assert_eq!(
        judge_line(&[], "gh api repos/acme/web/issues -f title=x", cwd, &[]),
        Goes::Asks
    );
}

/// `stage: ${opt:stage, 'prod'}` and a `serverless.ts` read as dev.
#[test]
fn a_stage_that_cannot_be_read_asks() {
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("serverless.yml");
    std::fs::write(
        &file,
        "service: shop\nprovider:\n  stage: ${opt:stage, 'prod'}\n",
    )
    .unwrap();
    assert_eq!(
        judge_line(&[], "serverless deploy", project.path(), &[]),
        Goes::Asks
    );
    std::fs::write(&file, "service: shop\nprovider:\n  name: aws\n").unwrap();
    assert_eq!(
        judge_line(&[], "serverless deploy", project.path(), &[]),
        Goes::Freely
    );
    std::fs::remove_file(&file).unwrap();
    std::fs::write(project.path().join("serverless.ts"), "export default {}").unwrap();
    assert_eq!(
        judge_line(&[], "serverless deploy", project.path(), &[]),
        Goes::Asks
    );
}

/// Plain `supabase db push` and `prisma migrate deploy` with its address in `.env` weren't seen.
#[test]
fn a_migration_asks_unless_its_database_is_local_or_named_otherwise() {
    let project = tempfile::tempdir().unwrap();
    let cwd = project.path();
    let ships =
        |line: &str| crate::ship::of_command(line, &[], crate::ship::Looks::Every, cwd, &|_| None);
    assert_eq!(judge_line(&[], "supabase db push", cwd, &[]), Goes::Asks);
    assert!(ships("supabase db push --local").is_empty());
    let env = cwd.join(".env");
    std::fs::write(&env, "DATABASE_URL=postgres://u:p@localhost:5432/app\n").unwrap();
    assert!(ships("prisma migrate deploy").is_empty());
    std::fs::write(
        &env,
        "DATABASE_URL=postgres://u:p@staging-db.acme.dev/app\n",
    )
    .unwrap();
    assert_eq!(
        judge_line(&[], "prisma migrate deploy", cwd, &[]),
        Goes::Freely
    );
    std::fs::write(&env, "DATABASE_URL=postgres://u:p@db.acme.dev/app\n").unwrap();
    assert_eq!(
        judge_line(&[], "prisma migrate deploy", cwd, &[]),
        Goes::Asks
    );
}

/// Reads asked or were listed: each must run with no card and no listing, while the change
/// beside it is still seen.
#[test]
fn reads_never_reach_the_judge() {
    let project = tempfile::tempdir().unwrap();
    let cwd = project.path();
    std::fs::write(cwd.join("read.graphql"), "query { viewer { login } }").unwrap();
    std::fs::write(
        cwd.join("star.graphql"),
        "mutation { addStar(input: {}) { clientMutationId } }",
    )
    .unwrap();
    let ships =
        |line: &str| crate::ship::of_command(line, &[], crate::ship::Looks::Every, cwd, &|_| None);
    let pairs = [
        ("kubectl get pods", "kubectl delete pod web"),
        (
            "kubectl describe deploy web",
            "kubectl scale deploy web --replicas=2",
        ),
        (
            "kubectl rollout status deploy/web",
            "kubectl rollout restart deploy/web",
        ),
        (
            "kubectl rollout history deploy/web",
            "kubectl rollout undo deploy/web",
        ),
        ("kubectl auth can-i create pods", "kubectl create ns web"),
        (
            "kubectl apply -f k8s --dry-run=client -o yaml",
            "kubectl apply -f k8s",
        ),
        ("helm list", "helm upgrade web chart"),
        ("helm status web", "helm uninstall web"),
        ("helm upgrade web chart --dry-run", "helm install web chart"),
        (
            "kubectl get pods --help",
            "helm upgrade web chart --description --dry-run",
        ),
        ("helm plugin install https://x", "helm rollback web 3"),
        ("terraform plan", "terraform apply"),
        ("terraform show", "terraform destroy"),
        ("aws s3 ls s3://acme", "aws s3 rm s3://acme/x"),
        (
            "aws ecs describe-services --services web",
            "aws ecs update-service --service web",
        ),
        ("aws s3 cp s3://acme/x .", "aws s3 cp x s3://acme/x"),
        ("vercel ls --prod", "vercel --prod"),
        ("vercel build --prod", "vercel deploy --prod"),
        ("vercel alias ls", "vercel alias set dpl acme.dev"),
        ("npm publish --dry-run", "npm publish"),
        ("cargo publish --dry-run", "cargo publish"),
        ("supabase db push --dry-run", "supabase db push"),
        ("git push --dry-run origin main", "git push origin main"),
        ("dokku config web", "dokku config:set web A=1"),
        ("dokku apps:exists web", "dokku ps:rebuild web"),
        ("serverless deploy list", "serverless deploy"),
        (
            "gh api graphql -F query=@read.graphql",
            "gh api graphql -F query=@star.graphql",
        ),
        ("gh pr view 12", "gh pr merge 12"),
        (
            "psql -h db.acme.com -c 'select 1'",
            "psql -h db.acme.com -c 'delete from t'",
        ),
        (
            "mysql -h db.acme.com -e 'SHOW TABLES'",
            "mysql -h db.acme.com -e 'DROP TABLE t'",
        ),
        ("pnpm run build:release", "pnpm publish"),
        ("./scripts/release-notes.sh", "twine upload dist/*"),
    ];
    for (read, write) in pairs {
        assert!(ships(read).is_empty(), "{read}: {:?}", ships(read));
        assert!(!ships(write).is_empty(), "{write}");
    }
}

/// This repository's docs workflow deploys with `npx "$VERCEL_CLI" deploy --prebuilt --prod`,
/// which no deploy string matched, and ran unasked.
#[test]
fn this_repositorys_docs_workflow_asks() {
    let docs = include_str!("../../../../../.github/workflows/docs.yml");
    let read = crate::ship::read_workflow(docs);
    assert!(read.deploys);
    let calls_another = "jobs:\n  go:\n    uses: ./.github/workflows/_deploy.yml\n";
    assert!(!crate::ship::read_workflow(calls_another).deploys);
    assert_eq!(
        crate::ship::called_workflows(calls_another),
        [".github/workflows/_deploy.yml"]
    );
    let named = Target::Workflow {
        file: "deploy.yml".into(),
        environments: Vec::new(),
        publishes: false,
        deploys: false,
        read: true,
        inputs: Vec::new(),
    };
    assert!(judge::workflow_asks(&named));
}
