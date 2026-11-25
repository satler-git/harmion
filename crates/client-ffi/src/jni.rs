//! JNI adapters for the SDK data plane. Mirrors
//! `org.harmion.sdk.core.HarmionNative` in Kotlin.
//!
//! Strings are read through the `Env` obtained from `EnvUnowned::with_env` so the
//! first native call of a client process works without prior JNI state.
use crate::api;
use jni::{
    Env, EnvUnowned,
    errors::{Error as JniError, ThrowRuntimeExAndDefault},
    objects::{JObject, JString},
    sys::{jint, jlong},
};

fn read(env: &mut Env, value: &JString) -> String {
    value.mutf8_chars(env).map(String::from).unwrap_or_default()
}

fn call<'local>(
    env: &mut EnvUnowned<'local>,
    operation: impl FnOnce(&mut Env<'local>) -> Result<String, JniError>,
) -> JString<'local> {
    let outcome = env.with_env(|env| -> Result<JString<'local>, JniError> {
        let output = operation(env)?;
        JString::from_str(env, output)
    });
    outcome.resolve::<ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_connect<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    info: JString<'local>,
    credential: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let info = read(env, &info);
        let credential = read(env, &credential);
        Ok(api::connect(&info, &credential))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_anonymous<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    info: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let info = read(env, &info);
        Ok(api::anonymous(&info))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_close<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
) -> JString<'local> {
    call(&mut env, |_| Ok(api::close(handle as u64)))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_callRaw<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    operation: JString<'local>,
    body: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let operation = read(env, &operation);
        let body = read(env, &body);
        Ok(api::call_raw(handle as u64, &operation, &body))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_save<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    request: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let request = read(env, &request);
        Ok(api::save(handle as u64, &request))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_get<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    content: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let content = read(env, &content);
        Ok(api::get(handle as u64, &content))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_query<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    condition: JString<'local>,
    limit: jint,
) -> JString<'local> {
    call(&mut env, |env| {
        let condition = read(env, &condition);
        Ok(api::query(handle as u64, &condition, limit.max(0) as u32))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_subscribe<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    condition: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let condition = read(env, &condition);
        Ok(api::subscribe(handle as u64, &condition))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_subscriptionNext<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    subscription: jlong,
) -> JString<'local> {
    call(&mut env, |_| {
        Ok(api::subscription_next(handle as u64, subscription as u64))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_unsubscribe<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    subscription: jlong,
) -> JString<'local> {
    call(&mut env, |_| {
        Ok(api::unsubscribe(handle as u64, subscription as u64))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_connectStart<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    offer: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let offer = read(env, &offer);
        Ok(api::connect_start(handle as u64, &offer))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_connectStatus<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    request: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let request = read(env, &request);
        Ok(api::connect_status(handle as u64, &request))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_connectComplete<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    request: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let request = read(env, &request);
        Ok(api::connect_complete(handle as u64, &request))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_connectList<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
) -> JString<'local> {
    call(&mut env, |_| Ok(api::connect_list(handle as u64)))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_connectApprove<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    approval: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let approval = read(env, &approval);
        Ok(api::connect_approve(handle as u64, &approval))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_syncStatus<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    update: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let update = read(env, &update);
        Ok(api::sync_status(handle as u64, &update))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_oauthRefresh<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    origin: JString<'local>,
    root: JString<'local>,
    client_id: JString<'local>,
    refresh_token: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let origin = read(env, &origin);
        let root = read(env, &root);
        let client_id = read(env, &client_id);
        let refresh_token = read(env, &refresh_token);
        Ok(api::oauth_refresh(
            &origin,
            &root,
            &client_id,
            &refresh_token,
        ))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_randomHex<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    count: jint,
) -> JString<'local> {
    call(&mut env, |_| Ok(api::random_hex(count.max(0) as u32)))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_ownerChallenge<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    operation: JString<'local>,
    target: JString<'local>,
    body: JString<'local>,
    session: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let operation = read(env, &operation);
        let target = read(env, &target);
        let body = read(env, &body);
        let session = read(env, &session);
        Ok(api::owner_challenge(
            handle as u64,
            &operation,
            &target,
            &body,
            &session,
        ))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_callWithOwner<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    operation: JString<'local>,
    body: JString<'local>,
    challenge: JString<'local>,
    signature: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let operation = read(env, &operation);
        let body = read(env, &body);
        let challenge = read(env, &challenge);
        let signature = read(env, &signature);
        Ok(api::call_with_owner(
            handle as u64,
            &operation,
            &body,
            &challenge,
            &signature,
        ))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_conflicts<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    content: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let content = read(env, &content);
        Ok(api::conflicts(handle as u64, &content))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_mergeInput<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    content: JString<'local>,
    token: JString<'local>,
    left: JString<'local>,
    right: JString<'local>,
    base: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let content = read(env, &content);
        let token = read(env, &token);
        let left = read(env, &left);
        let right = read(env, &right);
        let base = read(env, &base);
        Ok(api::merge_input(
            handle as u64,
            &content,
            &token,
            &left,
            &right,
            &base,
        ))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_sdk_core_HarmionNative_mergePlan<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    content: JString<'local>,
    token: JString<'local>,
    left: JString<'local>,
    right: JString<'local>,
    base: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let content = read(env, &content);
        let token = read(env, &token);
        let left = read(env, &left);
        let right = read(env, &right);
        let base = read(env, &base);
        Ok(api::merge_plan(
            handle as u64,
            &content,
            &token,
            &left,
            &right,
            &base,
        ))
    })
}
