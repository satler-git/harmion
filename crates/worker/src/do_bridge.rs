//! wasm-bindgen boundary for the Cloudflare Durable Object host.
//!
//! Compiled only for `wasm32`. The host provides synchronous `transactionSync`
//! and `sql.exec` primitives; this module adapts them to the `SqlHost`/`KvHost`
//! contracts used by the peer-only Worker store and the durable challenge store.
//! It performs no network I/O, TLS, routing or authentication policy.
use crate::{
    admin,
    challenge::WorkerChallenges,
    entry::{self, HostContext, Request},
    kv::{HostError, KvHost, KvTransaction, Row},
    sql::{SqlHost, SqlTransaction, SqlValue},
    store::WorkerStore,
};
use harmion_domain::{ClusterId, NodeId};
use harmion_node::{
    Error as NodeError,
    https_peer::Config,
    membership_state::MembershipSigner,
    provisioning::{self, Manifest},
    storage::{Identity, Store},
};
use harmion_protocol::{LogEpoch, membership::Certificate, unhex, unhex_bytes};
use js_sys::{Array, Object, Reflect, Uint8Array};
use serde::Deserialize;
use wasm_bindgen::{JsCast, JsValue, prelude::*};

#[wasm_bindgen(module = "host-api")]
extern "C" {
    /// Runs `callback` inside the host's synchronous durable transaction.
    #[wasm_bindgen(catch)]
    fn transaction(callback: &JsValue) -> Result<JsValue, JsValue>;
    /// Executes one SQL statement with positional parameters inside the current
    /// transaction and returns an array of row arrays.
    #[wasm_bindgen(js_name = sqlExec, catch)]
    fn sql_exec(sql: &str, params: JsValue) -> Result<JsValue, JsValue>;
    /// 32 cryptographically random bytes from the runtime.
    #[wasm_bindgen(js_name = randomBytes, catch)]
    fn random_bytes() -> Result<Uint8Array, JsValue>;
}

pub(crate) fn random_bytes32() -> Result<[u8; 32], JsValue> {
    let bytes = random_bytes()?;
    if bytes.length() < 32 {
        return Err(JsValue::from_str("short host randomness"));
    }
    let mut out = [0u8; 32];
    bytes.slice(0, 32).copy_to(&mut out);
    Ok(out)
}

fn describe(error: JsValue) -> String {
    error
        .as_string()
        .or_else(|| {
            Reflect::get(&error, &JsValue::from_str("message"))
                .ok()
                .and_then(|value| value.as_string())
        })
        .unwrap_or_else(|| "host error".to_owned())
}

/// Runs one host transaction. Returning `Err` makes the callback throw, which
/// rolls back; the original Rust error is retained and returned afterwards.
fn host_transaction<T>(mut run: impl FnMut() -> Result<T, HostError>) -> Result<T, HostError> {
    let mut outcome: Option<Result<T, HostError>> = None;
    {
        let mut callback = || -> Result<JsValue, JsValue> {
            let result = run();
            let rollback = result.is_err();
            outcome = Some(result);
            if rollback {
                Err(JsValue::from_str("harmion rollback"))
            } else {
                Ok(JsValue::UNDEFINED)
            }
        };
        let callback = ScopedClosure::borrow_mut_assert_unwind_safe(&mut callback);
        let committed = transaction(callback.as_ref()).is_ok();
        drop(callback);
        match outcome {
            Some(Ok(value)) if committed => Ok(value),
            Some(Err(error)) => Err(error),
            Some(Ok(_)) => Err(HostError("transaction did not commit".into())),
            None => Err(HostError("host did not invoke the transaction".into())),
        }
    }
}

fn to_js(value: &SqlValue) -> JsValue {
    match value {
        SqlValue::Null => JsValue::NULL,
        SqlValue::Int(value) => JsValue::from_f64(*value as f64),
        SqlValue::Text(text) => JsValue::from_str(text),
        SqlValue::Blob(bytes) => Uint8Array::from(bytes.as_slice()).into(),
    }
}

fn params_js(params: &[SqlValue]) -> JsValue {
    let array = Array::new_with_length(params.len() as u32);
    for (index, value) in params.iter().enumerate() {
        array.set(index as u32, to_js(value));
    }
    array.into()
}

