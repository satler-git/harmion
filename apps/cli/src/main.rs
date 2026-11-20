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
async fn execute(args: &[String]) -> Result<serde_json::Value> {
    let [flag, directory, command, rest @ ..] = args else {
        return Err(USAGE.into());
    };
    if flag != "--node" {
        return Err(USAGE.into());
    }
    let client = Client::from_directory(directory)?;
    match (command.as_str(), rest) {
        ("join" | "join-recovery", [path]) => Ok(client
            .join_directory(
                &ProtectedDir::open(directory)?,
                &read(path, MAX_RECORD_BYTES)?,
                command == "join-recovery",
            )
            .await?),
        ("identity" | "capabilities" | "permissions" | "members", []) => {
            Ok(client.call(command, &[]).await?)
        }
        ("export", [path]) => Ok(client.export_directory(path).await?),
        ("oauth-register", [name, redirect]) => Ok(client
            .call(
                command,
                &serde_json::to_vec(&OAuthRegistration {
                    name: name.clone(),
                    redirect_uri: redirect.clone(),
                })?,
            )
            .await?),
        ("oauth-consent", [request]) => Ok(client.call(command, &unhex::<32>(request)?).await?),
        (
            "get" | "conflicts" | "inspect" | "sync" | "retire" | "dismiss-retirement"
            | "confirm-retirement",
            [id],
        ) => Ok(client.call(command, &unhex::<32>(id)?).await?),
        ("approve" | "change-grants" | "trust-recovery-peer", [path]) => {
            Ok(client.call(command, &read(path, MAX_RECORD_BYTES)?).await?)
        }
        ("save", [path]) => {
            let bytes = read(path, MAX_RECORD_BYTES)?;
            decode_save_request(&bytes)?;
            Ok(client.call("save", &bytes).await?)
        }
        ("create", [source, request_file, kinds @ ..]) => {
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
        ("edit" | "restore", [id, token, source, request_file]) => {
            let token = EditToken(unhex(token)?);
            let fields = body(source)?;
            let command = if command == "edit" {
                Command::Edit { token, fields }
            } else {
                Command::Restore { token, fields }
            };
            save(&client, ContentId(unhex(id)?), command, request_file).await
        }
        ("delete" | "keep", [id, token, request_file]) => {
            let token = EditToken(unhex(token)?);
            let command = if command == "delete" {
                Command::Delete { token }
            } else {
                Command::Keep { token }
            };
            save(&client, ContentId(unhex(id)?), command, request_file).await
        }
        ("resolve", [id, token, source, request_file, versions @ ..]) if !versions.is_empty() => {
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
        ("select", [id, token, selections, request_file]) => {
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
        ("merge-base" | "merge-plan", [id, token, left, right, base @ ..]) if base.len() <= 1 => {
            let request = MergeInputRequest {
                content: ContentId(unhex(id)?),
                token: EditToken(unhex(token)?),
                left: UpdateId(unhex(left)?),
                right: UpdateId(unhex(right)?),
                part: MergePart::Base(base.first().map(|id| unhex(id).map(UpdateId)).transpose()?),
            };
            if command == "merge-base" {
                Ok(serde_json::to_value(client.merge_input(request).await?)?)
            } else {
                Ok(serde_json::to_value(client.merge_plan(request).await?)?)
            }
        }
        _ => Err(USAGE.into()),
    }
}
const USAGE: &str = "usage: harmion --node DIR COMMAND ...\nidentity | capabilities | permissions | members\napprove|join PEER_JSON | sync|retire NODE_ID | change-grants JSON\nconfirm-retirement|dismiss-retirement CERTIFICATE_ID\njoin-recovery|trust-recovery-peer PEER_JSON (explicit local owner recovery)\nget|conflicts|inspect CONTENT_ID\nexport NEW_DIRECTORY\noauth-register NAME REDIRECT_URI | oauth-consent REQUEST_ID\ncreate BODY_FILE REQUEST_FILE [KIND...]\nedit|restore CONTENT_ID TOKEN BODY_FILE REQUEST_FILE\ndelete|keep CONTENT_ID TOKEN REQUEST_FILE\nresolve CONTENT_ID TOKEN BODY_FILE REQUEST_FILE VERSION_ID...\nselect CONTENT_ID TOKEN SELECTIONS_JSON REQUEST_FILE\nmerge-base|merge-plan CONTENT_ID TOKEN LEFT_VERSION RIGHT_VERSION [BASE_VERSION]\nsave REQUEST_FILE\nRequest files require an owner-only parent directory and must not exist. Restore replaces all known fields; this command supplies only body.";
#[tokio::main]
async fn main() {
    match execute(&std::env::args().skip(1).collect::<Vec<_>>()).await {
        Ok(result) => println!("{result}"),
        Err(error) => {
            eprintln!("harmion: {error}");
            std::process::exit(1);
        }
    }
}
