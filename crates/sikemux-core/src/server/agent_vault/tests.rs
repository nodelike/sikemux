#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use serde_json::json;
use sikemux_calls::vault::{
    VaultApprovalAnswerParams, VaultApprovalsParams, VaultAuditEntry, VaultAuditLogParams,
    VaultAuditStatus, VaultCategory, VaultDeliveryMode, VaultDocumentInput, VaultFieldInput,
    VaultFieldKind, VaultGetParams, VaultListParams, VaultRequestCancelParams,
    VaultRequestSubmitParams, VaultRequestsParams, VaultResolveBrowserParams, VaultResolvedBrowser,
    VaultScope, VaultSource, VaultUpsertParams,
};
use sikemux_process::sync::Locked;
use sikemux_wire::protocol::BuildIdentity;

use super::crypto::{
    generate_totp, open, parse_op_ref, parse_vault_ref, random_key, seal, ParsedOpRef,
    ParsedVaultRef,
};
use super::store::VaultStore;
use super::{
    answer_approval, approvals, audit_log, cancel_request, delete, get, list, requests,
    resolve_for_browser, submit_request, tool_list, tool_request, upsert,
};
use crate::cli::protocol::HarnessRequest;
use crate::server::access::Peer;
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use crate::server::{Core, ServerCore};

fn test_core(data_dir: Option<&std::path::Path>) -> Arc<Core> {
    let core = Core::new(BuildIdentity::default(), data_dir).expect("core");
    let key = random_key().expect("key");
    *core.agent_vault().store.locked() = VaultStore::new(data_dir, Some(key));
    core.remote().answer_window_with(true);
    core
}