fn from_js(value: &JsValue) -> Result<SqlValue, HostError> {
    if value.is_null() || value.is_undefined() {
        return Ok(SqlValue::Null);
    }
    if let Some(number) = value.as_f64() {
        return Ok(SqlValue::Int(number as i64));
    }
    if let Some(text) = value.as_string() {
        return Ok(SqlValue::Text(text));
    }
    if let Some(bytes) = value.dyn_ref::<Uint8Array>() {
        return Ok(SqlValue::Blob(bytes.to_vec()));
    }
    Err(HostError("unsupported SQL value type".into()))
}

struct DoSqlTransaction;

impl SqlTransaction for DoSqlTransaction {
    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<usize, HostError> {
        sql_exec(sql, params_js(params)).map_err(|error| HostError(describe(error)))?;
        Ok(0)
    }
    fn query(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, HostError> {
        let rows = sql_exec(sql, params_js(params)).map_err(|error| HostError(describe(error)))?;
        let rows: Array = rows
            .dyn_into()
            .map_err(|_| HostError("SQL rows are not an array".into()))?;
        let mut result = Vec::with_capacity(rows.length() as usize);
        for index in 0..rows.length() {
            let row = rows.get(index);
            let row: Array = row
                .dyn_into()
                .map_err(|_| HostError("SQL row is not an array".into()))?;
            let mut values = Vec::with_capacity(row.length() as usize);
            for column in 0..row.length() {
                values.push(from_js(&row.get(column))?);
            }
            result.push(values);
        }
        Ok(result)
    }
}

/// The peer-only node store's SQL host over `transactionSync`.
pub(crate) struct DoSqlHost;

impl SqlHost for DoSqlHost {
    fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn SqlTransaction) -> Result<T, HostError>,
    ) -> Result<T, HostError> {
        let mut f = Some(f);
        host_transaction(move || {
            let f = f.take().ok_or_else(|| HostError("repeated call".into()))?;
            f(&mut DoSqlTransaction)
        })
    }
    fn sync(&mut self) -> Result<(), HostError> {
        // The JS host awaits `storage.sync()` after the export returns and before
        // releasing the response; the Rust side cannot await.
        Ok(())
    }
}

struct DoKvTransaction;

impl KvTransaction for DoKvTransaction {
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, HostError> {
        let mut tx = DoSqlTransaction;
        let rows = tx
            .query(
                "SELECT v FROM kv WHERE k = ?",
                &[SqlValue::Blob(key.to_vec())],
            )
            .map_err(|error| HostError(error.0))?;
        Ok(rows.first().and_then(|row| match row.first() {
            Some(SqlValue::Blob(bytes)) => Some(bytes.clone()),
            Some(SqlValue::Text(text)) => Some(text.as_bytes().to_vec()),
            _ => None,
        }))
    }
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), HostError> {
        let mut tx = DoSqlTransaction;
        tx.execute(
            "INSERT INTO kv (k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            &[SqlValue::Blob(key.to_vec()), SqlValue::Blob(value)],
        )
        .map_err(|error| HostError(error.0))?;
        Ok(())
    }
    fn delete(&mut self, key: &[u8]) -> Result<(), HostError> {
        let mut tx = DoSqlTransaction;
        tx.execute(
            "DELETE FROM kv WHERE k = ?",
            &[SqlValue::Blob(key.to_vec())],
        )
        .map_err(|error| HostError(error.0))?;
        Ok(())
    }
    fn range(&mut self, prefix: &[u8], limit: usize) -> Result<Vec<Row>, HostError> {
        let mut tx = DoSqlTransaction;
        let rows = tx
            .query(
                "SELECT k, v FROM kv WHERE substr(k, 1, ?) = ? ORDER BY k LIMIT ?",
                &[
                    SqlValue::Int(prefix.len() as i64),
                    SqlValue::Blob(prefix.to_vec()),
                    SqlValue::Int(limit as i64),
                ],
            )
            .map_err(|error| HostError(error.0))?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let [SqlValue::Blob(key), SqlValue::Blob(value), ..] = row.as_slice() else {
                return Err(HostError("unexpected KV row".into()));
            };
            result.push((key.clone(), value.clone()));
        }
        Ok(result)
    }
}

/// Challenge KV over the same SQL transaction boundary. This avoids the
/// asynchronous Durable Object KV API, which cannot join a synchronous CAS.
struct DoKv;

