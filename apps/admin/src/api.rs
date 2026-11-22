use crate::transfer::{self, Confirmations, PeerAction};
use harmion_client::{
    Client, Error, OwnerSigner, ProtectedDir, SshSigner,
    protocol::{
        ChangeGrantsRequest, Command, EditToken, MAX_RECORD_BYTES, OAuthRegistration, PeerBinding,
        PeerRegistration, RequestId, SaveRequest,
        connect::ConnectionApproval,
        domain::{Change, ContentId, Field, UpdateId, Value as FieldValue},
        interpreter::ProviderId,
        query::{Condition, QueryCursor},
        resolution::{
            MergeInputRequest, MergePart, SelectionField, SelectionSpec, parse_selections,
        },
        unhex, unhex_bytes,
    },
};
use hyper::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) struct Caller<'a> {
    client: &'a Client,
    signer: Option<&'a SshSigner>,
    enforce: bool,
    session: [u8; 32],
}

impl<'a> Caller<'a> {
    pub fn new(
        client: &'a Client,
        signer: Option<&'a SshSigner>,
        enforce: bool,
        session: [u8; 32],
    ) -> Self {
        Self {
            client,
            signer,
            enforce,
            session,
        }
    }

    fn client(&self) -> &Client {
        self.client
    }

    async fn call(
        &self,
        operation: &str,
        body: &[u8],
        target: Option<[u8; 32]>,
    ) -> Result<Value, Error> {
        if self.enforce && harmion_client::protocol::owner::requires_assertion(operation) {
            let Some(signer) = self.signer else {
                tracing::warn!(
                    operation,
                    reason = "owner_signer_unavailable",
                    status = 403,
                    "owner assertion required but no signer configured"
                );
                return Err(Error::Http(403));
            };
            self.client
                .call_owner(operation, body, target, Some(self.session), signer)
                .await
        } else {
            self.client.call(operation, body).await
        }
    }

