use harmion_protocol::GrantSpec;
use harmion_protocol::connect::*;
use serde_json::json;

fn offer() -> ConnectionOffer {
    ConnectionOffer {
        version: 1,
        name: "AntiWebUI".into(),
        redirect_uri: "http://127.0.0.1:43123/callback".into(),
        web_origin: Some("http://127.0.0.1:43123".into()),
        client_id: None,
        challenge: "A".repeat(43),
        state: "state".into(),
        grants: GrantSpec {
            read: Some(harmion_protocol::FilterSpec::All),
            write: None,
            call: true,
            provide: true,
            provide_names: vec!["ledger/balance".into()],
            manage_apps: false,
            manage_nodes: false,
        },
        provider: Some(harmion_protocol::interpreter::ProviderRegistration {
            name: "ledger/balance".into(),
            contract_version: 1,
            input_schema: json!({}),
            output_schema: json!({}),
            default_query: Some(harmion_protocol::interpreter::DefaultQuery {
                kinds: vec!["ledger".into()],
            }),
        }),
        provider_replace: false,
    }
}

#[test]
fn connection_offer_requires_loopback_origin_matching_redirect() {
    let bytes = serde_json::to_vec(&offer()).unwrap();
    assert_eq!(decode_offer(&bytes).unwrap(), offer());

    let mut bad = offer();
    bad.redirect_uri = "https://example.com/callback".into();
    assert!(decode_offer(&serde_json::to_vec(&bad).unwrap()).is_err());

    let mut bad = offer();
    bad.web_origin = Some("http://127.0.0.1:43124".into());
    assert!(decode_offer(&serde_json::to_vec(&bad).unwrap()).is_err());

    let mut bad = offer();
    bad.web_origin = Some("http://127.0.0.1:43123/path".into());
    assert!(decode_offer(&serde_json::to_vec(&bad).unwrap()).is_err());
}

#[test]
fn connection_offer_rejects_management_grants_and_unmatched_provider_names() {
    let mut bad = offer();
    bad.grants.manage_apps = true;
    assert!(decode_offer(&serde_json::to_vec(&bad).unwrap()).is_err());

    let mut bad = offer();
    bad.grants.provide_names = vec!["other/name".into()];
    assert!(decode_offer(&serde_json::to_vec(&bad).unwrap()).is_err());

    let mut bad = offer();
    bad.grants.provide = false;
    bad.grants.provide_names.clear();
    bad.provider = None;
    bad.grants.read = None;
    bad.grants.call = false;
    assert!(decode_offer(&serde_json::to_vec(&bad).unwrap()).is_err());

    let mut bad = offer();
    bad.challenge = "short".into();
    assert!(decode_offer(&serde_json::to_vec(&bad).unwrap()).is_err());
    bad.challenge = "A".repeat(44);
    assert!(decode_offer(&serde_json::to_vec(&bad).unwrap()).is_err());
}