impl KvHost for DoKv {
    fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn KvTransaction) -> Result<T, HostError>,
    ) -> Result<T, HostError> {
        let mut f = Some(f);
        host_transaction(move || {
            let f = f.take().ok_or_else(|| HostError("repeated call".into()))?;
            f(&mut DoKvTransaction)
        })
    }
    fn sync(&mut self) -> Result<(), HostError> {
        Ok(())
    }
}

fn identity(cluster: &str, node: &str, epoch: &str) -> Result<Identity, JsValue> {
    Ok(Identity {
        cluster: ClusterId(
            unhex::<32>(cluster).map_err(|error| JsValue::from_str(&error.to_string()))?,
        ),
        node: NodeId(unhex::<32>(node).map_err(|error| JsValue::from_str(&error.to_string()))?),
        epoch: LogEpoch(unhex::<32>(epoch).map_err(|error| JsValue::from_str(&error.to_string()))?),
    })
}

fn signer(seed: &str) -> Result<MembershipSigner, JsValue> {
    let seed = unhex::<32>(seed).map_err(|error| JsValue::from_str(&error.to_string()))?;
    Ok(MembershipSigner::from_bytes(&seed))
}

fn response(status: u16, body: Vec<u8>) -> Result<JsValue, JsValue> {
    let object = Object::new();
    Reflect::set(
        &object,
        &JsValue::from_str("status"),
        &JsValue::from_f64(status as f64),
    )?;
    Reflect::set(
        &object,
        &JsValue::from_str("body"),
        &Uint8Array::from(body.as_slice()).into(),
    )?;
    Ok(object.into())
}

pub(crate) fn open(
    cluster: &str,
    node: &str,
    epoch: &str,
) -> Result<(WorkerStore<DoSqlHost>, Identity), JsValue> {
    let identity = identity(cluster, node, epoch)?;
    let store = WorkerStore::open(DoSqlHost, identity)
        .map_err(|error| JsValue::from_str(&format!("open node store: {error}")))?;
    // Challenge rows live in SQL so the CAS transaction is synchronous.
    sql_exec(
        "CREATE TABLE IF NOT EXISTS kv (k BLOB PRIMARY KEY, v BLOB NOT NULL)",
        params_js(&[]),
    )
    .map_err(|error| JsValue::from_str(&format!("prepare challenge table: {}", describe(error))))?;
    Ok((store, identity))
}

#[wasm_bindgen]
#[allow(clippy::too_many_arguments)] // flat wasm boundary; each argument is a host-supplied scalar
pub fn peer_handle(
    cluster: &str,
    node: &str,
    epoch: &str,
    seed: &str,
    origin: &str,
    path: &str,
    host: &str,
    has_origin: bool,
    now_micros: f64,
    body: &[u8],
) -> Result<JsValue, JsValue> {
    let (mut store, identity) = open(cluster, node, epoch)?;
    let signer = signer(seed)?;
    let config = Config {
        origin: origin.to_owned(),
    };
    let context = HostContext {
        signer: &signer,
        config: &config,
        local: identity,
        now: now_micros as i64,
    };
    let mut challenges = WorkerChallenges::new(DoKv);
    let result = entry::handle(
        &mut store,
        &mut challenges,
        &context,
        Request {
            method: "POST",
            path,
            host: Some(host),
            has_origin,
            body,
        },
        &mut || {
            let bytes = random_bytes().map_err(|_| NodeError::Entropy("host randomness".into()))?;
            if bytes.length() < 32 {
                return Err(NodeError::Entropy("short host randomness".into()));
            }
            let mut out = [0u8; 32];
            bytes.slice(0, 32).copy_to(&mut out);
            Ok(out)
        },
    );
    peer_response(result.status, result.body, result.data)
}

fn peer_response(status: u16, body: Vec<u8>, data: bool) -> Result<JsValue, JsValue> {
    let object = response(status, body)?;
    Reflect::set(
        &object,
        &JsValue::from_str("data"),
        &JsValue::from_bool(data),
    )?;
    Ok(object)
}

