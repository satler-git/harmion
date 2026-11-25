package org.harmion.sdk.core;

/**
 * JNI declarations mirrored by the Kotlin {@code HarmionNative} object in the
 * SDK module. Used by the host JVM smoke tests.
 */
public class HarmionNative {
    static {
        System.loadLibrary("harmion_sdk_native");
    }

    public native String connect(String connection, String credential);

    public native String anonymous(String connection);

    public native String close(long handle);

    public native String callRaw(long handle, String operation, String bodyHex);

    public native String save(long handle, String request);

    public native String get(long handle, String content);

    public native String query(long handle, String condition, int limit);

    public native String subscribe(long handle, String condition);

    public native String subscriptionNext(long handle, long subscription);

    public native String unsubscribe(long handle, long subscription);

    public native String connectStart(long handle, String offer);

    public native String connectStatus(long handle, String request);

    public native String connectComplete(long handle, String request);

    public native String connectList(long handle);

    public native String connectApprove(long handle, String approval);

    public native String syncStatus(long handle, String update);

    public native String oauthRefresh(
            String origin, String root, String clientId, String refreshToken);

    public native String randomHex(int count);

    public native String ownerChallenge(
            long handle, String operation, String target, String body, String session);

    public native String callWithOwner(
            long handle, String operation, String body, String challenge, String signature);
}
