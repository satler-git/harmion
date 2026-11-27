mod auth;
mod cli;
mod spool;

use auth::{Credentials, Pkce};
use clap::Parser;
use cli::{AuthCommand, Cli, Command, ConnectArgs, ListArgs, QArgs, SearchArgs, ShowArgs};
use harmion_client::{
    Client, protocol,
    protocol::{
        Command as SaveCommand, EditToken, RequestId, SaveRequest, domain::ContentId, unhex,
    },
    random_id,
};
use mote_core::{Capture, Filter, Sort};
use serde_json::{Value, json};
use spool::{PendingCapture, Spool};
use std::{
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use time::{OffsetDateTime, UtcOffset, macros::format_description};

type Result<T> = std::result::Result<T, Error>;

/// Leave room for the create envelope, attributes, and reply metadata under
/// the node's 1 MiB record limit.
const MAX_CAPTURE_BYTES: usize = 1024 * 1024 - 64 * 1024;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Client(#[from] harmion_client::Error),
    #[error(transparent)]
    Capture(#[from] mote_core::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Message(String),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => {}
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe => {}
        Err(error) => {
            eprintln!("mote: {}", render_error(&error));
            std::process::exit(exit_code(&error));
        }
    }
}

fn render_error(error: &Error) -> String {
    match error {
        Error::Client(harmion_client::Error::Http(401)) => {
            "authentication failed; refresh the capture-only token or use --owner for local admin"
                .into()
        }
        Error::Client(harmion_client::Error::Http(403)) => {
            "permission denied for this capture operation".into()
        }
        Error::Client(harmion_client::Error::Http(404)) => "capture not found".into(),
        Error::Client(harmion_client::Error::Http(410)) => {
            "query or edit token expired; retry the command".into()
        }
        Error::Client(harmion_client::Error::Http(413)) => {
            "capture is too large for the node".into()
        }
        Error::Client(harmion_client::Error::Http(code)) => {
            format!("node returned HTTP {code}")
        }
        Error::Client(harmion_client::Error::Transport) => {
            "cannot reach the node; is harmiond running?".into()
        }
        Error::Client(harmion_client::Error::Timeout) => "the node did not answer in time".into(),
        Error::Client(harmion_client::Error::InvalidDiscovery) => {
            "node directory has an invalid connection.json".into()
        }
        Error::Client(harmion_client::Error::InvalidRequest) => {
            "the node rejected the request; check the token and IDs".into()
        }
        Error::Client(harmion_client::Error::InvalidResponse) => {
            "the node returned an invalid response".into()
        }
        Error::Client(harmion_client::Error::Io(error)) => error.to_string(),
        Error::Client(harmion_client::Error::OAuth(code)) => {
            format!("token refresh was rejected ({code})")
        }
        Error::Client(harmion_client::Error::ProviderAttachRejected { code }) => {
            format!("provider rejected the attach ({code})")
        }
        Error::Client(harmion_client::Error::Signer(error)) => error.clone(),
        _ => error.to_string(),
    }
}

fn exit_code(error: &Error) -> i32 {
    match error {
        Error::Client(harmion_client::Error::Http(401 | 403)) => 5,
        Error::Client(harmion_client::Error::Http(404)) => 3,
        Error::Client(harmion_client::Error::Http(409)) => 7,
        Error::Client(harmion_client::Error::Transport | harmion_client::Error::Timeout) => 6,
        _ => 1,
    }
}

async fn run(cli: Cli) -> Result<()> {
    let tsv = match &cli.command {
        Command::List(args) => args.tsv,
        Command::Search(args) => args.tsv,
        Command::Show(args) => args.tsv,
        _ => false,
    };
    if tsv && cli.json {
        return Err(Error::Message(
            "--tsv cannot be combined with --json".into(),
        ));
    }
    let output = Output {
        json: cli.json,
        tsv,
        color: !cli.no_color
            && io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none(),
    };
    let clock = Clock::new();
    if matches!(&cli.command, Command::Pending) {
        return pending_cmd(output, &clock);
    }
    if let Command::Forget { id } = &cli.command {
        return forget_cmd(id, output);
    }
    let command = cli.command;
    if let Command::Connect(args) = command {
        return connect_cmd(args, cli.node, output).await;
    }
    if let Command::Auth { command } = command {
        return auth_cmd(command, cli.node, output).await;
    }
    // Capture must spool before touching the node or credentials, so a down
    // daemon or an auth error cannot lose the body.
    if let Command::Q(args) = command {
        return capture(args, cli.node, cli.token_file, cli.owner, output).await;
    }
    let client = open_client(cli.node, cli.token_file, cli.owner).await?;
    match command {
        Command::List(args) => list(&client, args, output, &clock).await,
        Command::Search(args) => search(&client, args, output, &clock).await,
        Command::Show(args) => show(&client, args, output, &clock).await,
        Command::Archive { ids } => mutate(&client, ids, Mutation::Archive, output).await,
        Command::Unarchive { ids } => mutate(&client, ids, Mutation::Unarchive, output).await,
        Command::Pin { ids } => mutate(&client, ids, Mutation::Pin, output).await,
        Command::Unpin { ids } => mutate(&client, ids, Mutation::Unpin, output).await,
        Command::Retry { id } => retry(&client, id, output).await,
        Command::Q(_) => unreachable!("capture is handled before client construction"),
        Command::Pending => unreachable!("pending is handled before auth"),
        Command::Forget { .. } => unreachable!("forget is handled before auth"),
        Command::Connect(_) => unreachable!("connect is handled before client construction"),
        Command::Auth { .. } => unreachable!("auth is handled before client construction"),
    }
}

#[derive(Clone, Copy)]
struct Output {
    json: bool,
    tsv: bool,
    color: bool,
}

async fn capture(
    args: QArgs,
    node: Option<String>,
    token_file: Option<PathBuf>,
    owner: bool,
    output: Output,
) -> Result<()> {
    let raw = match args.text {
        Some(text) if text != "-" => text,
        Some(_) => read_stdin_capture()?,
        None if io::stdin().is_terminal() => {
            return Err(Error::Message(
                "no text; pass TEXT, use `-` to read stdin, or pipe input".into(),
            ));
        }
        None => read_stdin_capture()?,
    };
    let body = prepare_body(raw)?;
    let pending = PendingCapture::new(body, args.reply_to)?;
    let spool = Spool::open()?;
    let path = spool.save(&pending)?;
    let client = match open_client(node, token_file, owner).await {
        Ok(client) => client,
        Err(error) => {
            eprintln!(
                "mote: request kept at {}; retry with `mote retry {}`",
                path.display(),
                pending.short_id()
            );
            return Err(error);
        }
    };
    match send_pending(&client, &pending).await {
        Ok((id, update_id)) => {
            if let Err(error) = spool.remove(&pending) {
                eprintln!(
                    "mote: saved {id}, but could not remove pending record {}: {error}",
                    path.display()
                );
            }
            print_saved(&id, &update_id, output)
        }
        Err(error) => {
            eprintln!(
                "mote: request kept at {}; retry with `mote retry {}`",
                path.display(),
                pending.short_id()
            );
            Err(error)
        }
    }
}

fn print_saved(id: &str, update_id: &str, output: Output) -> Result<()> {
    if output.json {
        writeln!(
            io::stdout(),
            "{}",
            serde_json::to_string_pretty(&json!({
                "id": id,
                "saved": true,
                "update_id": update_id,
            }))?
        )?;
    } else {
        writeln!(io::stdout(), "{id}")?;
    }
    Ok(())
}

async fn send_pending(client: &Client, pending: &PendingCapture) -> Result<(String, String)> {
    let reply_to = match pending.reply_to.as_deref() {
        Some(id) => Some(resolve(client, id).await?.id),
        None => None,
    };
    let content_id = ContentId(unhex::<32>(&pending.content_id).map_err(|_| {
        Error::Message(format!(
            "pending record has an invalid content ID: {}",
            pending.content_id
        ))
    })?);
    let request_id = RequestId(unhex::<32>(&pending.request_id).map_err(|_| {
        Error::Message(format!(
            "pending record has an invalid request ID: {}",
            pending.request_id
        ))
    })?);
    let request = SaveRequest {
        request_id,
        content_id,
        command: SaveCommand::Create {
            fields: mote_core::create_fields(
                &pending.body,
                pending.created_at_micros,
                reply_to.as_deref(),
            )?,
        },
    };
    let result = client.save(&request).await?;
    verify_save_result(&result)?;
    let update_id = result
        .get("update_id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Message("malformed save response: missing update_id".into()))?
        .to_owned();
    Ok((pending.content_id.clone(), update_id))
}

fn pending_cmd(output: Output, clock: &Clock) -> Result<()> {
    let pending = Spool::open()?.list()?;
    if output.json {
        let records: Vec<_> = pending
            .iter()
            .map(|record| {
                json!({
                    "id": record.content_id,
                    "created_at_micros": record.created_at_micros,
                    "reply_to": record.reply_to,
                    "body": record.body,
                })
            })
            .collect();
        writeln!(io::stdout(), "{}", serde_json::to_string_pretty(&records)?)?;
        return Ok(());
    }
    if pending.is_empty() {
        if !output.json {
            writeln!(io::stdout(), "no pending captures")?;
        }
        return Ok(());
    }
    for record in pending {
        let reply = record
            .reply_to
            .as_deref()
            .map(|id| format!(" -> {}", one_line(id)))
            .unwrap_or_default();
        writeln!(
            io::stdout(),
            "{}  {}  {}{}",
            clock.format(Some(record.created_at_micros)),
            record.short_id(),
            record.preview(72),
            reply
        )?;
    }
    Ok(())
}

fn forget_cmd(id: &str, output: Output) -> Result<()> {
    let spool = Spool::open()?;
    let pending = spool.find(id)?;
    spool.remove(&pending)?;
    if output.json {
        writeln!(
            io::stdout(),
            "{}",
            serde_json::to_string_pretty(&json!({
                "id": pending.content_id,
                "forgotten": true,
            }))?
        )?;
    } else {
        writeln!(io::stdout(), "forgot {}", pending.short_id())?;
    }
    Ok(())
}

async fn retry(client: &Client, id: Option<String>, output: Output) -> Result<()> {
    let spool = Spool::open()?;
    let pending = match id {
        Some(prefix) => vec![spool.find(&prefix)?],
        None => spool.list()?,
    };
    if pending.is_empty() {
        return Err(Error::Message("no pending captures".into()));
    }
    let mut results = Vec::new();
    let mut failures = 0;
    for record in pending {
        match send_pending(client, &record).await {
            Ok((id, update_id)) => {
                if let Err(error) = spool.remove(&record) {
                    eprintln!("mote: saved {id}, but could not remove pending record: {error}");
                }
                if !output.json {
                    writeln!(io::stdout(), "retried {id}")?;
                }
                results.push(json!({
                    "id": id,
                    "saved": true,
                    "update_id": update_id,
                }));
            }
            Err(error) => {
                failures += 1;
                eprintln!("mote: {}: {}", record.short_id(), render_error(&error));
                results.push(json!({
                    "id": record.content_id,
                    "error": render_error(&error),
                }));
            }
        }
    }
    if output.json {
        writeln!(io::stdout(), "{}", serde_json::to_string_pretty(&results)?)?;
    }
    if failures > 0 {
        return Err(Error::Message(format!(
            "{failures} pending capture(s) still require retry"
        )));
    }
    Ok(())
}

async fn connect_cmd(args: ConnectArgs, node: Option<String>, output: Output) -> Result<()> {
    let directory = resolve_mote_node(&node_spec(node)?)?;
    let info = auth::connection_info(&directory)?;
    // The integrated flow returns tokens from connect-complete. Bind a
    // loopback port only to satisfy the offer's redirect URI; no callback is
    // served.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/mote/callback");
    let Pkce {
        verifier,
        challenge,
    } = auth::generate_pkce()?;
    let state = auth::random_state()?;
    let offer = auth::offer(&args.name, &redirect_uri, &challenge, &state)?;
    let client = Client::anonymous(info.clone())?;
    let started = client.connect_start(&offer).await?;
    let connection_id = json_string(&started, "connection_id")?;
    let request_secret = json_string(&started, "request_secret")?;
    let verification_url = started
        .get("verification_url")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut output_handle: Box<dyn Write> = if output.json {
        Box::new(io::stderr())
    } else {
        Box::new(io::stdout())
    };
    writeln!(output_handle, "connection: {connection_id}")?;
    if let Some(url) = &verification_url {
        writeln!(output_handle, "Open this URL to approve Mote:")?;
        writeln!(output_handle, "  {url}")?;
    } else {
        writeln!(
            output_handle,
            "Approve this request in Harmion Admin > Connect (no verification URL was published)."
        )?;
    }
    writeln!(
        output_handle,
        "Waiting for owner approval (up to {}s)...",
        args.timeout
    )?;
    output_handle.flush()?;

    let deadline = Instant::now() + std::time::Duration::from_secs(args.timeout.max(1));
    let mut last_status = String::new();
    loop {
        let request = auth::status_request(&connection_id, &request_secret);
        let view = client.connect_status(&request).await?;
        let status = json_string(&view, "status")?;
        if status != last_status {
            writeln!(io::stderr(), "mote: connection status: {status}")?;
            last_status = status.clone();
        }
        match status.as_str() {
            "approved" => break,
            "denied" => {
                return Err(Error::Message(
                    "the owner denied the connection request".into(),
                ));
            }
            "expired" => {
                return Err(Error::Message(
                    "the connection request expired before approval; run `mote connect` again"
                        .into(),
                ));
            }
            "completed" => {
                return Err(Error::Message(
                    "the connection was already completed without saved credentials; run `mote connect` again"
                        .into(),
                ));
            }
            "pending" => {}
            other => {
                return Err(Error::Message(format!(
                    "unexpected connection status `{other}`"
                )));
            }
        }
        if Instant::now() >= deadline {
            return Err(Error::Message(format!(
                "timed out after {}s waiting for owner approval",
                args.timeout
            )));
        }
        tokio::time::sleep(std::time::Duration::from_secs(args.poll)).await;
    }

    let completion = client
        .connect_complete(&auth::complete_request(
            &connection_id,
            &request_secret,
            &verifier,
        ))
        .await?;
    let client_id = json_string(&completion, "client_id")?;
    let access_token = json_string(&completion, "access_token")?;
    let refresh_token = json_string(&completion, "refresh_token")?;
    let expires_in = completion
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| Error::Message("connect-complete returned no valid expires_in".into()))?;
    let scope = completion
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let credentials = Credentials {
        version: 1,
        client_id,
        access_token,
        refresh_token,
        expires_at: auth::now_seconds().saturating_add(expires_in),
        scope,
        oauth_origin: info.oauth_origin.clone(),
    };
    credentials.save(&directory)?;
    if info.oauth_origin.is_none() {
        eprintln!(
            "mote: warning: node published no OAuth origin; automatic refresh is unavailable and re-connect is required after expiry"
        );
    }
    if output.json {
        writeln!(
            io::stdout(),
            "{}",
            serde_json::to_string_pretty(&json!({
                "connected": true,
                "client_id": credentials.client_id,
                "expires_at": credentials.expires_at,
                "scope": credentials.scope,
                "refresh": info.oauth_origin.is_some(),
                "credentials_file": directory.join(auth::CREDENTIALS_FILE).display().to_string(),
            }))?
        )?;
    } else {
        writeln!(io::stdout(), "connected Mote to {}", info.endpoint)?;
        writeln!(
            io::stdout(),
            "credentials: {} (mode 0600)",
            directory.join(auth::CREDENTIALS_FILE).display()
        )?;
        writeln!(io::stdout(), "client_id: {}", credentials.client_id)?;
        writeln!(
            io::stdout(),
            "access token expires at unix {}; refresh {}",
            credentials.expires_at,
            if info.oauth_origin.is_some() {
                "enabled"
            } else {
                "unavailable"
            }
        )?;
    }
    Ok(())
}

async fn auth_cmd(command: AuthCommand, node: Option<String>, output: Output) -> Result<()> {
    let directory = resolve_mote_node(&node_spec(node)?)?;
    match command {
        AuthCommand::Status => {
            let credentials = Credentials::load(&directory)?;
            if output.json {
                let value = match &credentials {
                    Some(credentials) => json!({
                        "connected": true,
                        "client_id": credentials.client_id,
                        "expires_at": credentials.expires_at,
                        "scope": credentials.scope,
                        "refresh": credentials.oauth_origin.is_some(),
                    }),
                    None => json!({"connected": false}),
                };
                writeln!(io::stdout(), "{}", serde_json::to_string_pretty(&value)?)?;
            } else if let Some(credentials) = credentials {
                writeln!(io::stdout(), "connected to {}", directory.display())?;
                writeln!(io::stdout(), "client_id: {}", credentials.client_id)?;
                writeln!(
                    io::stdout(),
                    "access token expires at unix {} ({})",
                    credentials.expires_at,
                    if credentials.expires_at <= auth::now_seconds() {
                        "expired"
                    } else {
                        "valid"
                    }
                )?;
                writeln!(
                    io::stdout(),
                    "refresh: {}",
                    if credentials.oauth_origin.is_some() {
                        "enabled"
                    } else {
                        "unavailable"
                    }
                )?;
                writeln!(io::stdout(), "scope: {}", one_line(&credentials.scope))?;
            } else {
                writeln!(io::stdout(), "not connected; run `mote connect`")?;
            }
        }
        AuthCommand::Logout => match Credentials::load(&directory)? {
            Some(mut credentials) => {
                revoke_credentials(&directory, &mut credentials).await?;
                Credentials::clear(&directory)?;
                if output.json {
                    writeln!(
                        io::stdout(),
                        "{}",
                        json!({"connected": false, "revoked": true})
                    )?;
                } else {
                    writeln!(io::stdout(), "logged out; node grant revoked")?;
                }
            }
            None => {
                if output.json {
                    writeln!(
                        io::stdout(),
                        "{}",
                        json!({"connected": false, "revoked": false})
                    )?;
                } else {
                    writeln!(io::stdout(), "not connected")?;
                }
            }
        },
    }
    Ok(())
}

async fn revoke_credentials(directory: &Path, credentials: &mut Credentials) -> Result<()> {
    if credentials.expires_at <= auth::now_seconds() {
        if !credentials.refresh_available() {
            return Err(Error::Message(
                "access token expired and no OAuth origin is available; cannot revoke the node grant. Revoke the app in Harmion Admin, then remove the local credential."
                    .into(),
            ));
        }
        credentials.refresh(directory).await?;
    } else if credentials.needs_refresh()
        && credentials.refresh_available()
        && let Err(error) = credentials.refresh(directory).await
    {
        eprintln!(
            "mote: warning: OAuth refresh failed; trying the current access token: {}",
            render_error(&error)
        );
    }
    let client = Client::from_directory_with_token(directory, credentials.token())?;
    match client.revoke_self().await {
        Ok(_) => Ok(()),
        // The family may already have been revoked or expired. Local cleanup is
        // still safe in that case.
        Err(harmion_client::Error::Http(401)) => Ok(()),
        Err(error) => Err(Error::Message(format!(
            "node grant revocation failed: {}; local credential kept so logout can be retried",
            render_error(&Error::Client(error))
        ))),
    }
}

fn json_string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::Message(format!("node response is missing string field `{key}`")))
}
async fn list(client: &Client, args: ListArgs, output: Output, clock: &Clock) -> Result<()> {
    let filter = Filter {
        archive: args.archive_filter(),
        pinned_only: args.pinned,
    };
    let (mut captures, total) = select(
        fetch_all(client, mote_core::query_condition(&filter)).await?,
        filter,
        args.search.as_deref(),
        args.order(),
        args.reverse,
        args.limit,
    );
    if args.limit > 0 && total > captures.len() {
        eprintln!(
            "mote: showing {} of {}; use --limit 0 for all",
            captures.len(),
            total
        );
    }
    print_captures(&mut captures, output, clock)
}