    async fn export(&self, path: &str) -> Result<Value, Error> {
        if self.enforce {
            let Some(signer) = self.signer else {
                tracing::warn!(
                    operation = "export",
                    reason = "owner_signer_unavailable",
                    status = 403,
                    "owner assertion required but no signer configured"
                );
                return Err(Error::Http(403));
            };
            let challenge = self
                .client
                .owner_challenge("export", None, &[], Some(self.session))
                .await?;
            let statement = unhex_bytes(&challenge.statement, MAX_RECORD_BYTES)?;
            let signature = signer.sign(&statement)?;
            self.client
                .export_directory_signed(path, unhex(&challenge.challenge)?, signature)
                .await
        } else {
            self.client.export_directory(path).await
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    next: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inspection {
    content: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum NodeOperation {
    Approve { registration: PeerRegistration },
    Join { registration: PeerRegistration },
    Sync { node: String },
    Retire { node: String },
    ConfirmRetirement { certificate: String },
    DismissRetirement { certificate: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsentRequest {
    request: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextInput {
    input: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerPreview {
    input: String,
    action: PeerAction,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    ticket: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeProvider {
    provider: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApproveProvider {
    provider: String,
    #[serde(default)]
    replace: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Export {
    directory: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Merge {
    content: String,
    token: String,
    left: String,
    right: String,
    base: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    content: String,
    token: String,
    request_id: String,
    edit: Edit,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Edit {
    Select {
        fields: Vec<SelectionSpec>,
    },
    Body {
        text: String,
        versions: Vec<String>,
    },
    Delete,
    Keep,
    Restore {
        text: String,
        fields: Vec<RestoreField>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreField {
    field: SelectionField,
    version: String,
}

pub(crate) async fn dispatch(
    caller: &Caller<'_>,
    node: &ProtectedDir,
    confirmations: &Confirmations,
    path: &str,
    body: &[u8],
) -> Result<Value, StatusCode> {
    match path {
        "/overview" => {
            let (identity, permissions, capabilities, metrics, members) = tokio::try_join!(
                caller.call("identity", &[], None),
                caller.call("permissions", &[], None),
                caller.call("capabilities", &[], None),
                caller.call("metrics", &[], None),
                caller.call("members", &[], None),
            )
            .map_err(remote)?;
            let transfer = transfer::encode_peer(&identity)?;
            Ok(
                json!({"identity":identity,"permissions":permissions,"capabilities":capabilities,
                "metrics":metrics,"members":members,"peer_transfer":transfer}),
            )
        }
        "/peer-preview" => {
            let request: PeerPreview = parse(body)?;
            let registration = transfer::decode_peer(&request.input)?;
            let local = caller
                .client()
                .call("identity", &[])
                .await
                .map_err(remote)?;
            // v3 is the native TLS/iroh registration; v4 is the explicit
            // signed-HTTPS binding. Never reinterpret one version as the other.
            let version_ok = match (&registration.binding, registration.version) {
                (PeerBinding::SignedHttps { .. }, 4) => true,
                (PeerBinding::SignedHttps { .. }, _) => false,
                (_, 3) => true,
                (_, _) => false,
            };
            if !version_ok
                || registration.cluster_id != local["cluster_id"]
                || registration.node_id == local["node_id"]
                || id(&registration.node_id).is_err()
                || registration
                    .signing_key
                    .as_deref()
                    .and_then(|value| id(value).ok())
                    .is_none()
            {
                return Err(StatusCode::BAD_REQUEST);
            }
            let ticket = confirmations.preview(registration.clone(), request.action)?;
            Ok(json!({"ticket":ticket,"registration":registration,"action":request.action}))
        }
        "/peer-confirm" => {
            let request: Confirmation = parse(body)?;
            let preview = confirmations.take(&request.ticket)?;
            let operation = match preview.action {
                PeerAction::Approve => NodeOperation::Approve {
                    registration: preview.registration,
                },
                PeerAction::Join => NodeOperation::Join {
                    registration: preview.registration,
                },
            };
            node_operation(caller, node, operation).await
        }
        "/app-preview" => {
            let request: TextInput = parse(body)?;
            let input = request.input.trim();
            if let Some(request_id) = input.strip_prefix("harmion-auth:v1:") {
                let preview = caller
                    .client()
                    .call("oauth-preview", &id(request_id)?)
                    .await
                    .map_err(remote)?;
                Ok(json!({"type":"authorization","request":request_id,"preview":preview}))
            } else {
                let registration: OAuthRegistration = transfer::decode("harmion-app:v1:", input)?;
                Ok(
                    json!({"type":"registration","name":registration.name,"redirect_uri":registration.redirect_uri}),
                )
            }
        }
        "/members" => caller.call("members", &[], None).await.map_err(remote),
        "/connect-list" => caller.call("connect-list", &[], None).await.map_err(remote),
        "/connect-approve" => {
            let request: ConnectionApproval = parse(body)?;
            caller
                .call("connect-approve", &encode(&request)?, None)
                .await
                .map_err(remote)
        }
        "/interpreter-list" => caller.client().interpreter_list().await.map_err(remote),
        "/interpreter-describe" => {
            let request: DescribeProvider = parse(body)?;
            caller
                .client()
                .interpreter_describe(ProviderId(
                    unhex(&request.provider).map_err(|_| StatusCode::BAD_REQUEST)?,
                ))
                .await
                .map_err(remote)
        }
        "/interpreter-approve" => {
            let request: ApproveProvider = parse(body)?;
            let provider =
                ProviderId(unhex(&request.provider).map_err(|_| StatusCode::BAD_REQUEST)?);
            let body = serde_json::to_vec(&json!({
                "provider_id": provider,
                "replace": request.replace,
            }))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            caller
                .call("interpreter-approve", &body, Some(provider.0))
                .await
                .map_err(remote)
        }
        "/node" => node_operation(caller, node, parse(body)?).await,
        "/oauth-register" => {
            let request: OAuthRegistration = parse(body)?;
            caller
                .call("oauth-register", &encode(&request)?, None)
                .await
                .map_err(remote)
        }
        "/oauth-consent" => {
            let request: ConsentRequest = parse(body)?;
            let target = id(&request.request)?;
            caller
                .call("oauth-consent", &target, Some(target))
                .await
                .map_err(remote)
        }
        "/change-grants" => {
            let request: ChangeGrantsRequest = parse(body)?;
            let target = id(&request.app_id)?;
            caller
                .call("change-grants", &encode(&request)?, Some(target))
                .await
                .map_err(remote)
        }
        "/export" => {
            let request: Export = parse(body)?;
            if request.directory.is_empty() {
                return Err(StatusCode::BAD_REQUEST);
            }
            caller.export(&request.directory).await.map_err(remote)
        }
        "/contents" => {
            let request: Page = parse(body)?;
            let page = if let Some(next) = request.next {
                caller
                    .client()
                    .query_next(QueryCursor(id(&next)?), 24)
                    .await
            } else {
                caller.client().query(Condition::And(vec![]), 24).await
            }
            .map_err(remote)?;
            let items: Vec<_> = page.items.iter().map(|item| json!({
                "id": item["content"]["id"],
                "preview": item["content"]["body"].as_str().unwrap_or_default().chars().take(100).collect::<String>(),
                "body": item["content"]["body"],
                "kinds": item["content"]["kinds"],
                "attributes": item["content"]["attributes"].to_string(),
                "conflicted": item["deletion_conflict"] == true
                    || item["conflicts"].as_array().is_some_and(|items| !items.is_empty()),
            })).collect();
            Ok(
                json!({"items":items,"next":page.next.map(|cursor| harmion_client::protocol::hex(&cursor.0))}),
            )
        }
        "/inspect" => {
            let request: Inspection = parse(body)?;
            let mut data = caller
                .call("inspect", &id(&request.content)?, None)
                .await
                .map_err(remote)?;
            // Render typed attributes as exact JSON text in the UI, including large numbers.
            if let Some(groups) = data["candidates"].as_array_mut() {
                for group in groups {
                    if group["field"]["type"] != "attribute" {
                        continue;
                    }
                    if let Some(candidates) = group["candidates"].as_array_mut() {
                        for candidate in candidates {
                            if candidate["value"]["type"] == "attribute" {
                                let value = candidate["value"]["value"].take();
                                candidate["value"]["value"] = Value::String(value.to_string());
                            }
                        }
                    }
                }
            }
            // Selected values are available in candidates; the UI needs only the edit token here.
            Ok(json!({
                "token":data["view"]["edit_token"],"deleted":data["deleted"],
                "deletion_conflict":data["view"]["deletion_conflict"],"groups":data["candidates"],
            }))
        }
        "/merge" | "/merge-base" => {
            let request: Merge = parse(body)?;
            let input = MergeInputRequest {
                content: ContentId(id(&request.content)?),
                token: EditToken(id(&request.token)?),
                left: UpdateId(id(&request.left)?),
                right: UpdateId(id(&request.right)?),
                part: MergePart::Base(request.base.as_deref().map(id).transpose()?.map(UpdateId)),
            };
            let serialized = if path == "/merge-base" {
                serde_json::to_value(caller.client().merge_input(input).await.map_err(remote)?)
            } else {
                serde_json::to_value(caller.client().merge_plan(input).await.map_err(remote)?)
            };
            serialized.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
        "/resolve" => {
            let request: Save = parse(body)?;
            let token = EditToken(id(&request.token)?);
            let command = match request.edit {
                Edit::Select { fields } => Command::Select {
                    token,
                    selections: parse_selections(fields).map_err(|_| StatusCode::BAD_REQUEST)?,
                },
                Edit::Body { text, versions } => Command::Resolve {
                    token,
                    changes: body_changes(text, versions)?,
                },
                Edit::Delete => Command::Delete { token },
                Edit::Keep => Command::Keep { token },
                Edit::Restore { text, fields } => {
                    let mut selected = std::collections::BTreeMap::new();
                    for field in fields {
                        if selected
                            .insert(field.field.into(), UpdateId(id(&field.version)?))
                            .is_some()
                        {
                            return Err(StatusCode::BAD_REQUEST);
                        }
                    }
                    Command::RestoreFrom {
                        token,
                        body: text,
                        fields: selected,
                    }
                }
            };
            let receipt = caller
                .client()
                .save(&SaveRequest {
                    content_id: ContentId(id(&request.content)?),
                    request_id: RequestId(id(&request.request_id)?),
                    command,
                })
                .await
                .map_err(remote)?;
            Ok(json!({"saved":receipt["saved"],"update_id":receipt["update_id"]}))
        }
        _ => Err(StatusCode::NOT_FOUND),
    }
}

pub(crate) fn approval_summary(
    path: &str,
    body: &[u8],
    confirmations: &Confirmations,
) -> Option<String> {
    let summary = match path {
        "/change-grants" => {
            let request: ChangeGrantsRequest = serde_json::from_slice(body).ok()?;
            let grants = serde_json::to_string(&request.grants).ok()?;
            format!(
                "アプリ {} のgrantを変更（revoked={}）\n{}",
                request.app_id, request.revoked, grants
            )
        }
        "/node" => {
            let request: NodeOperation = serde_json::from_slice(body).ok()?;
            match request {
                NodeOperation::Approve { registration } => {
                    format!("ノード {} を承認", registration.node_id)
                }
                NodeOperation::Join { registration } => {
                    format!("ノード {} をこのクラスタへ参加", registration.node_id)
                }
                NodeOperation::Sync { .. } => return None,
                NodeOperation::Retire { node } => format!("ノード {node} を退役"),
                NodeOperation::ConfirmRetirement { certificate } => {
                    format!("退役証明 {certificate} を確定")
                }
                NodeOperation::DismissRetirement { certificate } => {
                    format!("保留中の退役証明 {certificate} を却下")
                }
            }
        }
        "/peer-confirm" => {
            let request: Confirmation = serde_json::from_slice(body).ok()?;
            confirmations.summary(&request.ticket)?
        }
        "/connect-approve" => {
            let request: ConnectionApproval = serde_json::from_slice(body).ok()?;
            format!(
                "接続要求 {} を{}",
                request.connection_id,
                if request.allow { "承認" } else { "拒否" }
            )
        }
        "/interpreter-approve" => {
            let request: ApproveProvider = serde_json::from_slice(body).ok()?;
            format!(
                "Interpreter Provider {} を承認（replace={}）",
                request.provider, request.replace
            )
        }
        "/oauth-consent" => {
            let request: ConsentRequest = serde_json::from_slice(body).ok()?;
            format!("OAuth要求 {} の認可リンクを発行", request.request)
        }
        "/export" => {
            let request: Export = serde_json::from_slice(body).ok()?;
            // `directory` is owner-supplied but may be influenced by a stolen
            // admin session; keep the terminal prompt on one line.
            format!("全データを {} へExport", one_line(&request.directory))
        }
        _ => return None,
    };
    Some(
        summary
            .chars()
            .filter(|character| *character == '\n' || !character.is_control())
            .take(2000)
            .collect(),
    )
}

/// Terminal approval text for an integrated connection request. The request
/// body only carries an opaque connection id, so fetch the pending request and
/// show the app name, callback, origin, existing client binding and grants that
/// are actually about to be approved.
pub(crate) async fn connection_approval_summary(client: &Client, body: &[u8]) -> Option<String> {
    let request: ConnectionApproval = serde_json::from_slice(body).ok()?;
    let listing = client.call("connect-list", &[]).await.ok()?;
    let connection = listing["connections"]
        .as_array()?
        .iter()
        .find(|connection| {
            connection["connection_id"].as_str() == Some(request.connection_id.as_str())
        })?;
    let grants = serde_json::to_string(&connection["grants"]).ok()?;
    let client_id = connection["client_id"]
        .as_str()
        .map_or_else(|| "(新規)".to_owned(), one_line);
    Some(format!(
        "接続要求を{}: アプリ {} / redirect {} / web_origin {} / client {} / grants {}\nconnection_id={}",
        if request.allow { "承認" } else { "拒否" },
        one_line(connection["name"].as_str().unwrap_or("-")),
        one_line(connection["redirect_uri"].as_str().unwrap_or("-")),
        one_line(connection["web_origin"].as_str().unwrap_or("(未指定)")),
        client_id,
        one_line(&grants),
        request.connection_id,
    ))
}

/// Collapse control characters so request-controlled strings cannot forge extra
/// lines in the terminal approval prompt.
fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(240)
        .collect()
}

async fn node_operation(
    caller: &Caller<'_>,
    node: &ProtectedDir,
    request: NodeOperation,
) -> Result<Value, StatusCode> {
    let (operation, body, target) = match request {
        NodeOperation::Join { registration } => {
            let target = id(&registration.node_id)?;
            let result = caller
                .call("join", &encode(&registration)?, Some(target))
                .await
                .map_err(remote)?;
            let bytes =
                serde_json::to_vec(&result).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            node.replace("peer.json", &bytes)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            return Ok(result);
        }
        NodeOperation::Approve { registration } => (
            "approve",
            encode(&registration)?,
            Some(id(&registration.node_id)?),
        ),
        NodeOperation::Retire { node } => ("retire", id(&node)?.to_vec(), Some(id(&node)?)),
        NodeOperation::Sync { node } => ("sync", id(&node)?.to_vec(), Some(id(&node)?)),
        NodeOperation::ConfirmRetirement { certificate } => (
            "confirm-retirement",
            id(&certificate)?.to_vec(),
            Some(id(&certificate)?),
        ),
        NodeOperation::DismissRetirement { certificate } => (
            "dismiss-retirement",
            id(&certificate)?.to_vec(),
            Some(id(&certificate)?),
        ),
    };
    caller.call(operation, &body, target).await.map_err(remote)
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, StatusCode> {
    serde_json::to_vec(value).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn body_changes(
    text: String,
    versions: Vec<String>,
) -> Result<std::collections::BTreeMap<Field, Change>, StatusCode> {
    Ok([(
        Field::Body,
        Change {
            value: FieldValue::Body(text),
            replaces: versions
                .iter()
                .map(|version| id(version).map(UpdateId))
                .collect::<Result<_, _>>()?,
        },
    )]
    .into())
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, StatusCode> {
    serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)
}

fn id(input: &str) -> Result<[u8; 32], StatusCode> {
    unhex(input).map_err(|_| StatusCode::BAD_REQUEST)
}

fn remote(error: Error) -> StatusCode {
    match error {
        Error::InvalidRequest => StatusCode::BAD_REQUEST,
        Error::Http(status) => StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
        Error::Timeout => StatusCode::GATEWAY_TIMEOUT,
        Error::Io(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            StatusCode::CONFLICT
        }
        _ => StatusCode::BAD_GATEWAY,
    }
}
