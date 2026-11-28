//! The data behind the HUD: credentials, the node read, the local outbox and the
//! snapshot shown before the node answers.

use anti_webui::prelude::{SharedSignal, shared_signal};
use harmion_client::protocol::{
    Command as SaveCommand, EditToken, RequestId, SaveRequest, domain::ContentId, unhex,
};
use harmion_client::{Client, random_id};
use mote::auth::Credentials;
use mote::spool::{PendingCapture, Spool};
use mote_core::{ArchiveFilter, Capture, Filter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, RecvTimeoutError, Sender},
    },
    time::Duration,
};

const SNAPSHOT_VERSION: u8 = 1;
const SNAPSHOT_FILE: &str = "hud-snapshot.json";
const REFRESH_INTERVAL: Duration = Duration::from_secs(20);
const PAGE: u32 = 32;
const MAX_CAPTURES: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Connected,
    Offline,
    NoCredentials,
}

#[derive(Clone)]
pub struct Model {
    /// Newest first, archived included so filters stay local.
    pub captures: Vec<Capture>,
    pub pending: Vec<PendingCapture>,
    pub status: Status,
    pub last_sync: Option<i64>,
    pub error: Option<String>,
    pub from_cache: bool,
}

impl Model {
    fn loading() -> Self {
        Self {
            captures: Vec::new(),
            pending: Vec::new(),
            status: Status::Connecting,
            last_sync: None,
            error: None,
            from_cache: false,
        }
    }

    pub fn inbox_len(&self) -> usize {
        self.captures.iter().filter(|item| !item.archived).count()
    }
}

#[derive(Clone, Debug)]
pub enum Command {
    Refresh,
    Archive(String),
    Unarchive(String),
    Pin(String, bool),
    Delete(String),
    Retry,
}

#[derive(Clone)]
pub struct Session {
    model: SharedSignal<Arc<Model>>,
    commands: Sender<Command>,
}