async fn search(client: &Client, args: SearchArgs, output: Output, clock: &Clock) -> Result<()> {
    if args.query.trim().is_empty() {
        return Err(Error::Message("search query is empty".into()));
    }
    let filter = Filter {
        archive: args.archive_filter(),
        pinned_only: args.pinned,
    };
    let (mut captures, total) = select(
        fetch_all(client, mote_core::query_condition(&filter)).await?,
        filter,
        Some(&args.query),
        args.order(),
        args.reverse,
        args.limit,
    );
    if args.limit > 0 && total > captures.len() {
        eprintln!(
            "mote: showing {} of {}; use --limit 0 for all",
            captures.len(),
            total
        );
    }
    print_captures(&mut captures, output, clock)
}

fn select(
    captures: Vec<Capture>,
    filter: Filter,
    query: Option<&str>,
    order: Sort,
    reverse: bool,
    limit: usize,
) -> (Vec<Capture>, usize) {
    let mut captures: Vec<_> = captures
        .into_iter()
        .filter(|capture| filter.matches(capture))
        .filter(|capture| query.is_none_or(|query| capture.matches(query)))
        .collect();
    let total = captures.len();
    mote_core::sort_captures(&mut captures, order);
    if reverse {
        captures.reverse();
    }
    if limit > 0 {
        captures.truncate(limit);
    }
    (captures, total)
}

