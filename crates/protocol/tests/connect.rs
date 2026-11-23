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
        providers: Vec::new(),
        provider_replace: false,
    }
}

fn registration(name: &str) -> harmion_protocol::interpreter::ProviderRegistration {
    harmion_protocol::interpreter::ProviderRegistration {
        name: name.into(),
        contract_version: harmion_protocol::interpreter::CONTRACT_VERSION,
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        default_query: None,
    }
}

fn multi_offer(names: &[&str]) -> ConnectionOffer {
    let mut offer = offer();
    offer.provider = None;
    offer.grants.provide_names = names.iter().map(|name| (*name).into()).collect();
    offer.providers = names.iter().map(|name| registration(name)).collect();
    offer
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

#[test]
fn connection_offer_accepts_multiple_provider_manifests() {
    let offer = multi_offer(&["wiki/backlinks", "wiki/outline", "wiki/transclude"]);
    let bytes = serde_json::to_vec(&offer).unwrap();
    assert_eq!(decode_offer(&bytes).unwrap(), offer);
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value.get("provider").is_none());
    assert_eq!(value["providers"].as_array().unwrap().len(), 3);
    assert_eq!(offer.all_providers().count(), 3);

    let mut single = offer.clone();
    single.providers.clear();
    single.provider = Some(registration("wiki/backlinks"));
    single.grants.provide_names = vec!["wiki/backlinks".into()];
    let value = serde_json::to_value(&single).unwrap();
    assert_eq!(value["provider"]["name"], "wiki/backlinks");
    assert!(value.get("providers").is_none());

    let mut no_provider = single;
    no_provider.provider = None;
    no_provider.grants.provide_names = vec!["wiki/backlinks".into()];
    let value = serde_json::to_value(&no_provider).unwrap();
    assert!(value.get("provider").is_none());
    assert!(value.get("providers").is_none());
}

#[test]
fn connection_offer_rejects_both_provider_fields_duplicates_and_oversized_lists() {
    let mut both = multi_offer(&["wiki/backlinks", "wiki/outline"]);
    both.provider = Some(registration("wiki/backlinks"));
    assert!(matches!(
        decode_offer(&serde_json::to_vec(&both).unwrap()),
        Err(harmion_protocol::CodecError::InvalidFieldValue)
    ));

    let mut duplicate = multi_offer(&["wiki/backlinks", "wiki/backlinks"]);
    duplicate.grants.provide_names = vec!["wiki/backlinks".into()];
    assert!(matches!(
        decode_offer(&serde_json::to_vec(&duplicate).unwrap()),
        Err(harmion_protocol::CodecError::Duplicate)
    ));

    let names = (0..=MAX_CONNECTION_PROVIDERS)
        .map(|index| format!("wiki/p{index}"))
        .collect::<Vec<_>>();
    let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    let too_many = multi_offer(&name_refs);
    assert!(matches!(
        decode_offer(&serde_json::to_vec(&too_many).unwrap()),
        Err(harmion_protocol::CodecError::InvalidFieldValue)
    ));

    let mut bad_name = multi_offer(&["wiki/backlinks"]);
    bad_name.providers[0].name = "not/in/grants".into();
    assert!(decode_offer(&serde_json::to_vec(&bad_name).unwrap()).is_err());

    let mut bad_manifest = multi_offer(&["wiki/backlinks"]);
    bad_manifest.providers[0].contract_version = 2;
    assert!(matches!(
        decode_offer(&serde_json::to_vec(&bad_manifest).unwrap()),
        Err(harmion_protocol::CodecError::UnsupportedVersion(2))
    ));
}