impl Session {
    pub fn start(node: Option<String>) -> Self {
        let model = shared_signal(Arc::new(Model::loading()));
        let (commands, receiver) = mpsc::channel();
        let worker = model.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("the Mote session runtime");
            runtime.block_on(async move {
                if let Some((views, saved_at)) = load_snapshot() {
                    let mut next = (*worker.get()).clone();
                    next.captures = parse_views(&views);
                    next.from_cache = true;
                    next.status = Status::Offline;
                    next.last_sync = Some(saved_at);
                    next.pending = pending_now();
                    worker.set(Arc::new(next));
                }
                refresh(&worker, &node).await;
                loop {
                    match receiver.recv_timeout(REFRESH_INTERVAL) {
                        Ok(Command::Refresh) | Err(RecvTimeoutError::Timeout) => {
                            refresh(&worker, &node).await;
                            if !worker.get().pending.is_empty() {
                                retry_pending(&worker, &node).await;
                            }
                        }
                        Ok(Command::Archive(id)) => {
                            mutate(&worker, &node, &id, Change::Archive).await;
                        }
                        Ok(Command::Unarchive(id)) => {
                            mutate(&worker, &node, &id, Change::Unarchive).await;
                        }
                        Ok(Command::Pin(id, pinned)) => {
                            mutate(&worker, &node, &id, Change::Pin(pinned)).await;
                        }
                        Ok(Command::Delete(id)) => {
                            delete_capture(&worker, &node, &id).await;
                        }
                        Ok(Command::Retry) => {
                            retry_pending(&worker, &node).await;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
        });
        Self { model, commands }
    }

    pub fn model(&self) -> SharedSignal<Arc<Model>> {
        self.model.clone()
    }

    pub fn refresh(&self) {
        let _ = self.commands.send(Command::Refresh);
    }

    pub fn archive(&self, id: &str) {
        let _ = self.commands.send(Command::Archive(id.to_owned()));
    }

    pub fn unarchive(&self, id: &str) {
        let _ = self.commands.send(Command::Unarchive(id.to_owned()));
    }

    pub fn pin(&self, id: &str, pinned: bool) {
        let _ = self.commands.send(Command::Pin(id.to_owned(), pinned));
    }

    pub fn delete(&self, id: &str) {
        let _ = self.commands.send(Command::Delete(id.to_owned()));
    }

    pub fn retry(&self) {
        let _ = self.commands.send(Command::Retry);
    }
}

async fn delete_capture(model: &SharedSignal<Arc<Model>>, node: &Option<String>, id: &str) {
    let Some(before) = model
        .get()
        .captures
        .iter()
        .find(|capture| capture.id == id)
        .cloned()
    else {
        return;
    };
    let Some(token) = before.token().map(str::to_owned) else {
        publish_error(
            model,
            Status::Offline,
            "接続してから操作してください".into(),
        );
        return;
    };
    let mut next = (*model.get()).clone();
    next.captures.retain(|capture| capture.id != id);
    model.set(Arc::new(next));

    let result = async {
        let directory = node_dir(node)?;
        let client = client_for(&directory).await?;
        let request = SaveRequest {
            request_id: RequestId(random_id().map_err(|error| format!("{error:?}"))?),
            content_id: ContentId(unhex::<32>(id).map_err(|_| "invalid content ID".to_owned())?),
            command: SaveCommand::Delete {
                token: EditToken(unhex::<32>(&token).map_err(|_| "invalid edit token".to_owned())?),
            },
        };
        let result = client
            .save(&request)
            .await
            .map_err(|error| format!("{error:?}"))?;
        if !result
            .get("saved")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err("ノードが削除を確認しませんでした".to_owned());
        }
        Ok(())
    }
    .await;

    match result {
        Ok(()) => refresh(model, node).await,
        Err(error) => {
            let mut rollback = (*model.get()).clone();
            rollback.captures.push(before);
            rollback.captures.sort_by(|left, right| {
                right
                    .created_at()
                    .cmp(&left.created_at())
                    .then_with(|| right.id.cmp(&left.id))
            });
            model.set(Arc::new(rollback));
            publish_error(model, Status::Connected, error);
        }
    }
}

async fn retry_pending(model: &SharedSignal<Arc<Model>>, node: &Option<String>) {
    let Ok(directory) = node_dir(node) else {
        return;
    };
    let Ok(client) = client_for(&directory).await else {
        return;
    };
    let Ok(spool) = Spool::open() else {
        return;
    };
    let Ok(_lock) = spool.lock() else {
        return;
    };
    let Ok(records) = spool.list() else {
        return;
    };
    for record in records {
        match send_pending(&client, &record).await {
            Ok(()) => {
                let _ = spool.remove(&record);
            }
            Err(error) => eprintln!("mote-hud: pending {}: {error}", record.short_id()),
        }
    }
    refresh(model, node).await;
}

async fn send_pending(client: &Client, pending: &PendingCapture) -> Result<(), String> {
    let request = SaveRequest {
        request_id: RequestId(
            unhex::<32>(&pending.request_id).map_err(|_| "invalid request ID".to_owned())?,
        ),
        content_id: ContentId(
            unhex::<32>(&pending.content_id).map_err(|_| "invalid content ID".to_owned())?,
        ),
        command: SaveCommand::Create {
            fields: mote_core::create_fields(
                &pending.body,
                pending.created_at_micros,
                pending.reply_to.as_deref(),
            )
            .map_err(|error| error.to_string())?,
        },
    };
    let result = client
        .save(&request)
        .await
        .map_err(|error| format!("{error:?}"))?;
    if !result
        .get("saved")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err("ノードが保存を確認しませんでした".into());
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Change {
    Archive,
    Unarchive,
    Pin(bool),
}

async fn mutate(model: &SharedSignal<Arc<Model>>, node: &Option<String>, id: &str, change: Change) {
    let Some(before) = model
        .get()
        .captures
        .iter()
        .find(|capture| capture.id == id)
        .cloned()
    else {
        return;
    };
    if before.token().is_none() {
        publish_error(
            model,
            Status::Offline,
            "接続してから操作してください".into(),
        );
        return;
    }
    if before.session_open && matches!(change, Change::Archive) {
        publish_error(
            model,
            Status::Connected,
            "長文の進行中はアーカイブできません".into(),
        );
        return;
    }
    let mut next = (*model.get()).clone();
    if let Some(capture) = next.captures.iter_mut().find(|capture| capture.id == id) {
        match change {
            Change::Archive => {
                capture.archived = true;
                capture.archived_at_micros = Some(mote::now_micros().unwrap_or(0));
            }
            Change::Unarchive => {
                capture.archived = false;
                capture.archived_at_micros = None;
            }
            Change::Pin(pinned) => capture.pinned = pinned,
        }
    }
    model.set(Arc::new(next));

    match save_change(node, &before, change).await {
        Ok(()) => refresh(model, node).await,
        Err(error) => {
            let mut rollback = (*model.get()).clone();
            if let Some(capture) = rollback
                .captures
                .iter_mut()
                .find(|capture| capture.id == id)
            {
                *capture = before;
            }
            rollback.error = Some(error.clone());
            model.set(Arc::new(rollback));
            publish_error(model, Status::Connected, error);
        }
    }
}

async fn save_change(
    node: &Option<String>,
    capture: &Capture,
    change: Change,
) -> Result<(), String> {
    let directory = node_dir(node)?;
    let client = client_for(&directory).await?;
    let token = capture
        .token()
        .ok_or_else(|| "読み取り専用のキャッシュです".to_owned())?;
    let fields = match change {
        Change::Archive => {
            mote_core::archive_fields(true, Some(mote::now_micros().map_err(|e| e.to_string())?))
                .map_err(|error| error.to_string())?
        }
        Change::Unarchive => {
            mote_core::archive_fields(false, None).map_err(|error| error.to_string())?
        }
        Change::Pin(pinned) => mote_core::pin_fields(pinned),
    };
    let request = SaveRequest {
        request_id: RequestId(random_id().map_err(|error| format!("{error:?}"))?),
        content_id: ContentId(
            unhex::<32>(&capture.id).map_err(|_| "invalid content ID".to_owned())?,
        ),
        command: SaveCommand::Edit {
            token: EditToken(unhex::<32>(token).map_err(|_| "invalid edit token".to_owned())?),
            fields,
        },
    };
    let result = client
        .save(&request)
        .await
        .map_err(|error| format!("{error:?}"))?;
    if !result
        .get("saved")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err("ノードが変更を確認しませんでした".into());
    }
    Ok(())
}

async fn client_for(directory: &Path) -> Result<Client, String> {
    let mut credentials = Credentials::load(directory)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Mote の認証がありません".to_owned())?;
    if credentials.needs_refresh()
        && credentials.refresh_available()
        && let Err(error) = credentials.refresh(directory).await
    {
        return Err(error.to_string());
    }
    Client::from_directory_with_token(directory, credentials.token())
        .map_err(|error| error.to_string())
}

async fn refresh(model: &SharedSignal<Arc<Model>>, node: &Option<String>) {
    let directory = match node_dir(node) {
        Ok(directory) => directory,
        Err(error) => {
            publish_error(model, Status::Offline, error);
            return;
        }
    };
    let mut credentials = match Credentials::load(&directory) {
        Ok(Some(credentials)) => credentials,
        Ok(None) => {
            publish_error(
                model,
                Status::NoCredentials,
                format!(
                    "Mote の認証がありません（{}）。`mote connect` で接続してください",
                    directory.display()
                ),
            );
            return;
        }
        Err(error) => {
            publish_error(model, Status::Offline, error.to_string());
            return;
        }
    };
    if credentials.needs_refresh()
        && credentials.refresh_available()
        && let Err(error) = credentials.refresh(&directory).await
    {
        publish_error(model, Status::Offline, error.to_string());
        return;
    }
    let client = match Client::from_directory_with_token(&directory, credentials.token()) {
        Ok(client) => client,
        Err(error) => {
            publish_error(model, Status::Offline, error.to_string());
            return;
        }
    };
    let views = match fetch_views(&client).await {
        Ok(views) => views,
        Err(error) => {
            publish_error(model, Status::Offline, error);
            return;
        }
    };
    let captures = parse_views(&views);
    let pending = pending_now();
    let next = Model {
        captures,
        pending,
        status: Status::Connected,
        last_sync: Some(mote::now_micros().unwrap_or(0)),
        error: None,
        from_cache: false,
    };
    model.set(Arc::new(next));
    let _ = save_snapshot(&views);
}

fn pending_now() -> Vec<PendingCapture> {
    Spool::open()
        .and_then(|spool| spool.list())
        .unwrap_or_default()
}

fn publish_error(model: &SharedSignal<Arc<Model>>, status: Status, error: String) {
    eprintln!("mote-hud: session: {error}");
    let mut next = (*model.get()).clone();
    next.status = status;
    next.error = Some(error);
    next.pending = pending_now();
    model.set(Arc::new(next));
}

fn node_dir(node: &Option<String>) -> Result<PathBuf, String> {
    let spec = node
        .clone()
        .or_else(|| std::env::var("MOTE_NODE").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "main".into());
    let path = Path::new(&spec);
    if path.is_absolute() || spec.contains('/') {
        return harmion_local::resolve_node(&spec).map_err(|error| error.to_string());
    }
    let named = harmion_local::node_root()
        .map_err(|error| error.to_string())?
        .join(&spec);
    if named.is_dir() {
        Ok(named)
    } else {
        Err(format!("node `{spec}` not found"))
    }
}

async fn fetch_views(client: &Client) -> Result<Vec<Value>, String> {
    let condition = mote_core::query_condition(&Filter {
        archive: ArchiveFilter::All,
        pinned_only: false,
    });
    let mut restarted = false;
    'pages: loop {
        let mut page = client
            .query(condition.clone(), PAGE)
            .await
            .map_err(|error| format!("{error:?}"))?;
        let mut views = Vec::new();
        loop {
            views.extend(page.items);
            if views.len() > MAX_CAPTURES {
                return Err(format!("{MAX_CAPTURES} 件を超えています"));
            }
            match page.next {
                Some(cursor) => match client.query_next(cursor, PAGE).await {
                    Ok(next) => page = next,
                    Err(harmion_client::Error::Http(410)) if !restarted => {
                        restarted = true;
                        continue 'pages;
                    }
                    Err(error) => return Err(format!("{error:?}")),
                },
                None => return Ok(views),
            }
        }
    }
}

fn parse_views(views: &[Value]) -> Vec<Capture> {
    let mut captures: Vec<Capture> = views
        .iter()
        .filter_map(|view| Capture::from_view(view).ok())
        .collect();
    captures.sort_by(|left, right| {
        right
            .created_at()
            .cmp(&left.created_at())
            .then_with(|| right.id.cmp(&left.id))
    });
    captures
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u8,
    saved_at_micros: i64,
    views: Vec<Value>,
}

fn snapshot_path() -> Result<PathBuf, String> {
    let mut directory = mote::state_dir().map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
    }
    directory.push(SNAPSHOT_FILE);
    Ok(directory)
}