async fn show(client: &Client, args: ShowArgs, output: Output, clock: &Clock) -> Result<()> {
    if args.meta && (output.json || output.tsv) {
        return Err(Error::Message(
            "--meta cannot be combined with --json or --tsv".into(),
        ));
    }
    let capture = resolve(client, &args.id).await?;
    if output.json {
        writeln!(io::stdout(), "{}", serde_json::to_string_pretty(&capture)?)?;
        return Ok(());
    }
    if output.tsv {
        print_tsv(&capture)?;
        return Ok(());
    }
    if args.meta {
        print_meta(&capture, clock)?;
    }
    let stdout = io::stdout();
    if stdout.is_terminal() {
        let text = terminal_text(&capture.body);
        stdout.lock().write_all(text.as_bytes())?;
        if !text.ends_with('\n') {
            writeln!(io::stdout())?;
        }
    } else {
        stdout.lock().write_all(capture.body.as_bytes())?;
    }
    Ok(())
}

async fn mutate(
    client: &Client,
    ids: Vec<String>,
    mutation: Mutation,
    output: Output,
) -> Result<()> {
    if ids.is_empty() {
        return Err(Error::Message("no capture IDs given".into()));
    }
    let mut results = Vec::new();
    let mut failures = 0;
    for id in ids {
        match apply_mutation(client, &id, mutation).await {
            Ok(result) => {
                if !output.json {
                    writeln!(
                        io::stdout(),
                        "{} {}{}",
                        mutation.label(),
                        result["short_id"].as_str().unwrap_or(&id),
                        if result["changed"].as_bool().unwrap_or(false) {
                            ""
                        } else {
                            " (unchanged)"
                        }
                    )?;
                }
                results.push(result);
            }
            Err(error) => {
                failures += 1;
                eprintln!("mote: {id}: {}", render_error(&error));
                results.push(json!({
                    "id": id,
                    "action": mutation.label(),
                    "error": render_error(&error),
                }));
            }
        }
    }
    if output.json {
        writeln!(io::stdout(), "{}", serde_json::to_string_pretty(&results)?)?;
    }
    if failures > 0 {
        return Err(Error::Message(format!(
            "{failures} operation(s) failed; the other changes remain committed"
        )));
    }
    Ok(())
}