#[wasm_bindgen]
#[allow(clippy::too_many_arguments)] // flat wasm boundary; tokens are passed explicitly
pub fn admin_handle(
    cluster: &str,
    node: &str,
    epoch: &str,
    seed: &str,
    origin: &str,
    now_micros: f64,
    operation: &str,
    expected_token: &str,
    provided_token: &str,
) -> Result<JsValue, JsValue> {
    let (mut store, _) = open(cluster, node, epoch)?;
    if !crate::bearer_matches(
        if provided_token.is_empty() {
            None
        } else {
            Some(provided_token)
        },
        expected_token,
    ) {
        let unauthorized = admin::Response::unauthorized();
        return response(unauthorized.status, unauthorized.body);
    }
    if operation == "registration-code" {
        let signer = signer(seed)?;
        let value = store
            .transaction(|tx| admin::registration_code(tx, origin, &signer.public_key()))
            .map_err(|error| JsValue::from_str(&format!("registration: {error}")))?;
        let body = serde_json::to_vec(&value)
            .map_err(|error| JsValue::from_str(&format!("registration: {error}")))?;
        return response(200, body);
    }
    let result = admin::handle(operation, &mut store, origin, now_micros as i64);
    response(result.status, result.body)
}

#[derive(Deserialize)]
struct MemberJson {
    node: String,
    signing_key: String,
    binding: BindingJson,
    endpoint: String,
}

#[derive(Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
enum BindingJson {
    Https { certificate: String },
    Iroh { endpoint_id: String },
    SignedHttps { key: String },
}

#[derive(Deserialize)]
struct ManifestJson {
    own: MemberJson,
    root: String,
    evidence: Vec<String>,
    contacts: Vec<MemberJson>,
}

fn member(json: &MemberJson) -> Result<harmion_protocol::membership::Member, JsValue> {
    let binding = match &json.binding {
        BindingJson::Https { certificate } => harmion_protocol::membership::Binding::Https(
            unhex_bytes(certificate, 4096)
                .map_err(|error| JsValue::from_str(&error.to_string()))?,
        ),
        BindingJson::Iroh { endpoint_id } => harmion_protocol::membership::Binding::Iroh(
            unhex::<32>(endpoint_id).map_err(|error| JsValue::from_str(&error.to_string()))?,
        ),
        BindingJson::SignedHttps { key } => harmion_protocol::membership::Binding::SignedHttps(
            unhex::<32>(key).map_err(|error| JsValue::from_str(&error.to_string()))?,
        ),
    };
    Ok(harmion_protocol::membership::Member {
        node: NodeId(
            unhex::<32>(&json.node).map_err(|error| JsValue::from_str(&error.to_string()))?,
        ),
        signing_key: unhex::<32>(&json.signing_key)
            .map_err(|error| JsValue::from_str(&error.to_string()))?,
        binding,
        endpoint: json.endpoint.clone(),
    })
}

fn certificate(hex: &str) -> Result<Certificate, JsValue> {
    let bytes = unhex_bytes(hex, 4096).map_err(|error| JsValue::from_str(&error.to_string()))?;
    Certificate::from_bytes(&bytes).map_err(|error| JsValue::from_str(&error.to_string()))
}

/// Host-only, deployment-time provisioning. The host must not route this to the
/// public network, and must call it at most once with a bootstrap credential.
#[wasm_bindgen]
pub fn provision(
    cluster: &str,
    node: &str,
    epoch: &str,
    seed: &str,
    manifest_json: &str,
) -> Result<JsValue, JsValue> {
    let (mut store, _) = open(cluster, node, epoch)?;
    let signer = signer(seed)?;
    let manifest: ManifestJson = serde_json::from_str(manifest_json)
        .map_err(|error| JsValue::from_str(&format!("manifest: {error}")))?;
    let own = member(&manifest.own)?;
    let root = certificate(&manifest.root)?;
    let evidence = manifest
        .evidence
        .iter()
        .map(|hex| certificate(hex))
        .collect::<Result<Vec<_>, _>>()?;
    let contacts = manifest
        .contacts
        .iter()
        .map(member)
        .collect::<Result<Vec<_>, _>>()?;
    provisioning::provision(
        &mut store,
        &signer,
        &Manifest {
            own,
            root,
            evidence: &evidence,
            contacts: &contacts,
        },
    )
    .map_err(|error| JsValue::from_str(&format!("provision: {error}")))?;
    Ok(JsValue::TRUE)
}