fn load_snapshot() -> Option<(Vec<Value>, i64)> {
    let path = snapshot_path().ok()?;
    let bytes = std::fs::read(&path).ok()?;
    let snapshot = serde_json::from_slice::<Snapshot>(&bytes).ok()?;
    (snapshot.version == SNAPSHOT_VERSION).then_some((snapshot.views, snapshot.saved_at_micros))
}

fn save_snapshot(views: &[Value]) -> Result<(), ()> {
    let path = snapshot_path().map_err(|_| ())?;
    // Edit tokens are capabilities. The snapshot is read-only until the next
    // successful query, so they never go to disk.
    let views = views
        .iter()
        .map(|view| {
            let mut view = view.clone();
            if let Some(object) = view.as_object_mut() {
                object.remove("edit_token");
            }
            view
        })
        .collect();
    let snapshot = Snapshot {
        version: SNAPSHOT_VERSION,
        saved_at_micros: mote::now_micros().unwrap_or(0),
        views,
    };
    let bytes = serde_json::to_vec(&snapshot).map_err(|_| ())?;
    let temporary = path.with_extension("tmp");
    {
        use std::io::Write;
        #[cfg(unix)]
        let mut options = {
            use std::os::unix::fs::OpenOptionsExt;
            let mut options = std::fs::OpenOptions::new();
            options.mode(0o600);
            options
        };
        #[cfg(not(unix))]
        let mut options = std::fs::OpenOptions::new();
        let mut file = options
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|_| ())?;
        file.write_all(&bytes).map_err(|_| ())?;
        file.sync_all().map_err(|_| ())?;
    }
    std::fs::rename(&temporary, &path).map_err(|_| ())
}
