import java.io.File;
import java.nio.file.Files;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.harmion.android.core.HarmionNative;

/**
 * Host-side JNI smoke test for {@code libharmion_android.so}.
 *
 * <p>It exercises the same symbols Android loads, so a JNI signature or symbol
 * mismatch fails here instead of on a device. No JSON library is used: replies
 * are inspected as strings on purpose.
 */
public final class Smoke {
    private static final Pattern HANDLE = Pattern.compile("\"handle\"\\s*:\\s*(\\d+)");

    private static void require(boolean condition, String message) {
        if (!condition) {
            throw new AssertionError(message);
        }
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            System.err.println("usage: Smoke <node-directory>");
            System.exit(2);
        }
        File root = new File(args[0]);
        root.getParentFile().mkdirs();
        String path = root.getAbsolutePath();
        String options =
                "{\"use_iroh\":false,\"public_lookup\":false,\"sync_seconds\":0,"
                        + "\"oauth_listen\":\"off\",\"worker_threads\":1}";

        HarmionNative harmion = new HarmionNative();
        String identifiers = harmion.identifiers(path);
        require(identifiers.contains("\"initialized\":false"), identifiers);

        String initialized = harmion.initialize(path, options);
        require(initialized.contains("\"ok\":true"), initialized);
        require(initialized.contains("\"initialized\":true"), initialized);

        String started = harmion.start(path, options);
        require(started.contains("\"ok\":true"), started);
        Matcher matcher = HANDLE.matcher(started);
        require(matcher.find(), started);
        long handle = Long.parseLong(matcher.group(1));

        try {
            String status = harmion.status(handle);
            require(status.contains("\"ok\":true"), status);
            require(status.contains("\"running\":true"), status);
            require(status.contains("\"membership\":\"active\""), status);

            String policy = harmion.setSyncPolicy(handle, false, "{\"code\":\"wifi_only\"}");
            require(policy.contains("\"permitted\":false"), policy);
            String requested = harmion.requestSync(handle, "network_return");
            require(requested.contains("\"requested\":true"), requested);

            String sync = harmion.syncNow(handle);
            require(sync.contains("\"ok\":true"), sync);

            String connection = harmion.connection(path);
            require(connection.contains("\"endpoint\""), connection);

            require(harmion.status(0L).contains("\"unknown_handle\""), "expected unknown handle");
            require(Files.exists(root.toPath().resolve("node.db")), "node.db must exist");
        } finally {
            String stopped = harmion.stop(handle);
            require(stopped.contains("\"stopped\":true"), stopped);
        }
        require(harmion.status(handle).contains("\"unknown_handle\""), "handle must be gone");

        System.out.println("android JNI smoke test: OK");
    }
}
