//! An org's vault key goes only to hosts someone let in by a code and those they vouch for, and is
//! taken only from them, whatever the accounts server lists.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use iroh::SecretKey;
use serde_json::{json, Value};
use sikemux_calls::vault::{OrgLetInAnswer, OrgVaultStatus};
use sikemux_process::sync::Locked;
use sikemux_view::device::{AccountOrg, OrgRole};
use sikemux_wire::protocol::BuildIdentity;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::server::remote;
use crate::server::remote_state::RemoteParts;
use crate::server::vault::{keys, random_key, Key};
use crate::server::{Core, CoreResult, ServerCore};

const OWNER: &str = "member-owner";
const MEMBER: &str = "member-other";

/// What the stand-in accounts server holds of one org's vault. It checks nothing.
#[derive(Default)]
struct Kept {
    org: String,
    generation: i64,
    rotate: bool,
    hosts: BTreeMap<String, String>,
    wraps: BTreeMap<String, String>,
    vouches: Vec<Value>,
    entries: Vec<Value>,
    ended: Vec<Value>,
    /// What it answers requests that end so, in place of taking them.
    refuses: Option<(&'static str, u16)>,
}

fn text<'a>(of: &'a Value, field: &str) -> &'a str {
    of[field].as_str().unwrap()
}

type Shared = Arc<Mutex<Kept>>;

/// An org of its own, as a host signs a request to one path at most once a second.
fn server() -> Shared {
    let org = format!("org_2{}", SecretKey::generate().public());
    Arc::new(Mutex::new(Kept {
        org,
        ..Kept::default()
    }))
}

impl Kept {
    fn state(&self, asker: &str) -> Value {
        let (holders, waiting): (Vec<&String>, Vec<&String>) =
            (self.hosts.keys()).partition(|host| self.wraps.contains_key(*host));
        let hosts: Vec<Value> = (self.hosts.iter())
            .map(|(host, member)| {
                json!({ "hostKey": host, "member": member, "computer": format!("{member}'s Mac") })
            })
            .collect();
        let vouches: Vec<Value> = (self.vouches.iter())
            .filter(|vouch| {
                self.hosts.get(text(vouch, "hostKey")) == Some(&text(vouch, "member").into())
            })
            .map(|vouch| {
                let mut vouch = vouch.clone();
                if vouch["hostKey"] != asker {
                    vouch["introduction"] = Value::Null;
                }
                vouch
            })
            .collect();
        json!({ "vault": {
            "generation": self.generation, "key": self.wraps.get(asker), "holders": holders,
            "waiting": waiting, "rotate": self.rotate, "remadeAt": null,
            "entries": self.entries, "ended": self.ended, "changes": 0, "since": null, "hosts": hosts, "vouches": vouches,
        }})
    }

    fn keep(&mut self, request: &str, asker: &str, body: &Value) -> u16 {
        if let Some((_, status)) = self.refuses.filter(|(end, _)| request.ends_with(end)) {
            return status;
        }
        if let Some(key) = body["key"].as_str() {
            self.generation = 1;
            self.wraps.insert(asker.to_owned(), key.to_owned());
        }
        for vouch in body["vouches"].as_array().into_iter().flatten() {
            let mut vouch = vouch.clone();
            vouch["member"] = json!(self.hosts[text(&vouch, "hostKey")]);
            vouch["by"] = json!(asker);
            vouch["byMember"] = json!(self.hosts[asker]);
            let had = (self.vouches.iter())
                .position(|had| had["hostKey"] == vouch["hostKey"] && had["by"] == vouch["by"])
                .map(|at| self.vouches.remove(at));
            if let Some(had) = had.filter(|_| vouch["introduction"].is_null()) {
                vouch["introduction"] = had["introduction"].clone();
            }
            self.vouches.push(vouch);
        }
        if request.ends_with("/rotate") {
            if body["from"] != self.generation {
                return 409;
            }
            self.generation += 1;
            self.rotate = false;
            self.wraps.clear();
            for dropped in body["drops"].as_array().into_iter().flatten() {
                let revision = dropped["revision"].as_i64().unwrap() + 1;
                let mark = json!({ "id": dropped["id"], "revision": revision });
                self.entries.retain(|entry| entry["id"] != dropped["id"]);
                self.ended.push(mark);
            }
            let resealed = body["entries"].as_array().unwrap();
            assert_eq!(resealed.len(), self.entries.len());
            for resealed in resealed {
                let entry = (self.entries.iter_mut())
                    .find(|entry| entry["id"] == resealed["id"])
                    .unwrap();
                entry["sealed"] = resealed["sealed"].clone();
                entry["revision"] = json!(resealed["revision"].as_i64().unwrap() + 1);
                entry["generation"] = json!(self.generation);
            }
        }
        for key in body["keys"].as_array().into_iter().flatten() {
            self.wraps
                .entry(text(key, "hostKey").to_owned())
                .or_insert_with(|| text(key, "wrapped").to_owned());
        }
        200
    }

