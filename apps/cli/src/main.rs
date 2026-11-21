mod cli;

use clap::Parser;
use cli::{Cli, Command as CliCommand};
use domain::{Change, ContentId, Field, UpdateId, Value};
use harmion_client::{Client, ProtectedDir, protocol::*, random_id};
use resolution::{MergeInputRequest, MergePart, parse_selections};
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
fn read(path: &str, max: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err("file exceeds request limit".into());
    }
    Ok(bytes)
}
fn body(path: &str) -> Result<BTreeMap<Field, Value>> {
    Ok([(
        Field::Body,
        Value::Body(String::from_utf8(read(path, MAX_RECORD_BYTES)?)?),
    )]
    .into())
}
fn write_protected(path: &str, bytes: &[u8]) -> Result<()> {
    let path = Path::new(path);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("invalid output filename")?;
    ProtectedDir::open(parent)?.write_new(name, bytes)?;
    Ok(())
}
async fn save(
    client: &Client,
    content_id: ContentId,
    command: Command,
    path: &str,
) -> Result<serde_json::Value> {
    let request = SaveRequest {
        request_id: RequestId(random_id()?),
        content_id,
        command,
    };
    let bytes = encode_save_request(&request)?;
    // Persist before sending. A timeout is ambiguous: retry this file, never mint another request.
    write_protected(path, &bytes)?;
    let result = client.call("save", &bytes).await?;
    Ok(
        serde_json::json!({"content_id":hex(&content_id.0),"request_id":hex(&request.request_id.0),"request_file":path,"result":result}),
    )
}
async fn execute(cli: &Cli) -> Result<serde_json::Value> {
    let directory = &cli.node;
    let client = Client::from_directory(directory)?;
    let command = cli.command.operation();
    match &cli.command {
        CliCommand::Join { path } | CliCommand::JoinRecovery { path } => Ok(client
            .join_directory(
                &ProtectedDir::open(directory)?,
                &read(path, MAX_RECORD_BYTES)?,
                command == "join-recovery",
            )
            .await?),
        CliCommand::Identity
        | CliCommand::Capabilities
        | CliCommand::Permissions
        | CliCommand::Members => Ok(client.call(command, &[]).await?),
        CliCommand::SyncStatus { update } => {
            let update = update
                .as_ref()
                .map(|id| unhex(id).map(UpdateId))
                .transpose()?;
            Ok(serde_json::to_value(client.sync_status(update).await?)?)
        }
        CliCommand::Export { path } => Ok(client.export_directory(path).await?),
        CliCommand::OauthRegister { name, redirect } => Ok(client
            .call(
                command,
                &serde_json::to_vec(&OAuthRegistration {
                    name: name.clone(),
                    redirect_uri: redirect.clone(),
                })?,
            )
            .await?),
        CliCommand::OauthConsent { request } => {
            Ok(client.call(command, &unhex::<32>(request)?).await?)
        }
        CliCommand::Get { id }
        | CliCommand::Conflicts { id }
        | CliCommand::Inspect { id }
        | CliCommand::Sync { id }
        | CliCommand::Retire { id }
        | CliCommand::DismissRetirement { id }
        | CliCommand::ConfirmRetirement { id } => {
            Ok(client.call(command, &unhex::<32>(id)?).await?)
        }
        CliCommand::Approve { path }
        | CliCommand::ChangeGrants { path }
        | CliCommand::TrustRecoveryPeer { path } => {
            Ok(client.call(command, &read(path, MAX_RECORD_BYTES)?).await?)
        }
        CliCommand::Save { path } => {
            let bytes = read(path, MAX_RECORD_BYTES)?;
            decode_save_request(&bytes)?;
            Ok(client.call("save", &bytes).await?)
        }
        CliCommand::Create {
            source,
            request_file,
            kinds,
        } => {
            let mut fields = body(source)?;
            for kind in kinds {
                fields.insert(Field::Kind(kind.clone()), Value::Kind(true));
            }
            save(
                &client,
                ContentId(random_id()?),
                Command::Create { fields },
                request_file,
            )
            .await
        }
        CliCommand::Edit {
            id,
            token,
            source,
            request_file,
        }
        | CliCommand::Restore {
            id,
            token,
            source,
            request_file,
        } => {
            let token = EditToken(unhex(token)?);
            let fields = body(source)?;
            let command = if command == "edit" {
                Command::Edit { token, fields }
            } else {
                Command::Restore { token, fields }
            };
            save(&client, ContentId(unhex(id)?), command, request_file).await
        }
        CliCommand::Delete {
            id,
            token,
            request_file,
        }
        | CliCommand::Keep {
            id,
            token,
            request_file,
        } => {
            let token = EditToken(unhex(token)?);
            let command = if command == "delete" {
                Command::Delete { token }
            } else {
                Command::Keep { token }
            };
            save(&client, ContentId(unhex(id)?), command, request_file).await
        }
        CliCommand::Resolve {
            id,
            token,
            source,
            request_file,
            versions,
        } => {
            let value = body(source)?.remove(&Field::Body).expect("body field");
            let replaces = versions
                .iter()
                .map(|v| unhex(v).map(UpdateId))
                .collect::<std::result::Result<_, _>>()?;
            let command = Command::Resolve {
                token: EditToken(unhex(token)?),
                changes: [(Field::Body, Change { value, replaces })].into(),
            };
            save(&client, ContentId(unhex(id)?), command, request_file).await
        }
        CliCommand::Select {
            id,
            token,
            selections,
            request_file,
        } => {
            let selections = parse_selections(serde_json::from_slice(&read(
                selections,
                MAX_RECORD_BYTES,
            )?)?)?;
            let command = Command::Select {
                token: EditToken(unhex(token)?),
                selections,
            };
            save(&client, ContentId(unhex(id)?), command, request_file).await
        }
        CliCommand::MergeBase {
            id,
            token,
            left,
            right,
            base,
        }
        | CliCommand::MergePlan {
            id,
            token,
            left,
            right,
            base,
        } => {
            let request = MergeInputRequest {
                content: ContentId(unhex(id)?),
                token: EditToken(unhex(token)?),
                left: UpdateId(unhex(left)?),
                right: UpdateId(unhex(right)?),
                part: MergePart::Base(
                    base.as_ref()
                        .map(|id| unhex(id).map(UpdateId))
                        .transpose()?,
                ),
            };
            if command == "merge-base" {
                Ok(serde_json::to_value(client.merge_input(request).await?)?)
            } else {
                Ok(serde_json::to_value(client.merge_plan(request).await?)?)
            }
        }
    }
}
#[tokio::main]
async fn main() {
    match execute(&Cli::parse()).await {
        Ok(result) => println!("{result}"),
        Err(error) => {
            eprintln!("harmion: {error}");
            std::process::exit(1);
        }
    }
}
