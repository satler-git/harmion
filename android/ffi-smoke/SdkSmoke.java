import java.io.File;
import java.nio.file.Files;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * SDK JNI smoke test. Runs in a second JVM against a node started by NodeHost,
 * which mirrors the Android process model (one app = one JVM).
 */
public final class SdkSmoke {
    private static final Pattern ENDPOINT = Pattern.compile("\"endpoint\"\\s*:\\s*\"([^\"]+)\"");
    private static final Pattern CERTIFICATE =
            Pattern.compile("\"certificate\"\\s*:\\s*\"([^\"]+)\"");
    private static final Pattern CLUSTER = Pattern.compile("\"cluster_id\"\\s*:\\s*\"([^\"]+)\"");
    private static final Pattern NODE = Pattern.compile("\"node_id\"\\s*:\\s*\"([^\"]+)\"");
    private static final Pattern HANDLE = Pattern.compile("\"handle\"\\s*:\\s*(\\d+)");
    private static final Pattern SUBSCRIPTION =
            Pattern.compile("\"subscription\"\\s*:\\s*(\\d+)");

    private static final String CONTENT_ID =
            "1111111111111111111111111111111111111111111111111111111111111111";
    private static final String CONTENT_ID_TWO =
            "2222222222222222222222222222222222222222222222222222222222222222";
    private static final String REQUEST_ONE =
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    private static final String REQUEST_TWO =
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    private static void require(boolean condition, String message) {
        if (!condition) {
            throw new AssertionError(message);
        }
    }

    private static String field(Pattern pattern, String json, String name) {
        Matcher matcher = pattern.matcher(json);
        require(matcher.find(), "missing " + name + " in " + json);
        return matcher.group(1);
    }

    private static long handle(String reply) {
        Matcher matcher = HANDLE.matcher(reply);
        require(matcher.find(), "no handle in " + reply);
        return Long.parseLong(matcher.group(1));
    }

    private static String hex(byte[] bytes) {
        StringBuilder builder = new StringBuilder(bytes.length * 2);
        for (byte value : bytes) {
            builder.append(String.format("%02x", value));
        }
        return builder.toString();
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            System.err.println("usage: SdkSmoke <node-directory>");
            System.exit(2);
        }
        File root = new File(args[0]);
        String connection = Files.readString(new File(root, "connection.json").toPath());
        String info =
                "{\"version\":1,\"cluster_id\":\""
                        + field(CLUSTER, connection, "cluster_id")
                        + "\",\"node_id\":\""
                        + field(NODE, connection, "node_id")
                        + "\",\"certificate\":\""
                        + field(CERTIFICATE, connection, "certificate")
                        + "\",\"endpoint\":\""
                        + field(ENDPOINT, connection, "endpoint")
                        + "\"}";
        String credential = hex(Files.readAllBytes(new File(root, "admin.secret").toPath()));

        org.harmion.sdk.core.HarmionNative sdk = new org.harmion.sdk.core.HarmionNative();
        String connected = sdk.connect(info, credential);
        require(connected.contains("\"ok\":true"), connected);
        long handle = handle(connected);
        try {
            String save =
                    sdk.save(
                            handle,
                            "{\"request_id\":\""
                                    + REQUEST_ONE
                                    + "\",\"content_id\":\""
                                    + CONTENT_ID
                                    + "\",\"command\":{\"type\":\"create\",\"fields\":"
                                    + "[{\"field\":\"body\",\"text\":\"hello from the SDK\"},"
                                    + "{\"field\":\"attribute\",\"namespace\":\"app\","
                                    + "\"key\":\"done\",\"value\":false}]}}");
            require(save.contains("\"ok\":true"), save);
            require(save.contains("update_id"), save);

            String view = sdk.get(handle, CONTENT_ID);
            require(view.contains("\"ok\":true"), view);
            require(view.contains("\"hello from the SDK\""), view);

            String page = sdk.query(handle, "{\"type\":\"and\",\"conditions\":[]}", 32);
            require(page.contains("\"ok\":true"), page);
            require(page.contains(CONTENT_ID), page);

            String subscribed = sdk.subscribe(handle, "{\"type\":\"and\",\"conditions\":[]}");
            require(subscribed.contains("\"ok\":true"), subscribed);
            Matcher subscriptionMatcher = SUBSCRIPTION.matcher(subscribed);
            require(subscriptionMatcher.find(), subscribed);
            long subscription = Long.parseLong(subscriptionMatcher.group(1));

            boolean ended = false;
            for (int attempt = 0; attempt < 8 && !ended; attempt++) {
                String event = sdk.subscriptionNext(handle, subscription);
                require(event.contains("\"ok\":true"), event);
                ended = event.contains("\"snapshot_end\"");
            }
            require(ended, "subscription snapshot did not end");

            String savedTwo =
                    sdk.save(
                            handle,
                            "{\"request_id\":\""
                                    + REQUEST_TWO
                                    + "\",\"content_id\":\""
                                    + CONTENT_ID_TWO
                                    + "\",\"command\":{\"type\":\"create\",\"fields\":"
                                    + "[{\"field\":\"body\",\"text\":\"live update\"}]}}");
            require(savedTwo.contains("\"ok\":true"), savedTwo);

            boolean changed = false;
            for (int attempt = 0; attempt < 6 && !changed; attempt++) {
                String event = sdk.subscriptionNext(handle, subscription);
                require(event.contains("\"ok\":true"), event);
                changed = event.contains("\"changes\"") && event.contains(CONTENT_ID_TWO);
            }
            require(changed, "subscription did not report the change");
            String unsubscribed = sdk.unsubscribe(handle, subscription);
            require(unsubscribed.contains("\"ok\":true"), unsubscribed);
        } finally {
            sdk.close(handle);
        }
        System.out.println("android SDK JNI smoke test: OK");
    }
}
