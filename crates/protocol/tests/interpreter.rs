use harmion_protocol::interpreter::*;
use serde_json::json;

fn registration(name: &str) -> ProviderRegistration {
    ProviderRegistration {
        name: name.into(),
        contract_version: CONTRACT_VERSION,
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        default_query: Some(DefaultQuery {
            kinds: vec!["ledger".into()],
        }),
    }
}

#[test]
fn reserved_serde_json_markers_are_rejected_not_reinterpreted() {
    // `serde_json::from_value` re-classifies an object with this first key as a
    // number, which would make the stored schema differ from the validated
    // bytes. The typed interpreter decoders reject the marker instead.
    let marker = r#"{"$serde_json::private::Number":"123"}"#;
    let registration = format!(
        r#"{{"name":"ledger/balance","contract_version":1,"input_schema":{marker},"output_schema":{{}}}}"#
    );
    assert!(decode_registration(registration.as_bytes()).is_err());
    let call = format!(r#"{{"provider":"ledger/balance","input":{marker},"contents":[]}}"#);
    assert!(decode_call_request(call.as_bytes()).is_err());

    let normal = r#"{"name":"ledger/balance","contract_version":1,"input_schema":{"type":"object"},"output_schema":{}}"#;
    assert!(decode_registration(normal.as_bytes()).is_ok());
}

#[test]
fn registration_and_call_requests_are_strict_and_bounded() {
    let bytes = serde_json::to_vec(&registration("ledger/balance")).unwrap();
    assert_eq!(
        decode_registration(&bytes).unwrap(),
        registration("ledger/balance")
    );

    for name in [
        "",
        "Ledger",
        "/ledger",
        "ledger/",
        "ledger//balance",
        "ledger balance",
        "ledger\u{7}",
    ] {
        let invalid = serde_json::to_vec(&registration(name)).unwrap();
        assert!(decode_registration(&invalid).is_err(), "{name:?}");
    }
    let mut invalid = registration("ledger/balance");
    invalid.contract_version = 2;
    assert!(validate_registration(&invalid).is_err());
    let mut invalid = registration("ledger/balance");
    invalid.input_schema = json!([]);
    assert!(validate_registration(&invalid).is_err());
    let mut invalid = registration("ledger/balance");
    invalid.default_query = Some(DefaultQuery { kinds: vec![] });
    assert!(validate_registration(&invalid).is_err());
    let mut invalid = registration("ledger/balance");
    invalid.default_query = Some(DefaultQuery {
        kinds: vec!["wiki".into(), "wiki".into()],
    });
    assert!(validate_registration(&invalid).is_err());
    assert!(decode_registration(br#"{"name":"ledger/balance","contract_version":1,"input_schema":{},"output_schema":{},"extra":true}"#).is_err());

    let call = json!({
        "provider": "ledger/balance",
        "input": {"currency": "JPY"},
        "contents": [{"id": "11".repeat(32), "token": "22".repeat(32)}],
    });
    let decoded = decode_call_request(&serde_json::to_vec(&call).unwrap()).unwrap();
    assert_eq!(decoded.provider, "ledger/balance");
    assert_eq!(decoded.timeout_ms, DEFAULT_CALL_TIMEOUT_MS);
    assert_eq!(decoded.contents.len(), 1);
    assert_eq!(decoded.contents[0].content.0, [0x11; 32]);
    assert_eq!(decoded.contents[0].token.0, [0x22; 32]);

    for call in [
        json!({"provider": "Ledger", "contents": []}),
        json!({"provider": "ledger/balance", "timeout_ms": 99}),
        json!({"provider": "ledger/balance", "timeout_ms": 30_001}),
        json!({"provider": "ledger/balance", "contents": [], "extra": 1}),
        json!({"provider": "ledger/balance", "contents": [
            {"id": "11".repeat(32), "token": "22".repeat(32)},
            {"id": "11".repeat(32), "token": "33".repeat(32)}
        ]}),
        json!({"provider": "ledger/balance", "contents": [{"id": "11", "token": "22".repeat(32)}]}),
        json!({"provider": "ledger/balance", "input": {"n": "1".repeat(MAX_CALL_INPUT_BYTES + 1)}}),
    ] {
        assert!(
            decode_call_request(&serde_json::to_vec(&call).unwrap()).is_err(),
            "{call}"
        );
    }
}

#[test]
fn default_query_is_only_an_input_declaration() {
    let query = DefaultQuery {
        kinds: vec!["ledger".into(), "entry".into()],
    };
    let condition = query.condition();
    assert_eq!(
        condition,
        harmion_protocol::query::Condition::And(vec![
            harmion_protocol::query::Condition::Kind("ledger".into()),
            harmion_protocol::query::Condition::Kind("entry".into()),
        ])
    );
}

#[test]
fn provider_results_validate_success_failure_display_and_limits() {
    let success = json!({"status": "ok", "data": {"balance": 1.10}, "display": {"media_type": "text/plain", "text": "ok"}});
    match decode_provider_result(success).unwrap() {
        ProviderOutcome::Success { data, display } => {
            assert_eq!(data, json!({"balance": 1.10}));
            assert_eq!(display.unwrap().text, "ok");
        }
        _ => panic!("success"),
    }
    let minimal = json!({"status": "ok", "data": null});
    assert!(matches!(
        decode_provider_result(minimal).unwrap(),
        ProviderOutcome::Success { display: None, .. }
    ));
    let failure = json!({"status": "error", "error": {"code": "unsupported", "message": "no", "details": {"x": 1}}});
    assert!(matches!(
        decode_provider_result(failure).unwrap(),
        ProviderOutcome::Error(ProviderFailure { ref code, .. }) if code == "unsupported"
    ));

    for invalid in [
        json!({"status": "ok"}),
        json!({"status": "ok", "data": null, "display": {"media_type": "bad type", "text": "x"}}),
        json!({"status": "ok", "data": null, "display": {"media_type": "text/plain", "text": "x".repeat(MAX_DISPLAY_TEXT_BYTES + 1)}}),
        json!({"status": "error", "error": {"code": "Bad", "message": "x"}}),
        json!({"status": "error", "error": {"code": "bad", "message": "x".repeat(MAX_PROVIDER_ERROR_MESSAGE_BYTES + 1)}}),
        json!({"status": "error"}),
        json!({"status": "other"}),
        json!({"status": "ok", "data": null, "extra": 1}),
    ] {
        assert!(decode_provider_result(invalid).is_err());
    }
}

#[test]
fn json_rpc_shapes_are_unambiguous() {
    let request = rpc_request(
        &json!("1"),
        RPC_ATTACH,
        json!({"provider_id": "11".repeat(32)}),
    );
    assert!(matches!(
        decode_rpc(&serde_json::to_vec(&request).unwrap()).unwrap(),
        RpcMessage::Request { ref method, .. } if method == RPC_ATTACH
    ));
    let notification = rpc_notification(RPC_CANCEL, json!({"invocation_id": "22".repeat(32)}));
    assert!(matches!(
        decode_rpc(&serde_json::to_vec(&notification).unwrap()).unwrap(),
        RpcMessage::Notification { ref method, .. } if method == RPC_CANCEL
    ));
    let response = rpc_result(&json!("1"), json!({"ok": true}));
    assert!(matches!(
        decode_rpc(&serde_json::to_vec(&response).unwrap()).unwrap(),
        RpcMessage::Response { result: Ok(_), .. }
    ));
    let error = rpc_error(&json!("1"), RPC_NOT_APPROVED, "not approved");
    assert!(matches!(
        decode_rpc(&serde_json::to_vec(&error).unwrap()).unwrap(),
        RpcMessage::Response { result: Err(RpcFailure { code, .. }), .. } if code == RPC_NOT_APPROVED
    ));

    for invalid in [
        r#"{"jsonrpc":"2.0","id":"1","method":"x","result":null}"#,
        r#"{"jsonrpc":"2.0","id":"1","method":"x","error":{"code":1,"message":"x"}}"#,
        r#"{"jsonrpc":"1.0","id":"1","method":"x"}"#,
        r#"{"jsonrpc":"2.0","method":""}"#,
        r#"{"jsonrpc":"2.0","id":{"x":1},"method":"x"}"#,
    ] {
        assert!(decode_rpc(invalid.as_bytes()).is_err(), "{invalid}");
    }
    let too_large = vec![b' '; MAX_PROVIDER_MESSAGE_BYTES + 1];
    assert!(decode_rpc(&too_large).is_err());
}

#[test]
fn attach_params_and_provider_content_use_hex_ids_and_exact_numbers() {
    let params = decode_attach_params(json!({"provider_id": "ab".repeat(32)})).unwrap();
    assert_eq!(params.provider_id.0, [0xab; 32]);
    assert!(decode_attach_params(json!({"provider_id": "ab"})).is_err());
    assert!(decode_attach_params(json!({"provider_id": "ab".repeat(32), "extra": 1})).is_err());

    // Content is constructed through the domain type to exercise attribute
    // serialization, but this test only needs the stable JSON fields.
    let content = harmion_domain::Content {
        id: harmion_domain::ContentId([3; 32]),
        body: "body".into(),
        attributes: std::collections::BTreeMap::new(),
        kinds: ["ledger".into()].into(),
    };
    let value = provider_content_json(&content);
    assert_eq!(value["id"], "03".repeat(32));
    assert_eq!(value["body"], "body");
    assert_eq!(value["kinds"], json!(["ledger"]));
    assert!(value.get("edit_token").is_none());
}

#[test]
fn websocket_accept_key_and_frame_shapes_are_the_rfc_ones() {
    use harmion_protocol::websocket::{Frame, Opcode, encode_frame, parse_frame};
    assert_eq!(
        harmion_protocol::websocket::accept_key("dGhlIHNhbXBsZSBub25jZQ==").unwrap(),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
    let frame = Frame::text("hello");
    let masked = encode_frame(&frame, Some([1, 2, 3, 4])).unwrap();
    assert_eq!(parse_frame(&masked, true).unwrap().unwrap().0, frame);
    let close = Frame::new(true, Opcode::Close, vec![3, 232]);
    assert_eq!(
        parse_frame(&encode_frame(&close, None).unwrap(), false)
            .unwrap()
            .unwrap()
            .0,
        close
    );
}

#[test]
fn interpreter_grants_are_an_additive_optional_extension_of_grant_json() {
    let old = r#"{"read":{"type":"all"},"write":null,"manage_apps":false,"manage_nodes":false}"#;
    let spec: harmion_protocol::GrantSpec = serde_json::from_str(old).unwrap();
    assert!(!spec.call);
    assert!(!spec.provide);
    let encoded = serde_json::to_value(harmion_protocol::GrantSpec {
        read: None,
        write: None,
        call: false,
        provide: false,
        provide_names: Vec::new(),
        manage_apps: false,
        manage_nodes: false,
    })
    .unwrap();
    assert!(encoded.get("call").is_none());
    assert!(encoded.get("provide").is_none());
    let encoded = serde_json::to_value(harmion_protocol::GrantSpec {
        read: None,
        write: None,
        call: true,
        provide: true,
        provide_names: Vec::new(),
        manage_apps: false,
        manage_nodes: false,
    })
    .unwrap();
    assert_eq!(encoded["call"], true);
    assert_eq!(encoded["provide"], true);
}