    /// Puts an entry, sealed as given, in place of any with its id.
    fn put(&mut self, (id, kind): (&str, &str), revision: i64, sealed: String) {
        self.entries.retain(|entry| entry["id"] != id);
        self.entries.push(json!({
            "id": id, "kind": kind, "generation": self.generation, "sealed": sealed,
            "revision": revision, "updatedBy": "", "updatedAt": "",
        }));
    }
}

/// Where `asker` reaches the stand-in server.
async fn serve(kept: Shared, asker: String) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let mut stream = BufReader::new(stream);
            let mut first = String::new();
            stream.read_line(&mut first).await.unwrap();
            let mut length = 0;
            loop {
                let mut line = String::new();
                stream.read_line(&mut line).await.unwrap();
                let line = line.trim().to_lowercase();
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            stream.read_exact(&mut body).await.unwrap();
            let request = first.rsplit_once(' ').unwrap().0.to_owned();
            let (status, answer) = {
                let mut kept = kept.locked();
                let body = serde_json::from_slice(&body).unwrap_or_default();
                let status = kept.keep(&request, &asker, &body);
                (status, kept.state(&asker).to_string())
            };
            let answer = format!(
                "HTTP/1.1 {status} Answer\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                answer.len()
            );
            stream.write_all(answer.as_bytes()).await.unwrap();
        }
    });
    base
}

struct Host {
    core: Arc<Core>,
    secret: SecretKey,
    org: AccountOrg,
}

impl Host {
    /// A host the server lists on the account with membership `member`.
    async fn new(kept: &Shared, member: &str, role: OrgRole) -> Host {
        Host::known_as(kept, SecretKey::generate(), member, role).await
    }

    async fn known_as(kept: &Shared, secret: SecretKey, member: &str, role: OrgRole) -> Host {
        let me = secret.public().to_string();
        kept.locked().hosts.insert(me.clone(), member.to_owned());
        let base = serve(kept.clone(), me).await;
        let core = Core::new(BuildIdentity::default(), None).unwrap();
        core.remote()
            .stand_in(secret.clone(), Some(member), Vec::new());
        core.remote().set_accounts_api(&base);
        let org = AccountOrg {
            id: kept.locked().org.clone(),
            name: "Team".into(),
            role,
            active: true,
            preview: false,
            hidden: false,
        };
        Host { core, secret, org }
    }

    /// The owner's host that made the vault, alone in holding its key.
    async fn founder(kept: &Shared) -> Host {
        Host::founder_of(kept, &SecretKey::generate()).await
    }

    async fn founder_of(kept: &Shared, secret: &SecretKey) -> Host {
        let host = Host::known_as(kept, secret.clone(), OWNER, OrgRole::Owner).await;
        host.read().await;
        keys::make_org(&host.core, &host.org.id, secret)
            .await
            .unwrap();
        assert!(host.open());
        host
    }

    /// An owner's host that holds `key` from before hosts were let in by their codes.
    async fn from_before(kept: &Shared, key: &Key) -> Host {
        kept.locked().generation = 1;
        let host = Host::new(kept, OWNER, OrgRole::Owner).await;
        host.keep_from_before(kept, key);
        host
    }

    fn keep_from_before(&self, kept: &Shared, key: &Key) {
        keys::keep_from_before(&self.core, &self.org.id, *key, 1);
        (kept.locked().wraps).insert(self.me(), "from before".into());
    }

    fn entries(&self) -> BTreeMap<String, (i64, Value)> {
        keys::org_entries(&self.core, &self.org.id)
    }

    fn me(&self) -> String {
        self.secret.public().to_string()
    }

    async fn read(&self) {
        keys::read_org(&self.core, &self.org, &self.secret)
            .await
            .unwrap();
    }

