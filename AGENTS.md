## UI Copy Policy

- Every visible text element must serve a concrete user need. Text is not decoration.
- Prefer self-explanatory interaction design over explanatory copy.
- Do not describe what is already apparent from the layout, controls, icons, labels, or surrounding context.
- Do not add subtitles, taglines, introductory paragraphs, or section descriptions by default.
- Do not expose implementation details, design intentions, or product requirements through UI copy.
- Use the shortest wording that preserves meaning.
- Prefer contextual help or progressive disclosure over permanently visible explanations.
- Preserve necessary accessibility labels, error messages, warnings, and instructions for non-obvious interactions.
- Before finalizing a UI, review every visible string. If removing it does not reduce usability, remove it.

When in doubt, omit the text rather than inventing copy.

## Android production-signed dev install

Use `android/scripts/install-dev-app.sh` to install a **production-signed dev build**
(the debug variant signed with the production release key). This keeps the existing
app data when updating a production-signed install. The ordinary debug APK has a
different signature and will fail with `INSTALL_FAILED_UPDATE_INCOMPATIBLE`.

The script reads the release keystore through the 1Password desktop integration.
In a sandboxed agent session, run it with **escalated / full file access**. Without
that, the 1Password CLI cannot write `/run/user/1000/op-daemon.pid` or reach the
desktop app, and fails with errors such as:

```text
couldn't start daemon: open /run/user/1000/op-daemon.pid: read-only file system
[ERROR] connecting to desktop app: cannot connect to 1Password app
```

Example (run the whole command with `danger-full-access`):

```sh
nix develop .#android --command bash -lc \
  './android/scripts/install-dev-app.sh --app mote --device 192.168.0.8:40345 --skip-rust'
```

The script signs and installs `target/prod-signing/mote-debug-prod.apk` (name
`*-debug-prod.apk`). Do not substitute the plain `mote-debug.apk` for a device that
already has the production-signed app.

Pair and connect the device before running the script. The script calls
`adb connect` but then waits indefinitely for the device, so confirm that
`adb devices -l` lists it as `device` first. A new pairing code and port are
required whenever the wireless debugging pairing dialog is reopened:

```sh
adb pair 192.168.0.8:<pairing-port> <pairing-code>
adb connect 192.168.0.8:<connect-port>
adb devices -l
```
