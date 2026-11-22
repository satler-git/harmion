mod cli;

use clap::Parser;
use cli::{Cli, Command as CliCommand};
use domain::{Change, ContentId, Field, UpdateId, Value};
use harmion_client::{
    Client, OwnerSigner, ProtectedDir, SshSigner,
    protocol::{
        connect::ConnectionApproval, interpreter, interpreter::InvocationId,
        interpreter::ProviderId, *,
    },
    random_id,
};
use resolution::{MergeInputRequest, MergePart, parse_selections};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{IsTerminal, Read, Write},
    path::{Path, PathBuf},
};

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
/// Accept either a JSON file or the compressed `harmion-peer:v1:z:` code that
/// the Worker UI displays. The daemon re-validates the registration.
fn peer_registration(input: &str) -> Result<Vec<u8>> {
    let bytes = match input.strip_prefix("harmion-peer:v1:z:") {
        Some(encoded) => {
            use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
            let compressed = URL_SAFE_NO_PAD.decode(encoded)?;
            let decoder = flate2::read::GzDecoder::new(compressed.as_slice());
            let mut bytes = Vec::new();
            decoder
                .take(MAX_RECORD_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            bytes
        }
        None => read(input, MAX_RECORD_BYTES)?,
    };
    if bytes.len() > MAX_RECORD_BYTES {
        return Err("registration exceeds request limit".into());
    }
    let _: PeerRegistration = serde_json::from_slice(&bytes)?;
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
struct OwnerCli {
    signer: Option<SshSigner>,
    enforce: bool,
}

impl OwnerCli {
    async fn new(client: &Client, key: Option<&str>) -> Result<Self> {
        let signer = key
            .map(PathBuf::from)
            .map(SshSigner::new)
            .or_else(|| {
                std::env::var_os("HARMION_OWNER_SSH_KEY")
                    .map(PathBuf::from)
                    .map(SshSigner::new)
            })
            .or_else(harmion_client::ssh_agent_signer)
            .or_else(|| {
                let home = std::env::var_os("HOME")?;
                let path = PathBuf::from(home).join(".ssh").join("id_ed25519.pub");
                path.is_file().then(|| SshSigner::new(path))
            });
        let enforce = client
            .owner_status()
            .await
            .map(|status| status["enforce"] == true)
            .unwrap_or(false);
        Ok(Self { signer, enforce })
    }

    async fn call(
        &self,
        client: &Client,
        operation: &str,
        body: &[u8],
        target: Option<[u8; 32]>,
        summary: &str,
    ) -> Result<serde_json::Value> {
        if self.enforce && harmion_client::protocol::owner::requires_assertion(operation) {
            let signer = self
                .signer
                .as_ref()
                .ok_or("owner enforcement is on; pass --owner-key, set HARMION_OWNER_SSH_KEY, or load a key into ssh-agent")?;
            confirm(summary)?;
            Ok(client
                .call_owner(operation, body, target, None, signer)
                .await?)
        } else {
            Ok(client.call(operation, body).await?)
        }
    }

    async fn export(&self, client: &Client, path: &str) -> Result<serde_json::Value> {
        if !self.enforce {
            return Ok(client.export_directory(path).await?);
        }
        let signer = self
            .signer
            .as_ref()
            .ok_or("owner enforcement is on; pass --owner-key, set HARMION_OWNER_SSH_KEY, or load a key into ssh-agent")?;
        confirm(&format!("全データを {path} へExport"))?;
        let challenge = client.owner_challenge("export", None, &[], None).await?;
        let statement = unhex_bytes(&challenge.statement, MAX_RECORD_BYTES)?;
        let signature = signer.sign(&statement)?;
        Ok(client
            .export_directory_signed(path, unhex(&challenge.challenge)?, signature)
            .await?)
    }
}

fn confirm(summary: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        return Err("owner approval requires a TTY; refusing a non-interactive R2".into());
    }
    eprint!("\n[harmion] 所有者の承認が必要です\n{summary}\n承認しますか? [y/N] ");
    std::io::stderr().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if matches!(input.trim(), "y" | "Y" | "yes" | "YES") {
        Ok(())
    } else {
        Err("owner approval denied".into())
    }
}

async fn execute(cli: &Cli) -> Result<serde_json::Value> {
    let directory = &cli.node;
    let client = Client::from_directory(directory)?;
    let owner = OwnerCli::new(&client, cli.owner_key.as_deref()).await?;
    let command = cli.command.operation();
    match &cli.command {
        CliCommand::Join { path } | CliCommand::JoinRecovery { path } => {
            let registration = owner
                .call(
                    &client,
                    command,
                    &read(path, MAX_RECORD_BYTES)?,
                    None,
                    "ピア登録を確認してノードを参加させます",
                )
                .await?;
            let bytes = serde_json::to_vec(&registration)?;
            ProtectedDir::open(directory)?.replace("peer.json", &bytes)?;
            Ok(registration)
        }
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
        CliCommand::Export { path } => Ok(owner.export(&client, path).await?),
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
            let request = unhex::<32>(request)?;
            owner
                .call(
                    &client,
                    command,
                    &request,
                    Some(request),
                    &format!("OAuth要求 {} の認可リンクを発行", hex(&request)),
                )
                .await
        }
        CliCommand::Get { id } | CliCommand::Conflicts { id } | CliCommand::Inspect { id } => {
            Ok(client.call(command, &unhex::<32>(id)?).await?)
        }
        CliCommand::Sync { id } => Ok(client.call(command, &unhex::<32>(id)?).await?),
        CliCommand::Retire { id }
        | CliCommand::DismissRetirement { id }
        | CliCommand::ConfirmRetirement { id } => {
            let id = unhex::<32>(id)?;
            owner
                .call(
                    &client,
                    command,
                    &id,
                    Some(id),
                    &format!("{command}: ノード/証明 {}", hex(&id)),
                )
                .await
        }
        CliCommand::Approve { path }
        | CliCommand::ChangeGrants { path }
        | CliCommand::TrustRecoveryPeer { path } => {
            let body = read(path, MAX_RECORD_BYTES)?;
            owner
                .call(
                    &client,
                    command,
                    &body,
                    None,
                    &format!("{command}: {path} の内容を適用"),
                )
                .await
        }
        CliCommand::InterpreterList => Ok(client.interpreter_list().await?),
        CliCommand::ConnectList => Ok(client.connect_list().await?),
        CliCommand::ConnectApprove { id, deny, replace } => {
            let body = serde_json::to_vec(&ConnectionApproval {
                connection_id: id.to_string(),
                allow: !deny,
                replace: *replace,
            })?;
            owner
                .call(
                    &client,
                    "connect-approve",
                    &body,
                    None,
                    &format!("接続要求 {id} を{}", if *deny { "拒否" } else { "承認" }),
                )
                .await
        }
        CliCommand::InterpreterDescribe { id } => {
            Ok(client.interpreter_describe(ProviderId(unhex(id)?)).await?)
        }
        CliCommand::InterpreterRegister { path } => {
            let registration = interpreter::decode_registration(&read(
                path,
                interpreter::MAX_REGISTRATION_BYTES,
            )?)?;
            Ok(client.interpreter_register(&registration).await?)
        }
        CliCommand::InterpreterApprove { id, replace } => {
            let provider = ProviderId(unhex(id)?);
            let body = serde_json::to_vec(&serde_json::json!({
                "provider_id": provider,
                "replace": *replace,
            }))?;
            owner
                .call(
                    &client,
                    "interpreter-approve",
                    &body,
                    Some(provider.0),
                    &format!("Interpreter Provider {id} を承認"),
                )
                .await
        }
        CliCommand::InterpreterCall { request } => {
            let request = interpreter::decode_call_request(&read(
                request,
                interpreter::MAX_CALL_REQUEST_BYTES,
            )?)?;
            Ok(client.interpreter_call(&request).await?)
        }
        CliCommand::InterpreterCancel { id } => {
            Ok(client.interpreter_cancel(InvocationId(unhex(id)?)).await?)
        }
        CliCommand::ProvisionManifest { peer, out } => {
            let manifest = owner
                .call(
                    &client,
                    command,
                    &peer_registration(peer)?,
                    None,
                    "Workerのprovisioning manifestを生成します",
                )
                .await?;
            match out {
                Some(path) => {
                    let mut text = serde_json::to_string_pretty(&manifest)?;
                    text.push('\n');
                    std::fs::write(path, text)?;
                    Ok(serde_json::json!({"written":path}))
                }
                None => Ok(manifest),
            }
        }
        CliCommand::OwnerStatus => Ok(client.owner_status().await?),
        CliCommand::OwnerEnroll { key, label } => {
            let public_key = String::from_utf8(read(key, 16 * 1024)?)?;
            let body = serde_json::to_vec(&serde_json::json!({
                "public_key": public_key,
                "label": label,
            }))?;
            owner
                .call(
                    &client,
                    "owner-enroll",
                    &body,
                    None,
                    "所有者のバックアップ鍵を追加します",
                )
                .await
        }
        CliCommand::OwnerRevoke { credential } => {
            let credential = unhex::<32>(credential)?;
            let body = serde_json::to_vec(&serde_json::json!({
                "credential": hex(&credential),
            }))?;
            owner
                .call(
                    &client,
                    "owner-revoke",
                    &body,
                    Some(credential),
                    "所有者credentialを失効します",
                )
                .await
        }
        CliCommand::OwnerRecover { code, key, label } => {
            let public_key = String::from_utf8(read(key, 16 * 1024)?)?;
            let body = serde_json::to_vec(&serde_json::json!({
                "recovery_code": code,
                "public_key": public_key,
                "label": label,
            }))?;
            confirm("回復コードで全所有者credentialを置き換えます")?;
            Ok(client.call("owner-recover", &body).await?)
        }
        CliCommand::OwnerEnforce { enable } => {
            let body = serde_json::to_vec(&serde_json::json!({"enforce": enable}))?;
            owner
                .call(
                    &client,
                    "owner-enforce",
                    &body,
                    None,
                    &format!(
                        "所有者enforcementを{}",
                        if *enable { "有効" } else { "無効" }
                    ),
                )
                .await
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

#[cfg(test)]
mod tests {
    use super::*;

    fn registration_code() -> (Vec<u8>, String) {
        let registration = serde_json::json!({
            "version": 4,
            "cluster_id": hex(&[1; 32]),
            "node_id": hex(&[2; 32]),
            "binding": {"transport": "signed_https", "origin": "https://worker.example"},
            "signing_key": hex(&[3; 32]),
        });
        let bytes = serde_json::to_vec(&registration).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        let code = format!("harmion-peer:v1:z:{}", URL_SAFE_NO_PAD.encode(compressed));
        (bytes, code)
    }

    #[test]
    fn peer_code_is_decompressed_and_validated() {
        let (bytes, code) = registration_code();
        assert_eq!(peer_registration(&code).unwrap(), bytes);
        assert!(peer_registration("harmion-peer:v1:z:not-base64!").is_err());
        assert!(peer_registration("harmion-peer:v1:z:").is_err());
    }

    #[test]
    fn malformed_registration_file_is_rejected() {
        let path = std::env::temp_dir().join(format!("harmion-peer-{}", std::process::id()));
        std::fs::write(&path, b"{\"version\":4}").unwrap();
        let result = peer_registration(path.to_str().unwrap());
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err());
    }
}
