# UI fixture checks

The management UI (`desktop/ui`) can be exercised in a plain browser against an isolated fixture. This file lists what those checks cover and how to run them. The repository does not ship recorded results, screenshots or logs: run the checks to produce your own.

## Fixture

`node desktop/tests/fixture.cjs` serves `desktop/ui` on `http://127.0.0.1:42197` (override with `SSHCC_UI_TEST_PORT`) and replaces the Tauri IPC bridge with an in-memory stub. It does not use local IPC, SSH, a real Target or production data. Its stub answers only a small set of methods (`system.status`, `targets.*`, `jobs.*`, `files.*`, `enrollment.export`, `masters.list` and the shell-open command); anything else returns an error. `GET /calls` returns every request the page sent, which the checks inspect.

Fixture behaviors that the checks rely on:

- The first `jobs.start` records the job and then answers HTTP 504 ("simulated timeout after acceptance"). A correct client retries with the same job ID and ends up with exactly one job.
- Job stdout containing `<script>` text must render literally.
- `files.write` is rejected unless it carries the SHA-256 returned by the preceding `files.read`.
- The shell-open command answers with a session list when the Owner has several sessions, or opens directly when `SSHCC_FIXTURE_SINGLE_SESSION=1`.

## Manual checklist

[desktop/README.md](../README.md#ui-validation) lists the manual browser checks to run with Playwright CLI: job retry with the same UUID, literal rendering of script-like output, file edit with the SHA-256 precondition, the enrollment wizard mapping, and layouts at 1180px and 820px.

## Automated check

`desktop/tests/open-shell.cjs` drives the fixture with Playwright (it requires the `playwright` npm package and launches the Microsoft Edge channel; no `package.json` is shipped, so install it yourself). For a multi-session and a single-session fixture it checks that:

- the Shell button works from the fleet row and from the Target details page;
- with several Owner sessions a dialog appears, cancelling it sends no shell-open request, and the chosen session ID is sent;
- with one session no dialog appears and no session ID is sent;
- the request always carries the correct Target ID.

It writes screenshots and a results JSON under `artifacts/shell-button-ui/`, which is git-ignored.

## Not covered

These checks validate UI behavior only. They do not exercise the native tray, native file dialogs, the real service, SSH, installer service lifetime or idle RAM/CPU. See [docs/VALIDATION.md](../../docs/VALIDATION.md) for the other test layers and their limits.
