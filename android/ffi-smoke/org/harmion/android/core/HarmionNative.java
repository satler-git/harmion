package org.harmion.android.core;

/**
 * JNI declarations mirrored by the Kotlin {@code HarmionNative} object in the
 * app module. Used by the host JVM smoke tests.
 */
public class HarmionNative {
    static {
        System.loadLibrary("harmion_android");
    }

    public native String initialize(String root, String options);

    public native String start(String root, String options);

    public native String stop(long handle);

    public native String status(long handle);

    public native String syncNow(long handle);

    public native String syncGated(long handle);

    public native String setSyncPolicy(long handle, boolean permitted, String reason);

    public native String requestSync(long handle, String trigger);

    public native String identifiers(String root);

    public native String connection(String root);

    public native String config(String root);

    public native String oauthRoot(String root);

    public native String ownerPublicKey(String seedHex);

    public native String ownerSign(String seedHex, String statementHex);

    public native String ownerEnroll(long handle, String publicKey, String label);
}
