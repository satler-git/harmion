//! JNI adapters for [`crate::api`].
//!
//! Kotlin declares these as `external fun` on `org.harmion.android.core.HarmionNative`.
//! Every function takes and returns a `String` (JSON), so the ABI stays stable
//! across Kotlin renames and no secret is marshalled through JNI types.
//!
//! Strings are read through the `Env` obtained from `EnvUnowned::with_env`, never
//! through a JavaVM singleton: the first native call of a process (for example
//! onboarding's `initialize`) must not depend on another call having initialized
//! any global state first.
use crate::api;
use jni::{
    Env, EnvUnowned,
    errors::{Error as JniError, ThrowRuntimeExAndDefault},
    objects::{JObject, JString},
    sys::{jboolean, jlong},
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
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_initialize<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    root: JString<'local>,
    options: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let root = read(env, &root);
        let options = read(env, &options);
        Ok(api::initialize(&root, &options))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_start<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    root: JString<'local>,
    options: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let root = read(env, &root);
        let options = read(env, &options);
        Ok(api::start(&root, &options))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_stop<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
) -> JString<'local> {
    call(&mut env, |_| Ok(api::stop(handle as u64)))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_status<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
) -> JString<'local> {
    call(&mut env, |_| Ok(api::status(handle as u64)))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_syncNow<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
) -> JString<'local> {
    call(&mut env, |_| Ok(api::sync_now(handle as u64)))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_syncGated<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
) -> JString<'local> {
    call(&mut env, |_| Ok(api::sync_gated(handle as u64)))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_setSyncPolicy<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    permitted: jboolean,
    reason: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let reason = read(env, &reason);
        Ok(api::set_sync_policy(handle as u64, permitted, &reason))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_requestSync<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    trigger: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let trigger = read(env, &trigger);
        Ok(api::request_sync(handle as u64, &trigger))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_identifiers<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    root: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let root = read(env, &root);
        Ok(api::identifiers(&root))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_connection<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    root: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let root = read(env, &root);
        Ok(api::connection(&root))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_config<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    root: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let root = read(env, &root);
        Ok(api::config(&root))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_oauthRoot<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    root: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let root = read(env, &root);
        Ok(api::oauth_root(&root))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_ownerPublicKey<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    seed: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let seed = read(env, &seed);
        Ok(api::owner_public_key(&seed))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_ownerSign<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    seed: JString<'local>,
    statement: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let seed = read(env, &seed);
        let statement = read(env, &statement);
        Ok(api::owner_sign(&seed, &statement))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_harmion_android_core_HarmionNative_ownerEnroll<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    public_key: JString<'local>,
    label: JString<'local>,
) -> JString<'local> {
    call(&mut env, |env| {
        let public_key = read(env, &public_key);
        let label = read(env, &label);
        Ok(api::owner_enroll(handle as u64, &public_key, &label))
    })
}