#[test]
fn encrypted_round_trip_for_details_secrets_payments_and_documents() {
    let dir = tempfile::tempdir().expect("tempdir");
    let key = random_key().expect("key");
    let mut store = VaultStore::new(Some(dir.path()), Some(key));

    // 1. Details (including sensitive PAN/SSN)
    let details = store
        .upsert(
            VaultUpsertParams {
                id: None,
                title: "Alex Rivera Profile".into(),
                category: VaultCategory::Details,
                scope: VaultScope::Personal,
                source: VaultSource::Sikemux,
                delivery_mode: None,
                domains: vec!["billing.acme-cloud.com".into()],
                require_approval: false,
                fields: vec![
                    VaultFieldInput {
                        key: "full_name".into(),
                        label: "Full name".into(),
                        kind: VaultFieldKind::Text,
                        sensitive: false,
                        value: Some("Alex Rivera".into()),
                        op_ref: None,
                    },
                    VaultFieldInput {
                        key: "pan".into(),
                        label: "PAN".into(),
                        kind: VaultFieldKind::TaxId,
                        sensitive: true,
                        value: Some("ABCDE1234F".into()),
                        op_ref: None,
                    },
                ],
                document: None,
                subtitle: None,
            },
            "person",
        )
        .expect("upsert details");

    // 2. Secrets (native + 1Password op:// reference)
    let secret = store
        .upsert(
            VaultUpsertParams {
                id: None,
                title: "Cloud Portal Login".into(),
                category: VaultCategory::Secrets,
                scope: VaultScope::Personal,
                source: VaultSource::Sikemux,
                delivery_mode: Some(VaultDeliveryMode::BlindFill),
                domains: vec!["portal.cloud.example".into()],
                require_approval: false,
                fields: vec![
                    VaultFieldInput {
                        key: "username".into(),
                        label: "Username".into(),
                        kind: VaultFieldKind::Email,
                        sensitive: false,
                        value: Some("alex@acme-studio.co".into()),
                        op_ref: None,
                    },
                    VaultFieldInput {
                        key: "password".into(),
                        label: "Password".into(),
                        kind: VaultFieldKind::Password,
                        sensitive: true,
                        value: Some("s3cr3t-p@ssw0rd-999!".into()),
                        op_ref: None,
                    },
                    VaultFieldInput {
                        key: "api_key".into(),
                        label: "1Password API Key".into(),
                        kind: VaultFieldKind::Secret,
                        sensitive: true,
                        value: None,
                        op_ref: Some("op://Work/CloudPortal/api_key".into()),
                    },
                ],
                document: None,
                subtitle: None,
            },
            "person",
        )
        .expect("upsert secret");

    // 3. Payments
    let payment = store
        .upsert(
            VaultUpsertParams {
                id: None,
                title: "Chase Sapphire Visa".into(),
                category: VaultCategory::Payments,
                scope: VaultScope::Org {
                    org_id: "org_acme".into(),
                    org_name: Some("Acme Studio".into()),
                },
                source: VaultSource::Sikemux,
                delivery_mode: None,
                domains: vec![],
                require_approval: false,
                fields: vec![
                    VaultFieldInput {
                        key: "cardholder".into(),
                        label: "Cardholder".into(),
                        kind: VaultFieldKind::Text,
                        sensitive: false,
                        value: Some("Alex Rivera".into()),
                        op_ref: None,
                    },
                    VaultFieldInput {
                        key: "card_number".into(),
                        label: "Card number".into(),
                        kind: VaultFieldKind::CardNumber,
                        sensitive: true,
                        value: Some("4242 4242 4242 4242".into()),
                        op_ref: None,
                    },
                    VaultFieldInput {
                        key: "expiry".into(),
                        label: "Expiry".into(),
                        kind: VaultFieldKind::Expiry,
                        sensitive: true,
                        value: Some("08/29".into()),
                        op_ref: None,
                    },
                    VaultFieldInput {
                        key: "cvc".into(),
                        label: "CVC".into(),
                        kind: VaultFieldKind::Cvc,
                        sensitive: true,
                        value: Some("842".into()),
                        op_ref: None,
                    },
                    VaultFieldInput {
                        key: "billing_address".into(),
                        label: "Billing address".into(),
                        kind: VaultFieldKind::Address,
                        sensitive: false,
                        value: Some("548 Market St, SF".into()),
                        op_ref: None,
                    },
                ],
                document: None,
                subtitle: None,
            },
            "person",
        )
        .expect("upsert payment");
    assert!(payment.require_approval, "payments always require approval");

    // 4. Documents
    let pdf_bytes = b"%PDF-1.7\nSECRET-PASSPORT-NUMBER-X94820144\n%%EOF";
    let doc = store
        .upsert(
            VaultUpsertParams {
                id: None,
                title: "Passport Scan".into(),
                category: VaultCategory::Documents,
                scope: VaultScope::Personal,
                source: VaultSource::Sikemux,
                delivery_mode: None,
                domains: vec!["kyc.example.com".into()],
                require_approval: true,
                fields: vec![],
                document: Some(VaultDocumentInput {
                    filename: "passport-verification.pdf".into(),
                    mime_type: Some("application/pdf".into()),
                    source_path: None,
                    data_base64: Some(BASE64_STANDARD.encode(pdf_bytes)),
                }),
                subtitle: None,
            },
            "person",
        )
        .expect("upsert document");
    assert_eq!(doc.delivery_mode, VaultDeliveryMode::PrivateTempFile);

    // Verify plaintext NEVER appears on disk in entries.enc, docs/<id>.enc, or audit.json.
    let entries_disk = std::fs::read(dir.path().join("agent-vault/entries.enc")).expect("enc file");
    let entries_str = String::from_utf8_lossy(&entries_disk);
    for forbidden in [
        "s3cr3t-p@ssw0rd-999!",
        "4242 4242 4242 4242",
        "ABCDE1234F",
        "Alex Rivera",
    ] {
        assert!(
            !entries_str.contains(forbidden),
            "plaintext '{forbidden}' leaked into entries.enc"
        );
    }
    let doc_disk = std::fs::read(dir.path().join(format!("agent-vault/docs/{}.enc", doc.id)))
        .expect("doc enc");
    assert!(
        !String::from_utf8_lossy(&doc_disk).contains("X94820144"),
        "document plaintext leaked into docs/<id>.enc"
    );
    let audit_disk =
        std::fs::read_to_string(dir.path().join("agent-vault/audit.json")).expect("audit");
    for forbidden in ["s3cr3t-p@ssw0rd-999!", "4242 4242 4242 4242", "ABCDE1234F"] {
        assert!(
            !audit_disk.contains(forbidden),
            "plaintext '{forbidden}' leaked into audit.json"
        );
    }
    let audit_entries: Vec<VaultAuditEntry> =
        serde_json::from_str(&audit_disk).expect("parse audit.json");
    for entry in &audit_entries {
        for text in [
            Some(entry.entry_title.as_str()),
            Some(entry.actor.as_str()),
            entry.domain.as_deref(),
            Some(entry.tool.as_str()),
        ]
        .into_iter()
        .flatten()
        .chain(entry.field_keys.iter().map(String::as_str))
        {
            assert!(
                !text.contains("842"),
                "CVC '842' leaked into audit entry text: {text}"
            );
        }
    }

    // Reopen with the same key and verify encrypted round-trip.
    let mut reopened = VaultStore::new(Some(dir.path()), Some(key));
    let all = reopened.list(&VaultListParams::default()).expect("list");
    assert_eq!(all.len(), 4);

    let revealed_details = reopened.get(&details.id, true).expect("get details");
    assert_eq!(
        revealed_details
            .fields
            .iter()
            .find(|f| f.key == "pan")
            .and_then(|f| f.value.as_deref()),
        Some("ABCDE1234F")
    );
    let masked_details = reopened.get(&details.id, false).expect("masked details");
    assert_eq!(
        masked_details
            .fields
            .iter()
            .find(|f| f.key == "pan")
            .and_then(|f| f.value.as_deref()),
        None
    );

    let revealed_secret = reopened.get(&secret.id, true).expect("get secret");
    assert_eq!(
        revealed_secret
            .fields
            .iter()
            .find(|f| f.key == "password")
            .and_then(|f| f.value.as_deref()),
        Some("s3cr3t-p@ssw0rd-999!")
    );
    assert_eq!(
        revealed_secret
            .fields
            .iter()
            .find(|f| f.key == "api_key")
            .and_then(|f| f.op_ref.as_deref()),
        Some("op://Work/CloudPortal/api_key")
    );

    let revealed_payment = reopened.get(&payment.id, true).expect("get payment");
    assert_eq!(
        revealed_payment
            .fields
            .iter()
            .find(|f| f.key == "card_number")
            .and_then(|f| f.value.as_deref()),
        Some("4242 4242 4242 4242")
    );
    assert_eq!(
        revealed_payment
            .fields
            .iter()
            .find(|f| f.key == "cvc")
            .and_then(|f| f.value.as_deref()),
        Some("842")
    );

    let decrypted_doc = reopened.read_document_bytes(&doc.id).expect("read doc");
    assert_eq!(decrypted_doc, pdf_bytes);

    // Wrong key MUST fail decryption.
    let wrong_key = random_key().expect("wrong key");
    let mut bad_store = VaultStore::new(Some(dir.path()), Some(wrong_key));
    assert!(bad_store.list(&VaultListParams::default()).is_err());

    // Tampered ciphertext MUST fail AEAD verification.
    let sealed = seal(&key, "aad-test", b"secret-bytes").expect("seal");
    assert_eq!(
        open(&key, "aad-test", &sealed).expect("open"),
        b"secret-bytes"
    );
    assert!(open(&key, "wrong-aad", &sealed).is_err());
    let mut tampered = sealed;
    if let Some(last) = tampered.last_mut() {
        *last ^= 0xff;
    }
    assert!(open(&key, "aad-test", &tampered).is_err());
}

