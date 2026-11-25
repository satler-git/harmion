// Control plane between a client app and the Harmion node. Data operations do
// not travel over Binder: they use the pinned HTTPS App API with an authorized
// token. Every method is synchronous from the caller's point of view.
package org.harmion.android.service;

interface IHarmionNodeService {
    // Public discovery: endpoint, pinned certificate, OAuth origin, ids.
    String connectionInfo();

    // Current node and sync status as JSON.
    String status();

    // Starts the node on demand; returns false if it could not start.
    boolean ensureRunning();

    // Requests a manual sync. The node still applies its own policy unless the
    // caller is the owner app; use the owner API for a user-confirmed sync.
    void requestSync();

    // Registers an approval request for the calling app and returns an opaque,
    // single-use token. Both values come from connect-start; the secret proves
    // that the caller owns this request.
    String beginApproval(String connectionId, String requestSecret);

    // Human-readable node summary for approval UIs (no secrets).
    String nodeSummary();

    // Records this package as an approved caller after connect-complete. Both
    // values come from connect-start; the node verifies the secret.
    boolean registerCaller(String connectionId, String requestSecret);
}