    fn open(&self) -> bool {
        self.core.vault().orgs().seen(&self.org.id).0 == OrgVaultStatus::Open
    }

    /// The code it shows once it read that it waits.
    async fn code(&self) -> String {
        self.read().await;
        assert!(!self.open());
        self.core.vault().orgs().seen(&self.org.id).1.unwrap()
    }

    async fn let_in(&self, code: &str) -> CoreResult<OrgLetInAnswer> {
        keys::let_in_to_org(&self.core, &self.org, &self.secret, code).await
    }
}

/// Has the server hand `to` a made-up key that `from` sealed for it.
fn hold(kept: &Shared, from: &SecretKey, to: &str) {
    let mut kept = kept.locked();
    let wrap = keys::org_wrap(&random_key().unwrap(), from, to, &kept.org, kept.generation);
    kept.wraps.insert(to.to_owned(), wrap);
}

fn list(kept: &Shared, member: &str) -> SecretKey {
    let host = SecretKey::generate();
    (kept.locked().hosts).insert(host.public().to_string(), member.to_owned());
    host
}

fn holders(kept: &Shared) -> Vec<String> {
    kept.locked().wraps.keys().cloned().collect()
}

fn sorted(hosts: &[&Host]) -> Vec<String> {
    let mut keys: Vec<String> = hosts.iter().map(|host| host.me()).collect();
    keys.sort();
    keys
}

