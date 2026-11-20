use crate::transfer::{self, Confirmations, PeerAction};
use harmion_client::{
    Client, Error, ProtectedDir,
    protocol::{
        ChangeGrantsRequest, Command, EditToken, OAuthRegistration, PeerRegistration, RequestId,
        SaveRequest,
        domain::{Change, ContentId, Field, UpdateId, Value as FieldValue},
        query::{Condition, QueryCursor},
        resolution::{
            MergeInputRequest, MergePart, SelectionField, SelectionSpec, parse_selections,
        },
        unhex,
    },
};
use hyper::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

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
    client: &Client,
    node: &ProtectedDir,
    confirmations: &Confirmations,
    path: &str,
    body: &[u8],
) -> Result<Value, StatusCode> {
    match path {
        "/overview" => {
            let (identity, permissions, capabilities, metrics, members) = tokio::try_join!(
                client.call("identity", &[]),
                client.call("permissions", &[]),
                client.call("capabilities", &[]),
                client.call("metrics", &[]),
                client.call("members", &[]),
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
            let local = client.call("identity", &[]).await.map_err(remote)?;
            if registration.version != 3
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
            node_operation(client, node, operation).await
        }
        "/app-preview" => {
            let request: TextInput = parse(body)?;
            let input = request.input.trim();
            if let Some(request_id) = input.strip_prefix("harmion-auth:v1:") {
                let preview = client
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
        "/members" => client.call("members", &[]).await.map_err(remote),
        "/node" => node_operation(client, node, parse(body)?).await,
        "/oauth-register" => {
            let request: OAuthRegistration = parse(body)?;
            client
                .call("oauth-register", &encode(&request)?)
                .await
                .map_err(remote)
        }
        "/oauth-consent" => {
            let request: ConsentRequest = parse(body)?;
            client
                .call("oauth-consent", &id(&request.request)?)
                .await
                .map_err(remote)
        }
        "/change-grants" => {
            let request: ChangeGrantsRequest = parse(body)?;
            client
                .call("change-grants", &encode(&request)?)
                .await
                .map_err(remote)
        }
        "/export" => {
            let request: Export = parse(body)?;
            if request.directory.is_empty() {
                return Err(StatusCode::BAD_REQUEST);
            }
            client
                .export_directory(&request.directory)
                .await
                .map_err(remote)
        }
        "/contents" => {
            let request: Page = parse(body)?;
            let page = if let Some(next) = request.next {
                client.query_next(QueryCursor(id(&next)?), 24).await
            } else {
                client.query(Condition::And(vec![]), 24).await
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
            let mut data = client
                .call("inspect", &id(&request.content)?)
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
                serde_json::to_value(client.merge_input(input).await.map_err(remote)?)
            } else {
                serde_json::to_value(client.merge_plan(input).await.map_err(remote)?)
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
            let receipt = client
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

async fn node_operation(
    client: &Client,
    node: &ProtectedDir,
    request: NodeOperation,
) -> Result<Value, StatusCode> {
    let (operation, body) = match request {
        NodeOperation::Join { registration } => {
            return client
                .join_directory(node, &encode(&registration)?, false)
                .await
                .map_err(remote);
        }
        NodeOperation::Approve { registration } => ("approve", encode(&registration)?),
        NodeOperation::Retire { node } => ("retire", id(&node)?.to_vec()),
        NodeOperation::Sync { node } => ("sync", id(&node)?.to_vec()),
        NodeOperation::ConfirmRetirement { certificate } => {
            ("confirm-retirement", id(&certificate)?.to_vec())
        }
        NodeOperation::DismissRetirement { certificate } => {
            ("dismiss-retirement", id(&certificate)?.to_vec())
        }
    };
    client.call(operation, &body).await.map_err(remote)
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