#[tokio::test]
async fn vault_list_and_vault_request_never_leak_plaintext() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = test_core(Some(dir.path()));

    let _saved = upsert(
        &core,
        VaultUpsertParams {
            id: None,
            title: "Production Stripe Key".into(),
            category: VaultCategory::Secrets,
            scope: VaultScope::Personal,
            source: VaultSource::Sikemux,
            delivery_mode: None,
            domains: vec!["dashboard.stripe.com".into()],
            require_approval: false,
            fields: vec![VaultFieldInput {
                key: "api_key".into(),
                label: "Secret Key".into(),
                kind: VaultFieldKind::Secret,
                sensitive: true,
                value: Some("sk_live_SUPERSECRET_99887766".into()),
                op_ref: None,
            }],
            document: None,
            subtitle: None,
        },
    )
    .expect("upsert");

    let _card = upsert(
        &core,
        VaultUpsertParams {
            id: None,
            title: "Corporate Amex".into(),
            category: VaultCategory::Payments,
            scope: VaultScope::Personal,
            source: VaultSource::Sikemux,
            delivery_mode: None,
            domains: vec![],
            require_approval: true,
            fields: vec![
                VaultFieldInput {
                    key: "card_number".into(),
                    label: "Card number".into(),
                    kind: VaultFieldKind::CardNumber,
                    sensitive: true,
                    value: Some("3782 822463 10005".into()),
                    op_ref: None,
                },
                VaultFieldInput {
                    key: "cvc".into(),
                    label: "CVC".into(),
                    kind: VaultFieldKind::Cvc,
                    sensitive: true,
                    value: Some("9182".into()),
                    op_ref: None,
                },
            ],
            document: None,
            subtitle: None,
        },
    )
    .expect("upsert card");

    // Agent calls vault_list -> verify zero plaintext values appear anywhere in JSON.
    let list_res = tool_list(
        &core,
        &HarnessRequest {
            id: "req-list".into(),
            method: "vault.list".into(),
            params: json!({}),
            project: dir.path().to_string_lossy().into_owned(),
            agent_id: Some("agent-1".into()),
        },
    )
    .expect("tool_list");
    let serialized = serde_json::to_string(&list_res).expect("json");
    for secret in [
        "sk_live_SUPERSECRET_99887766",
        "3782 822463 10005",
        "378282246310005",
        "9182",
    ] {
        assert!(
            !serialized.contains(secret),
            "vault_list leaked secret '{secret}': {serialized}"
        );
    }
    assert_eq!(list_res["count"], json!(2));

    // Agent calls vault_request -> person submits via vaultRequestSubmit -> agent only receives vault:<id> + field keys.
    let core_for_agent = Arc::clone(&core);
    let project = dir.path().to_string_lossy().into_owned();
    let agent_task = tokio::spawn(async move {
        tool_request(
            &core_for_agent,
            &HarnessRequest {
                id: "req-1".into(),
                method: "vault.request".into(),
                params: json!({
                    "category": "secrets",
                    "title": "GitHub Login",
                    "domain": "github.com",
                    "reason": "Sign in to check CI",
                    "fields": [
                        { "key": "username", "label": "Username" },
                        { "key": "password", "label": "Password", "kind": "password", "sensitive": true }
                    ],
                    "waitMs": 5000
                }),
                project,
                agent_id: Some("agent-1".into()),
            },
        )
        .await
    });

    // Wait briefly until request is registered.
    let pending = loop {
        let open = requests(
            &core,
            VaultRequestsParams {
                agent_id: Some("agent-1".into()),
            },
        );
        if let Some(first) = open.into_iter().next() {
            break first;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };

    let created = submit_request(
        &core,
        VaultRequestSubmitParams {
            request_id: pending.id,
            existing_entry_id: None,
            title: Some("GitHub Login".into()),
            scope: Some(VaultScope::Personal),
            source: VaultSource::Sikemux,
            domains: vec!["github.com".into()],
            require_approval: false,
            fields: vec![
                VaultFieldInput {
                    key: "username".into(),
                    label: "Username".into(),
                    kind: VaultFieldKind::Text,
                    sensitive: false,
                    value: Some("octocat".into()),
                    op_ref: None,
                },
                VaultFieldInput {
                    key: "password".into(),
                    label: "Password".into(),
                    kind: VaultFieldKind::Password,
                    sensitive: true,
                    value: Some("gh_top_secret_pw_555".into()),
                    op_ref: None,
                },
            ],
            document: None,
            approve_domain: true,
            ephemeral: false,
        },
    )
    .await
    .expect("submit_request");

    let agent_answer = agent_task.await.expect("join").expect("tool_request");
    let answer_json = serde_json::to_string(&agent_answer).expect("json");
    assert_eq!(agent_answer["status"], json!("saved"));
    assert_eq!(
        agent_answer["reference"],
        json!(format!("vault:{}", created.id))
    );
    assert!(
        !answer_json.contains("gh_top_secret_pw_555") && !answer_json.contains("octocat"),
        "vault_request leaked entered values: {answer_json}"
    );

    // Also test cancel flow.
    let core_cancel = Arc::clone(&core);
    let project = dir.path().to_string_lossy().into_owned();
    let cancel_task = tokio::spawn(async move {
        tool_request(
            &core_cancel,
            &HarnessRequest {
                id: "req-2".into(),
                method: "vault.request".into(),
                params: json!({
                    "category": "payments",
                    "title": "Checkout Card",
                    "waitMs": 5000
                }),
                project,
                agent_id: Some("agent-2".into()),
            },
        )
        .await
    });
    let pending2 = loop {
        let open = requests(
            &core,
            VaultRequestsParams {
                agent_id: Some("agent-2".into()),
            },
        );
        if let Some(first) = open.into_iter().next() {
            break first;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    cancel_request(
        &core,
        VaultRequestCancelParams {
            request_id: pending2.id,
            reason: Some("Not now".into()),
        },
    )
    .await
    .expect("cancel");
    let declined = cancel_task.await.expect("join").expect("declined");
    assert_eq!(declined["status"], json!("declined"));

    // Verify human UI CRUD helpers (`list`, `get`, `delete`, `audit_log`).
    assert_eq!(
        list(&core, VaultListParams::default()).expect("list").len(),
        3
    );
    let human_detail = get(
        &core,
        VaultGetParams {
            id: created.id.clone(),
            reveal: true,
            resolve_external: false,
        },
    )
    .await
    .expect("human get");
    assert!(human_detail.revealed);
    delete(
        &core,
        sikemux_calls::vault::VaultDeleteParams {
            id: created.id.clone(),
        },
    )
    .expect("delete");
    let logs = audit_log(&core, VaultAuditLogParams::default()).expect("audit");
    assert!(logs
        .iter()
        .any(|row| row.status == VaultAuditStatus::Revealed));
    assert!(logs
        .iter()
        .any(|row| row.status == VaultAuditStatus::Deleted));
}

#[tokio::test]
async fn blind_fill_resolution_payment_and_first_time_secret_approval_gating() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = test_core(Some(dir.path()));

    // 1. Secret entry with password + TOTP seed
    let secret = upsert(
        &core,
        VaultUpsertParams {
            id: None,
            title: "Portal Login".into(),
            category: VaultCategory::Secrets,
            scope: VaultScope::Personal,
            source: VaultSource::Sikemux,
            delivery_mode: None,
            domains: vec!["portal.cloud.example".into()],
            require_approval: false,
            fields: vec![
                VaultFieldInput {
                    key: "password".into(),
                    label: "Password".into(),
                    kind: VaultFieldKind::Password,
                    sensitive: true,
                    value: Some("hunter2-secret".into()),
                    op_ref: None,
                },
                VaultFieldInput {
                    key: "totp".into(),
                    label: "2FA".into(),
                    kind: VaultFieldKind::Totp,
                    sensitive: true,
                    value: Some("JBSWY3DPEHPK3PXP".into()),
                    op_ref: None,
                },
            ],
            document: None,
            subtitle: None,
        },
    )
    .expect("upsert secret");

    // First-time fill on portal.cloud.example MUST trigger approval!
    let core_fill = Arc::clone(&core);
    let secret_ref = format!("vault:{}.password", secret.id);
    let fill_task = tokio::spawn(async move {
        resolve_for_browser(
            &core_fill,
            VaultResolveBrowserParams {
                agent_id: "agent-1".into(),
                reference: secret_ref,
                domain: "https://portal.cloud.example/login".into(),
                tool: "browser.type".into(),
                for_upload: false,
            },
        )
        .await
    });

    let pending_secret = loop {
        let open = approvals(
            &core,
            VaultApprovalsParams {
                agent_id: Some("agent-1".into()),
            },
        );
        if let Some(first) = open.into_iter().next() {
            break first;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    assert_eq!(pending_secret.domain, "portal.cloud.example");
    assert!(!pending_secret.requires_biometric);

    answer_approval(
        &core,
        &Peer::App,
        VaultApprovalAnswerParams {
            id: pending_secret.id,
            approved: true,
            remember_domain: true,
            ephemeral_values: BTreeMap::new(),
        },
    )
    .await
    .expect("approve secret");

    let resolved = fill_task.await.expect("join").expect("resolved secret");
    match resolved {
        VaultResolvedBrowser::Text {
            value,
            masked_label,
            redaction_tokens,
            sensitive,
            ..
        } => {
            assert_eq!(value, "hunter2-secret");
            assert!(masked_label.contains('•'));
            assert!(redaction_tokens.contains(&"hunter2-secret".to_owned()));
            assert!(sensitive);
        }
        VaultResolvedBrowser::File { .. } => panic!("expected text payload"),
    }

    // Second fill on the SAME approved domain resolves immediately without prompting (including live TOTP)!
    let totp_resolved = resolve_for_browser(
        &core,
        VaultResolveBrowserParams {
            agent_id: "agent-1".into(),
            reference: format!("vault:{}.totp", secret.id),
            domain: "portal.cloud.example".into(),
            tool: "browser.type".into(),
            for_upload: false,
        },
    )
    .await
    .expect("totp fill without second prompt");
    match totp_resolved {
        VaultResolvedBrowser::Text {
            value, sensitive, ..
        } => {
            assert_eq!(value.len(), 6);
            assert!(value.chars().all(|c| c.is_ascii_digit()));
            assert!(sensitive);
        }
        VaultResolvedBrowser::File { .. } => panic!("expected text payload"),
    }

    // 2. Payment card ALWAYS triggers approval (and requires biometric) on EVERY fill!
    let card = upsert(
        &core,
        VaultUpsertParams {
            id: None,
            title: "Sapphire Visa".into(),
            category: VaultCategory::Payments,
            scope: VaultScope::Personal,
            source: VaultSource::Sikemux,
            delivery_mode: None,
            domains: vec!["billing.acme-cloud.com".into()],
            require_approval: false,
            fields: vec![
                VaultFieldInput {
                    key: "card_number".into(),
                    label: "Card number".into(),
                    kind: VaultFieldKind::CardNumber,
                    sensitive: true,
                    value: Some("4242 4242 4242 4242".into()),
                    op_ref: None,
                },
                VaultFieldInput {
                    key: "cvc".into(),
                    label: "CVC".into(),
                    kind: VaultFieldKind::Cvc,
                    sensitive: true,
                    value: None,
                    op_ref: None,
                },
            ],
            document: None,
            subtitle: None,
        },
    )
    .expect("upsert card");

    // Payment fill #1: approved with remember_domain=true (must NOT cache for payments!) + ephemeral CVC.
    let core_pay1 = Arc::clone(&core);
    let cvc_ref = format!("vault:{}.cvc", card.id);
    let pay_task1 = tokio::spawn(async move {
        resolve_for_browser(
            &core_pay1,
            VaultResolveBrowserParams {
                agent_id: "agent-1".into(),
                reference: cvc_ref,
                domain: "billing.acme-cloud.com".into(),
                tool: "browser.type".into(),
                for_upload: false,
            },
        )
        .await
    });
    let pending_pay1 = loop {
        let open = approvals(&core, VaultApprovalsParams::default());
        if let Some(first) = open.into_iter().next() {
            break first;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    assert!(pending_pay1.requires_biometric);
    let mut ephemeral = BTreeMap::new();
    ephemeral.insert("cvc".into(), "739".into());
    answer_approval(
        &core,
        &Peer::App,
        VaultApprovalAnswerParams {
            id: pending_pay1.id,
            approved: true,
            remember_domain: true,
            ephemeral_values: ephemeral,
        },
    )
    .await
    .expect("approve payment 1");

    let pay1 = pay_task1.await.expect("join").expect("pay1");
    match pay1 {
        VaultResolvedBrowser::Text { value, .. } => assert_eq!(value, "739"),
        VaultResolvedBrowser::File { .. } => panic!("expected text"),
    }

    // Payment fill #2 on the EXACT same domain MUST STILL require approval (and declining blocks it)!
    let core_pay2 = Arc::clone(&core);
    let num_ref = format!("vault:{}.card_number", card.id);
    let pay_task2 = tokio::spawn(async move {
        resolve_for_browser(
            &core_pay2,
            VaultResolveBrowserParams {
                agent_id: "agent-1".into(),
                reference: num_ref,
                domain: "billing.acme-cloud.com".into(),
                tool: "browser.type".into(),
                for_upload: false,
            },
        )
        .await
    });
    let pending_pay2 = loop {
        let open = approvals(&core, VaultApprovalsParams::default());
        if let Some(first) = open.into_iter().next() {
            break first;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    assert!(pending_pay2.requires_biometric);
    answer_approval(
        &core,
        &Peer::App,
        VaultApprovalAnswerParams {
            id: pending_pay2.id,
            approved: false,
            remember_domain: false,
            ephemeral_values: BTreeMap::new(),
        },
    )
    .await
    .expect("decline payment 2");
    assert!(pay_task2.await.expect("join").is_err());
}

#[test]
fn vault_ref_op_ref_and_totp_parsing() {
    assert_eq!(
        parse_vault_ref("vault:vlt_abc123.password"),
        Some(ParsedVaultRef {
            entry_id: "vlt_abc123".into(),
            field_key: Some("password".into()),
        })
    );
    assert_eq!(
        parse_vault_ref("vault:vlt_doc1"),
        Some(ParsedVaultRef {
            entry_id: "vlt_doc1".into(),
            field_key: None,
        })
    );
    assert_eq!(parse_vault_ref("vault:../etc/passwd"), None);
    assert_eq!(parse_vault_ref("plain text"), None);

    assert_eq!(
        parse_op_ref("op://Personal/Cloud Portal/password").expect("3-part op ref"),
        ParsedOpRef {
            vault: "Personal".into(),
            item: "Cloud Portal".into(),
            section: None,
            field: "password".into(),
        }
    );
    assert_eq!(
        parse_op_ref("op://Work/AWS/Production/secret_key").expect("4-part op ref"),
        ParsedOpRef {
            vault: "Work".into(),
            item: "AWS".into(),
            section: Some("Production".into()),
            field: "secret_key".into(),
        }
    );
    assert!(parse_op_ref("https://1password.com").is_err());
    assert!(parse_op_ref("op://OnlyVault/NoField").is_err());
    assert!(parse_op_ref("op://Vault//field").is_err());

    // RFC 6238 test vector (SHA-1, ASCII "12345678901234567890" -> GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ, t=59 -> 287082)
    let code = generate_totp("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ", 59).expect("totp");
    assert_eq!(code, "287082");
    let from_uri = generate_totp(
        "otpauth://totp/Example:alex@acme.co?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Example",
        59,
    )
    .expect("otpauth totp");
    assert_eq!(from_uri, "287082");
}

#[tokio::test]
async fn prefilled_document_and_details_vault_request_with_security_guards() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = test_core(Some(dir.path()));
    let project = dir.path().to_string_lossy().into_owned();

    // Create a valid PDF file to pre-attach.
    let pdf_path = dir.path().join("UDYAM-KR-03-0795566-Nodelike.pdf");
    let pdf_bytes = b"%PDF-1.7\nUDYAM-KR-03-0795566-BODY\n%%EOF";
    std::fs::write(&pdf_path, pdf_bytes).expect("write pdf");

    // Seed an existing document with the same title so we prove submit creates a new entry, never overwrites.
    let existing = upsert(
        &core,
        VaultUpsertParams {
            id: None,
            title: "Nodelike Udyam Certificate".into(),
            category: VaultCategory::Documents,
            scope: VaultScope::Personal,
            source: VaultSource::Sikemux,
            delivery_mode: None,
            domains: vec![],
            require_approval: true,
            fields: vec![],
            document: Some(VaultDocumentInput {
                filename: "old.pdf".into(),
                mime_type: Some("application/pdf".into()),
                source_path: None,
                data_base64: Some(BASE64_STANDARD.encode(b"%PDF-1.4\nOLD\n%%EOF")),
            }),
            subtitle: None,
        },
    )
    .expect("seed existing");

    // 1. Prefilled document filePath + prefilled ID number (`document_number`).
    let core_doc = Arc::clone(&core);
    let project_doc = project.clone();
    let pdf_arg = pdf_path.to_string_lossy().into_owned();
    let doc_task = tokio::spawn(async move {
        tool_request(
            &core_doc,
            &HarnessRequest {
                id: "req-prefill-doc".into(),
                method: "vault.request".into(),
                params: json!({
                    "category": "documents",
                    "title": "Nodelike Udyam Certificate",
                    "filePath": pdf_arg,
                    "fields": [
                        { "key": "document_number", "label": "ID number", "value": "UDYAM-KR-03-0795566", "required": false }
                    ],
                    "waitMs": 5000
                }),
                project: project_doc,
                agent_id: Some("agent-prefill-doc".into()),
            },
        )
        .await
    });

    let pending_doc = loop {
        let open = requests(
            &core,
            VaultRequestsParams {
                agent_id: Some("agent-prefill-doc".into()),
            },
        );
        if let Some(first) = open.into_iter().next() {
            break first;
        }
        if doc_task.is_finished() {
            panic!("doc_task exited early: {:?}", doc_task.await);
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };

    let proposed = pending_doc
        .proposed_document
        .as_ref()
        .expect("proposed_document populated");
    assert_eq!(proposed.filename, "UDYAM-KR-03-0795566-Nodelike.pdf");
    assert_eq!(proposed.mime_type, "application/pdf");
    assert_eq!(proposed.byte_len, pdf_bytes.len() as u64);
    let doc_num_field = pending_doc
        .fields
        .iter()
        .find(|f| f.key == "document_number")
        .expect("document_number field");
    assert_eq!(doc_num_field.kind, VaultFieldKind::TaxId);
    assert_eq!(doc_num_field.value.as_deref(), Some("UDYAM-KR-03-0795566"));

    // Submit with document: None -> host reads validated proposed_document directly into encrypted Vault.
    let saved_doc = submit_request(
        &core,
        VaultRequestSubmitParams {
            request_id: pending_doc.id,
            existing_entry_id: None,
            title: None,
            scope: None,
            source: VaultSource::Sikemux,
            domains: vec![],
            require_approval: true,
            fields: vec![VaultFieldInput {
                key: "document_number".into(),
                label: "ID number".into(),
                kind: VaultFieldKind::TaxId,
                sensitive: true,
                value: Some("UDYAM-KR-03-0795566".into()),
                op_ref: None,
            }],
            document: None,
            approve_domain: false,
            ephemeral: false,
        },
    )
    .await
    .expect("submit prefilled doc");

    assert_ne!(
        saved_doc.id, existing.id,
        "must never overwrite existing entry"
    );
    assert_eq!(
        saved_doc.document.as_ref().map(|d| d.filename.as_str()),
        Some("UDYAM-KR-03-0795566-Nodelike.pdf")
    );
    let stored_bytes = core
        .agent_vault()
        .store
        .locked()
        .read_document_bytes(&saved_doc.id)
        .expect("decrypt stored doc");
    assert_eq!(stored_bytes, pdf_bytes);
    let doc_answer = doc_task.await.expect("join").expect("doc_task");
    assert_eq!(doc_answer["status"], json!("saved"));
    assert_eq!(
        doc_answer["reference"],
        json!(format!("vault:{}", saved_doc.id))
    );
    assert!(!serde_json::to_string(&doc_answer)
        .expect("json")
        .contains("UDYAM-KR-03-0795566-BODY"));

    // 2. Prefilled non-sensitive details fields.
    let core_det = Arc::clone(&core);
    let project_det = project.clone();
    let det_task = tokio::spawn(async move {
        tool_request(
            &core_det,
            &HarnessRequest {
                id: "req-prefill-details".into(),
                method: "vault.request".into(),
                params: json!({
                    "category": "details",
                    "title": "Nodelike Business Details",
                    "fields": [
                        { "key": "company_name", "label": "Company", "value": "Nodelike" },
                        { "key": "email", "label": "Work Email", "value": "kishore.gunalan@sikemux.com" }
                    ],
                    "waitMs": 5000
                }),
                project: project_det,
                agent_id: Some("agent-prefill-det".into()),
            },
        )
        .await
    });

    let pending_det = loop {
        let open = requests(
            &core,
            VaultRequestsParams {
                agent_id: Some("agent-prefill-det".into()),
            },
        );
        if let Some(first) = open.into_iter().next() {
            break first;
        }
        if det_task.is_finished() {
            panic!("det_task exited early: {:?}", det_task.await);
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    assert_eq!(pending_det.fields[0].value.as_deref(), Some("Nodelike"));
    assert_eq!(
        pending_det.fields[1].value.as_deref(),
        Some("kishore.gunalan@sikemux.com")
    );
    cancel_request(
        &core,
        VaultRequestCancelParams {
            request_id: pending_det.id,
            reason: None,
        },
    )
    .await
    .expect("cancel det");
    let _ = det_task.await.expect("join").expect("det_task");

    // 3. Security guards: reject invalid filePaths and sensitive prefills.
    let call_req = |mut params: serde_json::Value| {
        if let Some(obj) = params.as_object_mut() {
            obj.entry("waitMs".to_owned()).or_insert(json!(200));
        }
        let core = Arc::clone(&core);
        let project = project.clone();
        async move {
            tool_request(
                &core,
                &HarnessRequest {
                    id: "req-guard".into(),
                    method: "vault.request".into(),
                    params,
                    project,
                    agent_id: Some("agent-guard".into()),
                },
            )
            .await
        }
    };

    // filePath on non-documents category
    assert!(call_req(json!({
        "category": "details",
        "title": "Bad",
        "filePath": pdf_path.to_string_lossy()
    }))
    .await
    .is_err());

    // Relative filePath
    assert!(call_req(json!({
        "category": "documents",
        "title": "Relative",
        "filePath": "relative/doc.pdf"
    }))
    .await
    .is_err());

    // Non-existent filePath
    assert!(call_req(json!({
        "category": "documents",
        "title": "Missing",
        "filePath": dir.path().join("nope.pdf").to_string_lossy()
    }))
    .await
    .is_err());

    // Directory path
    assert!(call_req(json!({
        "category": "documents",
        "title": "Dir",
        "filePath": dir.path().to_string_lossy()
    }))
    .await
    .is_err());

    // Empty 0-byte file
    let empty_path = dir.path().join("empty.pdf");
    std::fs::write(&empty_path, b"").expect("write empty");
    assert!(call_req(json!({
        "category": "documents",
        "title": "Empty",
        "filePath": empty_path.to_string_lossy()
    }))
    .await
    .is_err());

    // Oversized file (> 25 MB)
    let huge_path = dir.path().join("huge.pdf");
    let huge = std::fs::File::create(&huge_path).expect("create huge");
    huge.set_len(26 * 1024 * 1024).expect("set_len");
    assert!(call_req(json!({
        "category": "documents",
        "title": "Huge",
        "filePath": huge_path.to_string_lossy()
    }))
    .await
    .is_err());

    // Symbolic link
    #[cfg(unix)]
    {
        let link_path = dir.path().join("link.pdf");
        std::os::unix::fs::symlink(&pdf_path, &link_path).expect("symlink");
        assert!(call_req(json!({
            "category": "documents",
            "title": "Symlink",
            "filePath": link_path.to_string_lossy()
        }))
        .await
        .is_err());
    }

    // Prefilled value on secrets
    assert!(call_req(json!({
        "category": "secrets",
        "title": "Secret",
        "fields": [{ "key": "password", "value": "hunter2" }]
    }))
    .await
    .is_err());

    // Prefilled value on payments
    assert!(call_req(json!({
        "category": "payments",
        "title": "Card",
        "fields": [{ "key": "card_number", "value": "4242 4242 4242 4242" }]
    }))
    .await
    .is_err());

    // Prefilled value on sensitive details field
    assert!(call_req(json!({
        "category": "details",
        "title": "Sensitive detail",
        "fields": [{ "key": "pan", "value": "ABCDE1234F" }]
    }))
    .await
    .is_err());

    // Prefilled secret field under documents
    assert!(call_req(json!({
        "category": "documents",
        "title": "Doc with password",
        "fields": [{ "key": "password", "value": "secret-pw" }]
    }))
    .await
    .is_err());
}