async fn apply_mutation(client: &Client, id: &str, mutation: Mutation) -> Result<Value> {
    let capture = resolve(client, id).await?;
    if capture.session_open && matches!(mutation, Mutation::Archive) {
        return Err(Error::Message(
            "capture session is open; complete it on the creating device before archiving".into(),
        ));
    }
    if capture.conflicted || capture.deletion_conflict {
        return Err(Error::Message("capture has unresolved conflicts".into()));
    }
    let changed = mutation.changes(&capture);
    if changed {
        let token = capture
            .token()
            .ok_or_else(|| Error::Message("read view has no edit token".into()))?;
        let fields = match mutation {
            Mutation::Archive => mote_core::archive_fields(true, Some(now_micros()?))?,
            Mutation::Unarchive => mote_core::archive_fields(false, None)?,
            Mutation::Pin => mote_core::pin_fields(true),
            Mutation::Unpin => mote_core::pin_fields(false),
        };
        let request =
            SaveRequest {
                request_id: RequestId(random_id()?),
                content_id: content_id(&capture.id)?,
                command: SaveCommand::Edit {
                    token: EditToken(unhex::<32>(token).map_err(|_| {
                        Error::Message("read view has an invalid edit token".into())
                    })?),
                    fields,
                },
            };
        let result = client.save(&request).await?;
        verify_save_result(&result)?;
        if !result.get("view").is_some_and(Value::is_object) {
            return Err(Error::Message(
                "change was accepted but the node returned no read view; verify with `mote show`"
                    .into(),
            ));
        }
        if let Some(short_id) = save_conflicts(&result)? {
            return Err(Error::Message(format!(
                "{short_id} was accepted with conflicts; verify before trusting the change"
            )));
        }
    }
    Ok(json!({
        "id": capture.id,
        "short_id": capture.short_id(),
        "action": mutation.label(),
        "changed": changed,
    }))
}