#[tokio::test(flavor = "multi_thread")]
async fn an_orgs_key_is_sealed_for_no_listed_host_nobody_let_in() {
    for lock in [false, true] {
        let kept = server();
        let founder = Host::founder(&kept).await;
        let setting = serde_json::from_value(json!({ "requireApproval": lock })).unwrap();
        remote::set_require_approval(&founder.core, setting).unwrap();
        list(&kept, OWNER);
        list(&kept, MEMBER);
        founder.read().await;
        assert_eq!(holders(&kept), [founder.me()]);

        for member in [OWNER, MEMBER] {
            let held = list(&kept, member);
            hold(&kept, &held, &held.public().to_string());
        }
        kept.locked().rotate = true;
        founder.read().await;
        assert_eq!(kept.locked().generation, 2);
        assert_eq!(holders(&kept), [founder.me()]);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_orgs_key_is_taken_from_no_listed_holder_nobody_let_in() {
    let kept = server();
    kept.locked().generation = 1;
    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    for member in [OWNER, MEMBER] {
        let held = list(&kept, member);
        hold(&kept, &held, &held.public().to_string());
        hold(&kept, &held, &host.me());
        host.read().await;
        assert!(!host.open());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_let_in_by_its_code_is_sealed_for_and_lets_in_the_rest_of_its_account() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let first = Host::new(&kept, MEMBER, OrgRole::User).await;
    founder.let_in(&first.code().await).await.unwrap();
    first.read().await;
    assert!(first.open());

    let second = Host::new(&kept, MEMBER, OrgRole::User).await;
    let code = second.code().await;
    first.let_in(&code.to_lowercase()).await.unwrap();
    second.read().await;
    assert!(second.open());

    let third = Host::new(&kept, MEMBER, OrgRole::User).await;
    third.read().await;
    kept.locked().rotate = true;
    founder.read().await;
    assert_eq!(holders(&kept), sorted(&[&founder, &first, &second]));
    for host in [&first, &second] {
        host.read().await;
        assert!(host.open());
    }
    third.read().await;
    assert!(!third.open());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_holding_a_key_it_does_not_take_is_let_in_with_a_new_one() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    let held = list(&kept, OWNER);
    hold(&kept, &held, &held.public().to_string());
    hold(&kept, &held, &host.me());
    founder.let_in(&host.code().await).await.unwrap();
    assert_eq!(kept.locked().generation, 2);
    host.read().await;
    assert!(host.open());
    assert_eq!(holders(&kept), sorted(&[&founder, &host]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_code_lets_in_only_the_host_that_shows_it() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    let code = host.code().await;

    let mut altered: Vec<char> = code.chars().collect();
    let last = altered.len() - 1;
    altered[last] = if altered[last] == '2' { '3' } else { '2' };
    let altered: String = altered.into_iter().collect();
    assert!(founder.let_in(&altered).await.is_err());
    let cut: String = code.chars().take(last).collect();
    assert!(founder.let_in(&cut).await.is_err());

    let me = host.me();
    kept.locked().hosts.remove(&me);
    list(&kept, MEMBER);
    assert!(founder.let_in(&code).await.is_err());
    (kept.locked().hosts).insert(me.clone(), "member-rejoined".to_owned());
    assert!(founder.let_in(&code).await.is_err());
    assert_eq!(holders(&kept), [founder.me()]);

    kept.locked().hosts.insert(me, MEMBER.to_owned());
    founder.let_in(&code).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_lets_in_only_their_own_hosts() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let member = Host::new(&kept, MEMBER, OrgRole::User).await;
    founder.let_in(&member.code().await).await.unwrap();
    member.read().await;
    let colleague = Host::new(&kept, "member-third", OrgRole::User).await;
    assert!(member.let_in(&colleague.code().await).await.is_err());
    colleague.read().await;
    assert!(!colleague.open());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_opens_its_first_key_only_once_introduced() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    founder.let_in(&host.code().await).await.unwrap();
    let vouches = kept.locked().vouches.clone();

    kept.locked().vouches.clear();
    host.read().await;
    assert!(!host.open());
    kept.locked().vouches = vouches.clone();
    kept.locked().vouches[0]["introduction"] = Value::Null;
    host.read().await;
    assert!(!host.open());

    kept.locked().vouches = vouches;
    host.read().await;
    assert!(host.open());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_introduction_outlives_a_new_key_and_a_vouch_signed_again() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let admin = Host::new(&kept, "member-admin", OrgRole::Admin).await;
    founder.let_in(&admin.code().await).await.unwrap();
    admin.read().await;
    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    founder.let_in(&host.code().await).await.unwrap();

    let unsigned = json!("A".repeat(86));
    for vouch in &mut kept.locked().vouches {
        vouch["signature"] = unsigned.clone();
    }
    founder.read().await;
    assert!((kept.locked().vouches.iter())
        .filter(|vouch| vouch["by"] == founder.me().as_str())
        .all(|vouch| vouch["signature"] != unsigned));

    for rotated in [2, 3] {
        kept.locked().rotate = true;
        admin.read().await;
        assert_eq!(kept.locked().generation, rotated);
    }
    assert_eq!(holders(&kept), sorted(&[&founder, &admin, &host]));
    host.read().await;
    assert!(host.open());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_still_hears_of_those_let_in_by_one_that_left_since() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let leaver = Host::new(&kept, "member-leaver", OrgRole::Admin).await;
    founder.let_in(&leaver.code().await).await.unwrap();
    leaver.read().await;
    let offline = Host::new(&kept, "member-offline", OrgRole::Admin).await;
    founder.let_in(&offline.code().await).await.unwrap();
    offline.read().await;

    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    leaver.let_in(&host.code().await).await.unwrap();
    kept.locked().hosts.remove(&leaver.me());
    kept.locked().rotate = true;
    offline.read().await;
    assert_eq!(holders(&kept), sorted(&[&founder, &offline, &host]));
    for reader in [&founder, &host] {
        reader.read().await;
        assert!(reader.open());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vouch_names_one_host_however_the_server_spells_memberships() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let admin = Host::new(&kept, "member-admin", OrgRole::Admin).await;
    founder.let_in(&admin.code().await).await.unwrap();
    admin.read().await;
    let other = SecretKey::generate().public().to_string();
    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    let code = host.code().await;

    (kept.locked().hosts).insert(admin.me(), format!("member-admin|{other}"));
    admin.let_in(&code).await.unwrap();
    {
        let mut kept = kept.locked();
        let spelled = format!("{}|{MEMBER}", host.me());
        kept.hosts.insert(admin.me(), "member-admin".to_owned());
        kept.hosts.insert(other.clone(), spelled.clone());
        let vouch = (kept.vouches.iter_mut())
            .find(|vouch| vouch["hostKey"] == host.me().as_str())
            .unwrap();
        vouch["hostKey"] = json!(other);
        vouch["member"] = json!(spelled);
        vouch["byMember"] = json!("member-admin");
    }
    founder.read().await;
    assert!(!holders(&kept).contains(&other));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vouch_changed_or_made_for_another_org_counts_for_nothing() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let admin = Host::new(&kept, "member-admin", OrgRole::Admin).await;
    founder.let_in(&admin.code().await).await.unwrap();
    admin.read().await;
    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    let code = host.code().await;
    admin.let_in(&code).await.unwrap();
    let wrapped = kept.locked().wraps.remove(&host.me());
    assert!(wrapped.is_some());
    let made = (kept.locked().vouches.iter())
        .find(|vouch| vouch["hostKey"] == host.me().as_str())
        .unwrap()
        .clone();
    let with = |vouch: Value| {
        let mut kept = kept.locked();
        kept.vouches
            .retain(|had| had["hostKey"] != vouch["hostKey"] && had["hostKey"] != made["hostKey"]);
        kept.vouches.push(vouch);
    };

    let other = list(&kept, MEMBER).public().to_string();
    let mut changed = made.clone();
    changed["hostKey"] = json!(other);
    let mut later = made.clone();
    later["at"] = json!(made["at"].as_i64().unwrap() + 1);
    let elsewhere = {
        let kept = server();
        let founder = Host::founder_of(&kept, &founder.secret).await;
        let admin =
            Host::known_as(&kept, admin.secret.clone(), "member-admin", OrgRole::Admin).await;
        founder.let_in(&admin.code().await).await.unwrap();
        admin.read().await;
        let host = Host::known_as(&kept, host.secret.clone(), MEMBER, OrgRole::User).await;
        admin.let_in(&host.code().await).await.unwrap();
        let vouches = kept.locked().vouches.clone();
        (vouches.into_iter())
            .find(|vouch| vouch["hostKey"] == host.me().as_str())
            .unwrap()
    };
    for forged in [changed, later, elsewhere] {
        with(forged);
        founder.read().await;
        assert_eq!(holders(&kept), sorted(&[&founder, &admin]));
    }

    with(made.clone());
    founder.read().await;
    assert_eq!(holders(&kept), sorted(&[&founder, &admin, &host]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_dropped_from_the_org_is_let_in_anew() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let admin = Host::new(&kept, "member-admin", OrgRole::Admin).await;
    founder.let_in(&admin.code().await).await.unwrap();
    admin.read().await;
    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    admin.let_in(&host.code().await).await.unwrap();
    host.read().await;
    founder.read().await;

    kept.locked().hosts.remove(&host.me());
    kept.locked().rotate = true;
    founder.read().await;
    (kept.locked().hosts).insert(host.me(), MEMBER.to_owned());
    founder.read().await;
    assert_eq!(holders(&kept), sorted(&[&founder, &admin]));

    founder.let_in(&host.code().await).await.unwrap();
    host.read().await;
    assert!(host.open());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_introduction_made_with_another_code_counts_for_nothing() {
    let kept = server();
    Host::founder(&kept).await;
    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    host.code().await;

    let forged = server();
    forged.locked().org = kept.locked().org.clone();
    let other = Host::founder(&forged).await;
    let shown = Host::known_as(&forged, host.secret.clone(), MEMBER, OrgRole::User).await;
    other.let_in(&shown.code().await).await.unwrap();
    shown.read().await;
    assert!(shown.open());

    {
        let (mut kept, forged) = (kept.locked(), forged.locked());
        kept.hosts.extend(forged.hosts.clone());
        kept.wraps = forged.wraps.clone();
        kept.vouches = forged.vouches.clone();
    }
    host.read().await;
    assert!(!host.open());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_projects_entry_that_does_not_open_does_not_stop_the_key_being_replaced() {
    let kept = server();
    let key = random_key().unwrap();
    let org = kept.locked().org.clone();
    let seal =
        |id, revision, body: &Value| keys::org_entry(&key, &org, id, (1, revision), body).unwrap();
    let store = ("store", "project-memory");
    let said = json!({ "rules": ["kept"] });
    let owner = Host::from_before(&kept, &key).await;
    kept.locked().put(store, 3, seal(store, 3, &said));
    kept.locked().refuses = Some(("/rotate", 500));
    owner.read().await;
    assert_eq!(owner.entries()["store"], (3, said.clone()));

    kept.locked().refuses = None;
    kept.locked().put(store, 4, "A".repeat(64));
    let waiting = ("waiting", "project-memory-waiting");
    kept.locked().put(waiting, 7, "A".repeat(64));
    owner.read().await;
    assert_eq!(kept.locked().generation, 2);
    assert_eq!(
        owner.entries(),
        [("store".to_owned(), (5, said.clone()))].into()
    );
    assert_eq!(
        kept.locked().ended,
        [json!({ "id": "waiting", "revision": 8 })]
    );
    owner.read().await;
    assert_eq!(owner.entries(), [("store".to_owned(), (5, said))].into());

    assert!(keys::open_to_writes(&owner.core, &owner.org));
    kept.locked().put(("key", "model-key"), 1, "A".repeat(64));
    kept.locked().rotate = true;
    owner.read().await;
    assert_eq!(kept.locked().generation, 2);
    assert!(owner.open());
    assert!(!keys::open_to_writes(&owner.core, &owner.org));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_list_without_the_host_reading_it_drops_no_host_it_trusts() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    founder.let_in(&host.code().await).await.unwrap();
    let listed = std::mem::take(&mut kept.locked().hosts);
    founder.read().await;
    {
        let mut kept = kept.locked();
        kept.hosts = listed;
        kept.rotate = true;
    }
    founder.read().await;
    assert_eq!(holders(&kept), sorted(&[&founder, &host]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_from_before_hosts_were_let_in_is_replaced_once_and_sealed_for_no_other() {
    let kept = server();
    let key = random_key().unwrap();
    let first = Host::from_before(&kept, &key).await;
    let second = Host::from_before(&kept, &key).await;
    let waits = list(&kept, MEMBER);
    first.read().await;
    assert_eq!(kept.locked().generation, 2);
    assert_eq!(holders(&kept), [first.me()]);

    let code = second.code().await;
    first.read().await;
    assert_eq!(kept.locked().generation, 2);
    assert_eq!(holders(&kept), [first.me()]);

    first.let_in(&code).await.unwrap();
    second.read().await;
    assert!(second.open());
    second.read().await;
    assert_eq!(kept.locked().generation, 2);
    assert!(!holders(&kept).contains(&waits.public().to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_sealed_before_hosts_were_let_in_is_not_taken() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    founder.let_in(&host.code().await).await.unwrap();
    let hand = |seal: fn(&Key, &SecretKey, &str, &str, i64) -> String| {
        let mut kept = kept.locked();
        let key = random_key().unwrap();
        let wrap = seal(&key, &founder.secret, &host.me(), &kept.org, 1);
        kept.wraps.insert(host.me(), wrap);
    };
    hand(keys::org_wrap_from_before);
    host.read().await;
    assert!(!host.open());
    hand(keys::org_wrap);
    host.read().await;
    assert!(host.open());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_members_host_seals_no_key_from_before_hosts_were_let_in() {
    let kept = server();
    let key = random_key().unwrap();
    kept.locked().generation = 1;
    let member = Host::new(&kept, MEMBER, OrgRole::User).await;
    member.keep_from_before(&kept, &key);
    member.read().await;
    assert!(member.open());
    let own = Host::new(&kept, MEMBER, OrgRole::User).await;
    let code = own.code().await;
    assert!(member.let_in(&code).await.is_err());
    member.read().await;
    assert_eq!(holders(&kept), [member.me()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_orgs_key_goes_only_once_the_list_and_the_vault_both_say_it_is_gone() {
    for status in [403, 404] {
        let kept = server();
        let founder = Host::founder(&kept).await;
        let host = Host::new(&kept, MEMBER, OrgRole::User).await;
        founder.let_in(&host.code().await).await.unwrap();
        host.read().await;
        assert!(host.open());

        keys::leave_unlisted(&host.core, &[]).await;
        assert!(host.open());
        kept.locked().refuses = Some(("/vault", status));
        assert!(keys::read_org(&host.core, &host.org, &host.secret)
            .await
            .is_err());
        assert!(host.open());
        keys::leave_unlisted(&host.core, std::slice::from_ref(&host.org)).await;
        assert!(host.open());

        keys::leave_unlisted(&host.core, &[]).await;
        assert!(!host.open());
        kept.locked().refuses = None;
        host.read().await;
        assert!(host.open());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_let_in_the_server_did_not_take_trusts_no_host() {
    let kept = server();
    let founder = Host::founder(&kept).await;
    let host = Host::new(&kept, MEMBER, OrgRole::User).await;
    let code = host.code().await;
    kept.locked().refuses = Some(("/keys", 500));
    assert!(founder.let_in(&code).await.is_err());
    kept.locked().refuses = None;
    founder.read().await;
    assert_eq!(holders(&kept), [founder.me()]);

    let answer = founder.let_in(&code).await.unwrap();
    assert_eq!(answer.computer, Some(format!("{MEMBER}'s Mac")));
}
