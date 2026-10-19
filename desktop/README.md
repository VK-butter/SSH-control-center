# Native desktop

The sshcc-desktop binary is the native tray. Windows uses a Tauri 2 event loop;
Ubuntu uses GTK/AppIndicator. Both start a separate Tauri sshcc-window process
only when the management window is needed. The desktop does not run or install
the service. Its window uses the same owner-authorized local IPC client as
sshcc request and sshcc mcp.

From the repository root, build with: cargo build --release -p sshcc-desktop.
Windows needs the supported Tauri Windows toolchain and WebView2; Ubuntu needs
Tauri's GTK/WebKitGTK/AppIndicator build prerequisites. There is no frontend
build step, package manager, network font, React or Node runtime in the product.

- Launch without arguments for the login tray. It creates no startup WebView.
- Launch with --show for the Start Menu / application launcher.
- Clicking the tray or its Open menu creates the management window on a worker,
  avoiding the Windows synchronous WebView construction deadlock.
- Closing the window hides it, so the next tray click reopens it instantly. After
  10 idle minutes (SSHCC_WINDOW_IDLE_SECS; 0 means exit on close) sshcc-window
  exits, releasing its WebView processes. Windows confines each launch to a
  private JobObject and waits for remaining WebView2 descendants to exit. The
  native tray remains.
- Quit in the tray exits only sshcc-desktop; the independent service continues.
- A second launch focuses the existing instance.
- If the service is absent, the window displays an actionable error; it never
  presents fixture or fabricated fleet state.

The UI has Thai guidance and English technical terms. It provides fleet search
and tenant filters, registration and version-1 registry import, public enrollment export using
native file dialogs, route editing, diagnostics, jobs and paginated stdout/stderr,
cancellation, UTF-8 file editing with SHA-256 preconditions, directory operations,
local Master access management and service information. Job and file data are
escaped before rendering. Binary or truncated file reads are not writable.

Routes retain their explicit array order. Tenant changes require removing and
re-enrolling the Target because tenant keys are separate. Adding a Master via
the access page registers its public identity; the Target installer is still
needed to install its forced-command SSH authorized-key entry.

## Acceptance benchmark

Start with no existing sshcc-desktop instance:

    sshcc-desktop.exe --benchmark-cycles 3 --benchmark-report C:\Temp\sshcc-cycles.json

Both flags are required; cycles must be 1..10. This explicit test mode opens the
real window, waits five seconds, closes it normally, waits for its removal, then
settles five seconds per cycle. It leaves the native tray running. It does not
start the service, a debug endpoint or a network listener.

The report contains ok, pid, cyclesRequested, cyclesCompleted,
windowAbsent, completedAtUnixSeconds and error. Use a fresh report path,
check ok and the requested cycle count, then sample the tray plus the service
and remaining descendants for at least ten minutes. windowAbsent alone does
not prove WebView child processes exited; include them in process measurements.
On both platforms the parent validates each child lifecycle report and waits
for process exit before recording a completed cycle; windowProcessIsolation is
true. Windows also records windowsJobContainment after its private job is empty.
Single-instance activation redirects duplicate launches, so close a previous
test tray before starting a new benchmark.

## UI validation

Run node tests/fixture.cjs to start an explicitly isolated fixture at
http://127.0.0.1:42197 (override SSHCC_UI_TEST_PORT). It does not use local IPC,
SSH, files on a Target, or production data. Its first job start accepts a job and
then returns a simulated timeout; submitting the same form again must reuse the
UUID and return exactly one job.

Use Playwright CLI against this fixture to check:
1. Fleet → Target → jobs: submit a command, retry after the simulated timeout;
   inspect /calls to confirm both jobs.start requests have the same job ID.
2. Script-like stdout renders literally, with no inserted script elements.
3. Files: open /tmp/example.txt, edit Thai UTF-8 text and save; confirm the
   files.write request carries the SHA-256 from files.read.
4. Enrollment: Windows/Linux choice, sshcc-bridge, route port and public
   bundle map to the service contract.
5. Both 1180px and 820px layouts remain usable.

Fixture checks validate UI behavior only. Native file dialogs, actual SSH,
installer service lifetime and idle RAM/CPU are not covered by the fixture; see
[docs/VALIDATION.md](../docs/VALIDATION.md) for what is and is not tested.

References: [Tauri tray](https://v2.tauri.app/learn/system-tray/),
[dialog](https://v2.tauri.app/plugin/dialog/),
[single instance](https://v2.tauri.app/plugin/single-instance/).

## Ubuntu container acceptance

desktop/tests/Dockerfile.ubuntu builds against Ubuntu 24.04 native libraries. The isolated
desktop/tests/benchmark-ubuntu.sh harness runs the release service and tray with 50 offline
fixture targets, a private DBus session, Xvfb X11 display and tint2 tray host. It
uses the same benchmark.py process collector as Windows and requires 10 completed
native window cycles before at least 600 seconds of idle sampling.

This is a real GTK/WebKit native lifecycle and process measurement, not a browser
UI fixture. It does not establish GNOME/Wayland, real GPU, login/logout, or physical
desktop behavior. The container alone disables WebKit's sandbox because it runs
as root; production startup does not set that override. Testing on a real
desktop session is not covered by this harness.