fn verify_save_result(result: &Value) -> Result<()> {
    let saved = result
        .get("saved")
        .and_then(Value::as_bool)
        .ok_or_else(|| Error::Message("malformed save response: missing saved".into()))?;
    if !saved {
        return Err(Error::Message("node did not confirm the change".into()));
    }
    result
        .get("update_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::Message("malformed save response: missing update_id".into()))?;
    Ok(())
}

fn save_conflicts(result: &Value) -> Result<Option<String>> {
    let Some(view) = result.get("view") else {
        return Ok(None);
    };
    if view.is_null() {
        return Ok(None);
    }
    let capture = Capture::from_view(view)?;
    Ok((capture.conflicted || capture.deletion_conflict).then(|| capture.short_id().to_owned()))
}

#[derive(Clone, Copy)]
enum Mutation {
    Archive,
    Unarchive,
    Pin,
    Unpin,
}

impl Mutation {
    fn label(self) -> &'static str {
        match self {
            Self::Archive => "archived",
            Self::Unarchive => "unarchived",
            Self::Pin => "pinned",
            Self::Unpin => "unpinned",
        }
    }

    fn changes(self, capture: &Capture) -> bool {
        match self {
            Self::Archive => !capture.archived,
            Self::Unarchive => capture.archived,
            Self::Pin => !capture.pinned,
            Self::Unpin => capture.pinned,
        }
    }
}

