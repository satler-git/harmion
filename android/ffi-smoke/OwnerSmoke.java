import java.io.File;
import java.nio.file.Files;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * End-to-end owner approval on the host: the node cdylib enrolls an Ed25519
 * owner key, the SDK cdylib requests an owner challenge, the node signs it, and
 * the SDK submits the assertion. The node verifies the SSHSIG in-process (no
 * ssh-keygen), which is the Android path.
 */
public final class OwnerSmoke {
    private static final String SEED =
            "0909090909090909090909090909090909090909090909090909090909090909";
    private static final String CHALLENGE =
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    private static void require(boolean condition, String message) {
        if (!condition) {
            throw new AssertionError(message);
        }
    }

    private static String unescape(String value) {
        StringBuilder builder = new StringBuilder();
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            if (c != '\\') {
                builder.append(c);
                continue;
            }
            char next = value.charAt(++i);
            switch (next) {
                case 'n' -> builder.append('\n');
                case 'r' -> builder.append('\r');
                case 't' -> builder.append('\t');
                case '"' -> builder.append('"');
                case '\\' -> builder.append('\\');
                case '/' -> builder.append('/');
                case 'u' -> {
                    builder.append((char) Integer.parseInt(value.substring(i + 1, i + 5), 16));
                    i += 4;
                }
                default -> builder.append(next);
            }
        }
        return builder.toString();
    }

    private static long number(String json, String name) {
        Matcher matcher =
                Pattern.compile("\"" + name + "\"\\s*:\\s*(\\d+)").matcher(json);
        require(matcher.find(), "missing " + name + " in " + json);
        return Long.parseLong(matcher.group(1));
    }

    private static String field(String json, String name) {
        Matcher matcher =
                Pattern.compile("\"" + name + "\"\\s*:\\s*\"((?:[^\"\\\\]|\\\\.)*)\"").matcher(json);
        require(matcher.find(), "missing " + name + " in " + json);
        return unescape(matcher.group(1));
    }

    private static String hex(byte[] bytes) {
        StringBuilder builder = new StringBuilder(bytes.length * 2);
        for (byte value : bytes) {
            builder.append(String.format("%02x", value));
        }
        return builder.toString();
    }

    private static String hex(String text) {
        return hex(text.getBytes(java.nio.charset.StandardCharsets.UTF_8));
    }

    public static void main(String[] args) throws Exception {
        File root = new File(args[0]);
        root.getParentFile().mkdirs();
        String path = root.getAbsolutePath();
        String options =
                "{\"use_iroh\":false,\"public_lookup\":false,\"sync_seconds\":0,"
                        + "\"oauth_listen\":\"off\",\"worker_threads\":1}";

        org.harmion.android.core.HarmionNative node =
                new org.harmion.android.core.HarmionNative();
        String initialized = node.initialize(path, options);
        require(initialized.contains("\"ok\":true"), initialized);
        String started = node.start(path, options);
        require(started.contains("\"ok\":true"), started);
        long nodeHandle =
                number(started, "handle");
        org.harmion.sdk.core.HarmionNative sdk = new org.harmion.sdk.core.HarmionNative();
        try {
            String key = node.ownerPublicKey(SEED);
            String publicKey = field(key, "public_key");
            require(publicKey.startsWith("ssh-ed25519 "), publicKey);
            String enrolled = node.ownerEnroll(nodeHandle, publicKey, "primary");
            require(enrolled.contains("\"enrolled\":true"), enrolled);
            require(field(enrolled, "recovery_code").length() == 64, enrolled);

            String connection = Files.readString(new File(root, "connection.json").toPath());
            String adminSecret = hex(Files.readAllBytes(new File(root, "admin.secret").toPath()));
            String info =
                    "{\"version\":1,\"cluster_id\":\""
                            + field(connection, "cluster_id")
                            + "\",\"node_id\":\""
                            + field(connection, "node_id")
                            + "\",\"certificate\":\""
                            + field(connection, "certificate")
                            + "\",\"endpoint\":\""
                            + field(connection, "endpoint")
                            + "\"}";

            String anonymous = sdk.anonymous(info);
            require(anonymous.contains("\"ok\":true"), anonymous);
            long anon = number(anonymous, "handle");
            String admin = sdk.connect(info, adminSecret);
            require(admin.contains("\"ok\":true"), admin);
            long adminHandle = number(admin, "handle");
            try {
                String offer =
                        "{\"version\":1,\"name\":\"Owner smoke\","
                                + "\"redirect_uri\":\"http://127.0.0.1/harmion-sdk/callback\","
                                + "\"challenge\":\""
                                + CHALLENGE
                                + "\",\"state\":\"smoke\","
                                + "\"grants\":{\"read\":{\"type\":\"all\"},\"write\":{\"type\":\"all\"}}}";
                String startedRequest = sdk.connectStart(anon, offer);
                require(startedRequest.contains("\"ok\":true"), startedRequest);
                String connectionId = field(startedRequest, "connection_id");
                String requestSecret = field(startedRequest, "request_secret");

                String approval =
                        "{\"connection_id\":\""
                                + connectionId
                                + "\",\"allow\":true,\"replace\":false}";
                String approvalHex = hex(approval);
                String challenge = sdk.ownerChallenge(adminHandle, "connect-approve", "", approvalHex, "");
                require(challenge.contains("\"ok\":true"), challenge);
                String challengeId = field(challenge, "challenge");
                String statement = field(challenge, "statement");

                String signed = node.ownerSign(SEED, statement);
                require(signed.contains("\"ok\":true"), signed);
                String signature = field(signed, "signature");
                require(signature.startsWith("-----BEGIN SSH SIGNATURE-----"), signature);

                String approved =
                        sdk.callWithOwner(
                                adminHandle, "connect-approve", approvalHex, challengeId, signature);
                require(approved.contains("\"ok\":true"), approved);
                require(approved.contains("\"approved\"") || approved.contains("\"status\":\"approved\""), approved);

                String status =
                        sdk.connectStatus(
                                anon,
                                "{\"connection_id\":\""
                                        + connectionId
                                        + "\",\"request_secret\":\""
                                        + requestSecret
                                        + "\"}");
                require(status.contains("\"approved\""), status);
                // A replayed challenge must be rejected.
                String replay =
                        sdk.callWithOwner(
                                adminHandle, "connect-approve", approvalHex, challengeId, signature);
                require(!replay.contains("\"ok\":true"), replay);
            } finally {
                sdk.close(anon);
                sdk.close(adminHandle);
            }
        } finally {
            String stopped = node.stop(nodeHandle);
            require(stopped.contains("\"stopped\":true"), stopped);
        }
        System.out.println("owner approval smoke test: OK");
    }
}
