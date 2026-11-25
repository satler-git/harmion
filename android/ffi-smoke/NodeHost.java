/**
 * Host-side node process: runs the node cdylib in its own JVM so the SDK smoke
 * test can talk to it from a second JVM. On Android this separation is the norm:
 * the node app and each client app are different processes.
 */
public final class NodeHost {
    public static void main(String[] args) throws Exception {
        if (args.length != 2) {
            System.err.println("usage: NodeHost <node-directory> <stop-file>");
            System.exit(2);
        }
        java.io.File root = new java.io.File(args[0]);
        root.getParentFile().mkdirs();
        String options =
                "{\"use_iroh\":false,\"public_lookup\":false,\"sync_seconds\":0,"
                        + "\"oauth_listen\":\"off\",\"worker_threads\":1}";
        org.harmion.android.core.HarmionNative node =
                new org.harmion.android.core.HarmionNative();
        System.out.flush();
        String initialized = node.initialize(root.getAbsolutePath(), options);
        if (!initialized.contains("\"ok\":true")) {
            System.err.println("initialize failed: " + initialized);
            System.exit(1);
        }
        String started = node.start(root.getAbsolutePath(), options);
        if (!started.contains("\"ok\":true")) {
            System.err.println("start failed: " + started);
            System.exit(1);
        }
        java.util.regex.Matcher matcher =
                java.util.regex.Pattern.compile("\"handle\"\\s*:\\s*(\\d+)").matcher(started);
        if (!matcher.find()) {
            System.err.println("no handle: " + started);
            System.exit(1);
        }
        long handle = Long.parseLong(matcher.group(1));
        System.out.println("NODE READY");
        System.out.flush();

        java.io.File stop = new java.io.File(args[1]);
        while (!stop.exists()) {
            Thread.sleep(200);
        }
        String stopped = node.stop(handle);
        System.out.println("NODE STOPPED " + stopped);
    }
}