async fn fetch_all(client: &Client, condition: protocol::query::Condition) -> Result<Vec<Capture>> {
    const PAGE: u32 = 32;
    const MAX_CAPTURES: usize = 100_000;
    let mut restarted = false;
    'pages: loop {
        let mut page = client.query(condition.clone(), PAGE).await?;
        let mut captures = Vec::new();
        let mut skipped = 0usize;
        loop {
            for item in &page.items {
                match Capture::from_view(item) {
                    Ok(capture) => captures.push(capture),
                    Err(_) => skipped += 1,
                }
                if captures.len() > MAX_CAPTURES {
                    return Err(Error::Message(format!(
                        "more than {MAX_CAPTURES} captures; narrow the query"
                    )));
                }
            }
            match page.next {
                Some(cursor) => match client.query_next(cursor, PAGE).await {
                    Ok(next) => page = next,
                    // Query cursors live for five minutes. If a long scan
                    // outlives one, restart from a fresh fixed snapshot once.
                    Err(harmion_client::Error::Http(410)) if !restarted => {
                        restarted = true;
                        continue 'pages;
                    }
                    Err(error) => return Err(error.into()),
                },
                None => {
                    if skipped > 0 {
                        eprintln!("mote: skipped {skipped} malformed capture view(s)");
                    }
                    return Ok(captures);
                }
            }
        }
    }
}

async fn resolve(client: &Client, value: &str) -> Result<Capture> {
    let id = normalize_id(value)?;
    if id.len() == 64 {
        let view = client
            .get(ContentId(unhex::<32>(&id).map_err(|_| {
                Error::Message(format!("invalid content ID `{value}`"))
            })?))
            .await?;
        let capture = Capture::from_view(&view)
            .map_err(|error| Error::Message(format!("content {id}: {error}")))?;
        if capture.id != id {
            return Err(Error::Message(format!(
                "node returned a different content ID for `{value}`"
            )));
        }
        return Ok(capture);
    }
    if id.len() < 4 {
        return Err(Error::Message(
            "content ID prefix must be at least 4 hex characters".into(),
        ));
    }
    let captures = fetch_all(
        client,
        protocol::query::Condition::Kind(mote_core::CAPTURE_KIND.into()),
    )
    .await?;
    let mut matches: Vec<Capture> = captures
        .into_iter()
        .filter(|capture| capture.id.starts_with(&id))
        .collect();
    match matches.len() {
        0 => Err(Error::Message(format!("no capture matches `{value}`"))),
        1 => {
            let selected = matches.remove(0);
            // A query item is a fixed, possibly older snapshot. Mutations need
            // the current edit token and attributes, so refresh before returning.
            let view = client.get(content_id(&selected.id)?).await?;
            let capture = Capture::from_view(&view)
                .map_err(|error| Error::Message(format!("content {}: {error}", selected.id)))?;
            if capture.id != selected.id {
                return Err(Error::Message(format!(
                    "node returned a different content ID for `{value}`"
                )));
            }
            Ok(capture)
        }
        _ => Err(Error::Message(format!(
            "`{value}` is ambiguous: {}",
            matches
                .iter()
                .map(|capture| capture.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn normalize_id(value: &str) -> Result<String> {
    let id = value.trim().to_ascii_lowercase();
    if id.len() < 4 || id.len() > 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Message(format!("invalid content ID `{value}`")));
    }
    Ok(id)
}

fn content_id(value: &str) -> Result<ContentId> {
    Ok(ContentId(unhex::<32>(value).map_err(|_| {
        Error::Message("invalid content ID".into())
    })?))
}

fn now_micros() -> Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Message("system clock is before the Unix epoch".into()))
        .and_then(|duration| {
            i64::try_from(duration.as_micros())
                .map_err(|_| Error::Message("system clock is out of range".into()))
        })
}

fn resolve_mote_node(spec: &str) -> Result<std::path::PathBuf> {
    let path = Path::new(spec);
    if path.is_absolute() || spec.contains('/') {
        return Ok(harmion_local::resolve_node(spec)?);
    }
    // A bare name is a node under the Harmion root, never a cwd-relative
    // directory. Required to keep `--node main` deterministic in any cwd.
    let named = harmion_local::node_root()?.join(spec);
    if named.is_dir() {
        return Ok(named);
    }
    Err(Error::Message(format!(
        "node `{spec}` not found under {}",
        harmion_local::node_root()?.display()
    )))
}

async fn open_client(
    node: Option<String>,
    token_file: Option<PathBuf>,
    owner: bool,
) -> Result<Client> {
    let node_spec = node_spec(node)?;
    let token = match token_file {
        Some(path) => Some(read_token_file(&path)?),
        None => env_value("MOTE_TOKEN")?,
    };
    if owner && token.is_some() {
        return Err(Error::Message(
            "--owner cannot be combined with a capture-only token".into(),
        ));
    }
    let directory = resolve_mote_node(&node_spec)?;
    match token {
        Some(token) => {
            let token = token.trim().to_owned();
            validate_token(&token)?;
            Ok(Client::from_directory_with_token(&directory, &token)?)
        }
        None if owner => {
            eprintln!(
                "mote: warning: using the node admin credential (--owner); this is wider than capture-only"
            );
            Ok(Client::from_directory(&directory)?)
        }
        None => {
            let mut credentials = Credentials::load(&directory)?.ok_or_else(|| {
                Error::Message(format!(
                    "no Mote credentials for {}; run `mote connect` or pass --owner",
                    directory.display()
                ))
            })?;
            if credentials.expires_at <= auth::now_seconds() {
                if !credentials.refresh_available() {
                    return Err(Error::Message(
                        "Mote access token expired and no OAuth origin is available; run `mote connect` again"
                            .into(),
                    ));
                }
                credentials.refresh(&directory).await?;
            } else if credentials.needs_refresh()
                && credentials.refresh_available()
                && let Err(error) = credentials.refresh(&directory).await
            {
                eprintln!(
                    "mote: warning: OAuth refresh failed; using the current access token: {}",
                    render_error(&error)
                );
            }
            Ok(Client::from_directory_with_token(
                &directory,
                credentials.token(),
            )?)
        }
    }
}

fn node_spec(node: Option<String>) -> Result<String> {
    match node.or(env_value("MOTE_NODE")?) {
        Some(node) if !node.trim().is_empty() => Ok(node),
        Some(_) => Err(Error::Message(
            "--node/MOTE_NODE is empty; refusing to guess a node".into(),
        )),
        None => Ok("main".to_owned()),
    }
}

fn env_value(name: &str) -> Result<Option<String>> {
    match std::env::var_os(name) {
        None => Ok(None),
        Some(value) => value
            .into_string()
            .map(Some)
            .map_err(|_| Error::Message(format!("{name} is not valid UTF-8"))),
    }
}

fn validate_token(token: &str) -> Result<()> {
    if token.is_empty() {
        return Err(Error::Message(
            "capture-only token is empty; refusing the owner fallback".into(),
        ));
    }
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Message(
            "capture-only token must be a 64-character hex string".into(),
        ));
    }
    Ok(())
}

fn read_token_file(path: &Path) -> Result<String> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(Error::Message(
            "token file must be a regular file, not a symlink".into(),
        ));
    }
    if metadata.len() > 256 {
        return Err(Error::Message("token file is too large".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::Message(
                "token file must not be readable or writable by group/other (chmod 600)".into(),
            ));
        }
    }
    let token = std::fs::read_to_string(path)?;
    let token = token.trim().to_owned();
    validate_token(&token)?;
    Ok(token)
}

fn read_stdin_capture() -> Result<String> {
    let mut body = String::new();
    io::stdin()
        .take(MAX_CAPTURE_BYTES as u64 + 1)
        .read_to_string(&mut body)?;
    Ok(body)
}

fn trim_body(mut body: String) -> String {
    if body.ends_with("\r\n") {
        body.truncate(body.len() - 2);
    } else if body.ends_with('\n') {
        body.pop();
    }
    body
}

fn prepare_body(raw: String) -> Result<String> {
    let overlength = raw.len() > MAX_CAPTURE_BYTES;
    let body = trim_body(raw);
    if body.trim().is_empty() {
        return Err(Error::Message("empty capture".into()));
    }
    if overlength || body.len() > MAX_CAPTURE_BYTES {
        return Err(Error::Message(format!(
            "capture exceeds {MAX_CAPTURE_BYTES} bytes"
        )));
    }
    Ok(body)
}

fn print_captures(captures: &mut [Capture], output: Output, clock: &Clock) -> Result<()> {
    if output.json {
        writeln!(io::stdout(), "{}", serde_json::to_string_pretty(captures)?)?;
        return Ok(());
    }
    if output.tsv {
        for capture in captures {
            print_tsv(capture)?;
        }
        return Ok(());
    }
    for capture in captures {
        let time = clock.format(capture.created_at_micros);
        let id = paint(output.color, "36", capture.short_id());
        let flags = flags(capture);
        let flags = if output.color && !flags.trim().is_empty() {
            paint(output.color, "33", &flags)
        } else {
            flags
        };
        let preview = capture.preview(72);
        writeln!(io::stdout(), "{time}  {id}  {flags:<4}  {preview}")?;
    }
    Ok(())
}

fn print_tsv(capture: &Capture) -> Result<()> {
    writeln!(
        io::stdout(),
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        capture.id,
        capture
            .created_at_micros
            .map(|value| value.to_string())
            .unwrap_or_default(),
        u8::from(capture.archived),
        capture
            .archived_at_micros
            .map(|value| value.to_string())
            .unwrap_or_default(),
        u8::from(capture.pinned),
        u8::from(capture.session_open),
        one_line(capture.reply_to.as_deref().unwrap_or("")),
        one_line(&capture.body),
    )?;
    Ok(())
}

fn print_meta(capture: &Capture, clock: &Clock) -> Result<()> {
    writeln!(io::stdout(), "ID: {}", capture.id)?;
    writeln!(
        io::stdout(),
        "Created: {}",
        clock.format(capture.created_at_micros)
    )?;
    writeln!(
        io::stdout(),
        "Archived: {}{}",
        if capture.archived { "yes" } else { "no" },
        capture
            .archived_at_micros
            .map(|time| format!(" ({})", clock.format(Some(time))))
            .unwrap_or_default()
    )?;
    writeln!(
        io::stdout(),
        "Pinned: {}",
        if capture.pinned { "yes" } else { "no" }
    )?;
    writeln!(
        io::stdout(),
        "Session: {}",
        if capture.session_open {
            "open"
        } else {
            "closed"
        }
    )?;
    if let Some(reply_to) = &capture.reply_to {
        writeln!(io::stdout(), "Reply to: {}", one_line(reply_to))?;
    }
    writeln!(
        io::stdout(),
        "Kinds: {}",
        one_line(&capture.kinds.join(", "))
    )?;
    if capture.conflicted || capture.deletion_conflict {
        writeln!(
            io::stdout(),
            "Conflicts: {}{}",
            if capture.conflicted { "field" } else { "" },
            if capture.deletion_conflict {
                if capture.conflicted {
                    ", deletion"
                } else {
                    "deletion"
                }
            } else {
                ""
            }
        )?;
    }
    writeln!(io::stdout())?;
    Ok(())
}

fn flags(capture: &Capture) -> String {
    format!(
        "{}{}{}{}",
        if capture.archived { 'A' } else { ' ' },
        if capture.pinned { 'P' } else { ' ' },
        if capture.session_open { 'L' } else { ' ' },
        if capture.conflicted || capture.deletion_conflict {
            '!'
        } else {
            ' '
        }
    )
}

fn one_line(body: &str) -> String {
    body.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn terminal_text(body: &str) -> String {
    body.chars()
        .map(|character| {
            if character == '\n' {
                '\n'
            } else if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn paint(enabled: bool, code: &str, text: &str) -> String {
    if enabled {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

struct Clock {
    offset: UtcOffset,
}

impl Clock {
    fn new() -> Self {
        let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
        Self { offset }
    }

    fn format(&self, micros: Option<i64>) -> String {
        const FORMAT: &[time::format_description::FormatItem<'static>] =
            format_description!("[year]-[month]-[day] [hour]:[minute]");
        let Some(micros) = micros else {
            return "-".into();
        };
        let seconds = micros.div_euclid(1_000_000);
        OffsetDateTime::from_unix_timestamp(seconds)
            .ok()
            .map(|time| time.to_offset(self.offset))
            .and_then(|time| time.format(FORMAT).ok())
            .unwrap_or_else(|| micros.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn capture() -> Capture {
        Capture::from_view(&json!({
            "content": {
                "id": "11".repeat(32),
                "body": "one\ntwo\tthree",
                "kinds": ["capture"],
                "attributes": [
                    {"namespace":"capture","key":"created_at_micros","value":1_700_000_000_000_000i64},
                    {"namespace":"capture","key":"pinned","value":true}
                ]
            },
            "edit_token": "22".repeat(32),
            "conflicts": [],
            "deletion_conflict": false
        }))
        .unwrap()
    }

    #[test]
    fn body_trim_removes_only_one_line_ending() {
        assert_eq!(trim_body("text \n\n".into()), "text \n");
        assert_eq!(trim_body("text\r\n".into()), "text");
        assert_eq!(trim_body("text".into()), "text");
    }

    #[test]
    fn tsv_collapses_control_separators() {
        assert_eq!(one_line("one\ntwo\tthree"), "one two three");
    }

    #[test]
    fn terminal_text_keeps_newlines_but_drops_escapes() {
        assert_eq!(terminal_text("a\x1b[31m\nb"), "a [31m\nb");
    }

    #[test]
    fn token_validation_rejects_empty_and_short_values() {
        assert!(validate_token(&"a".repeat(64)).is_ok());
        assert!(validate_token("").is_err());
        assert!(validate_token("abc").is_err());
        assert!(validate_token(&"z".repeat(64)).is_err());
    }

    #[test]
    fn maps_http_failures_to_exit_codes() {
        assert_eq!(
            exit_code(&Error::Client(harmion_client::Error::Http(404))),
            3
        );
        assert_eq!(
            exit_code(&Error::Client(harmion_client::Error::Http(401))),
            5
        );
        assert!(render_error(&Error::Client(harmion_client::Error::Http(410))).contains("expired"));
    }

    #[test]
    fn capture_body_at_limit_with_a_newline_is_rejected() {
        let mut raw = "a".repeat(MAX_CAPTURE_BYTES);
        raw.push('\n');
        assert!(prepare_body(raw).is_err());
        assert_eq!(
            prepare_body("a".repeat(MAX_CAPTURE_BYTES)).unwrap().len(),
            MAX_CAPTURE_BYTES
        );
    }

    #[test]
    fn flags_include_state() {
        let mut capture = capture();
        capture.archived = true;
        capture.session_open = true;
        assert_eq!(flags(&capture), "APL ");
    }

    #[test]
    fn prefix_and_full_id_are_normalized() {
        assert_eq!(normalize_id("  AABB  ").unwrap(), "aabb");
        assert!(normalize_id("not-hex").is_err());
    }
}
