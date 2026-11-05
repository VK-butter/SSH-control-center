# UI fixture checks

The management UI (`desktop/ui`) can be exercised in a plain browser against an isolated fixture. This file lists what those checks cover and how to run them. The repository does not ship recorded results, screenshots or logs: run the checks to produce your own.

## Fixture

`node desktop/tests/fixture.cjs` serves `desktop/ui` on `http://127.0.0.1:42197` (override with `SSHCC_UI_TEST_PORT`) and replaces the Tauri IPC bridge with an in-memory stub. It does not use local IPC, SSH, a real Target or production data. It also serves `strings.js` and the bundled fonts. Its stub answers only these methods; anything else (method or native command) returns an error:

- `system.status`, `targets.list`, `targets.doctor`, `targets.update`, `fleet.health`
- `jobs.*`, `files.list`, `files.read`, `files.write`, `enrollment.export`
- pairing: `pairing.create` (`os: "unknown"` returns a Windows and an Ubuntu request linked by `sibling`; completing one removes the other, whose code then fails with `E_PAIRING_REPLACED`), `pairing.status` (list, one request, or address retry), `pairing.preview` (read only: host name, OS and a fake SHA-256 Host key fingerprint), `pairing.complete` (with an optional `label`), `pairing.cancel`, `pairing.check` (SSH answers only at 192.0.2.33 (Windows), 192.0.2.32 and 192.0.2.99 (Ubuntu) on port 22; 192.0.2.99 answers after 1.5 s; "already added" from the fixture's hosts and requests awaiting approval; with `requestId`, that request and its sibling are not counted)
- folders and datacenters: `folders.list`, `folders.create`, `folders.rename`, `folders.delete`, `folders.assign`, `datacenters.list`, `datacenters.create`, `datacenters.assign`
- Merges and relays: `merges.list`, `merges.discover`, `merges.add`, `merges.sync`, `merges.remove`, `relays.list`, `relays.allow`, `relays.revoke`
- allowed Masters: `masters.list`, `masters.revoke`, `enrollment.accept`
- native commands: shell open, `open_json` / `save_json` (always cancelled), `save_setup` ("save installer"; it returns a fixed path and writes nothing; for a sibling pair it returns `{folder, paths, files}` with the `.exe` and the `.deb`) and `merge_two_way` (marks the Merge two-way and adds the other Master as a relay); `targets.list` also lists each Merge's hosts as `<master>/<child>` (relayed)

Test-only endpoints:

- `GET /calls` returns every request the page sent, which the checks inspect.
- `GET /pairing-code[?requestId=…][&master=other]` returns the Pairing code the new host would show after Approve (default: the newest request awaiting approval). `master=other` gives a code made for another Master, which `pairing.preview` and `pairing.complete` refuse with `E_PAIRING_MASTER`. Any other text is refused as a damaged code.
- `POST /scenario` with `{"flags": [...]}` resets all state to that scenario and clears `/calls`.
- `POST /next-backup` with `{"kind": "good" | "newer" | "damaged" | "other"}` makes the next `open_backup` return that kind of file (once); "good" is a backup of the current fleet with the passphrase `fixture passphrase`. Otherwise `open_backup` returns the last file `save_backup` "wrote"; it is kept across scenario resets, so a check can back up, reset to `empty` and restore it.

Scenarios are comma-separated flags in `SSHCC_FIXTURE_SCENARIO` (or `POST /scenario`). With no flags the fixture has one Target, `ui-test`, and one allowed Master.

- `empty`: an empty Master: no hosts, folders, Merges, relays or allowed Masters.
- `new-host-master`: every host paired through the fixture is itself a Master, so `merges.discover` and `fleet.health` report `isMaster` and `merges.add` returns three merged hosts.
- `merge-fails`: `merges.add` / `merges.sync` fail as if the other Master did not answer.
- `needs-attention`: completing a pairing fails the connection check (`needs_attention` with one failed stage); an address retry (`pairing.status` with `routes`) then passes.
- `backed-up`: this Master has a last backup date (20 October 2026).
- `restored`: this Master was restored from a backup (24 October 2026) and its keys were not changed yet, so the section offers "Change keys…".
- `group-slow`: group jobs keep running until they are cancelled; jobs started by "Try again" finish as usual.
- `legacy-host`: adds `old-box`, a host registered without the SSH Control Center service (group runs skip it).
- `group-offline`: a group run can't reach `hq-master` ("No route to host"). Each `groups.status` call moves the started hosts on: queued → running → succeeded.
- `view-only-merge`: with `masters-page`, Branch Master gives this Master View only, so its hosts carry `access: "view"`.
- `merge-refuses`: with `masters-page`, a job on `branch-master/child-2` is refused with `E_MERGE_ROLE` (a Tag limit changed since the last sync).

- `multi-hop`: with `masters-page`, Branch Master also passes on Lab Master (a Master it merged) and Lab's two hosts, one of them View only (from Lab Master), and 3 hosts more than 4 hops away; a job on `branch-master/lab-master/lab-web` answers that Lab Master didn't answer in time.
- `outdated-merge`: with `masters-page`, Branch Master runs an older version (no `relayVersion`).

Relays carry their grant (`role`, `tags`, `passThrough`, Full control by default), and `relays.set` changes it.

- `six-digit`: `pairing.fetch` answers like a 0.5.0 installer: "waiting", then (once the request was approved with `GET /pairing-code`) "ready" with 6 digits and the review facts; `pairing.confirm` finishes or cancels (with its sibling). Without it, `pairing.fetch` answers "old", so the paste box shows as before.
- `old-disk`: `monitor.stats` reports only `/` (a host from before entry 8). Otherwise it reports three partitions (`/` ext4 41%, `/var` xfs 88%, `/boot/efi` vfat), with counters that advance each call.
- `outdated-hosts`: `ui-test` runs 0.4.0 while this Master runs 0.5.0; with `masters-page`, `hq-master` too, and `branch-master/child-1` (updated by Branch Master). An update finishes at once.
- `upgrade-fails`: with `outdated-hosts` and `masters-page`, updating `hq-master` times out (`E_UPGRADE_TIMEOUT`).
- `no-templates`: with `outdated-hosts`, this Master has no installer templates (a development build).
- `alarms-firing`: a red disk alarm (93.5%) and a yellow memory alarm on `ui-test`, which also has 3 OS updates pending; with `masters-page`, `hq-master` is unreachable (red).
- `alarm-new-red`: a red "unreachable" alarm on `ui-test` appears after the window's first look, for the notice.

Alarm rules start as the Master's three defaults; `alarms.rule_set` stores rules but the fixture doesn't evaluate them. `alarms.check_updates` finishes at once (`ui-test`: 1 update left).

- `activity-history`: the Activity tab has history: jobs today, yesterday and 3 days ago, a group run by Tag `web` (one host couldn't be reached), audit lines from every channel (one refused), a running job that finishes on the next refresh, and `lab-pc`, whose recorded job's host can't be reached.
- `activity-warning`: the audit log can't be written, so Activity shows its yellow warning (and records nothing).

Every audited request the page sends is written to the fixture's audit log as channel `desktop`, and every job it starts goes to Recent Jobs, as on the Master. `GET /last-csv` returns the last `save_csv` (`{name, csv}`).
- `rotation-trouble`: during Change keys, the first attempt on `ui-test` fails as if its Owner were not logged in; the next attempt works. (Each `keys.rotation_status` call moves one more host.)
- `two-way-merge`: with `masters-page`, the Merge with `branch-master` is two-way (it is also a relay here). A restore of it lists Branch Master under "Two-way needs enabling again"; `backup.restore` also lists the Masters that reach this one (`reachedBy`).
- `masters-page`: adds two hosts that are Masters (`branch-master`, already merged, and `hq-master`, not merged), an allowed Master `peer-both` that is also a relay, and a relay-only Master `peer-relay` with merging disabled.

`desktop/tests/decisions.cjs` holds the shared decision counter (each fill or choice on a field that was not pre-filled counts once) and `startFixture(flags)`, which starts the fixture on a free port for a check.

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

`desktop/tests/add-host.cjs` walks the one-column add flow ("+ Add host": address, Save installer…, paste the code) from an empty Master (plain Chromium) and fails if adding one host takes more than 2 decisions. The core task is a Windows host with no SSH answer (192.0.2.31), so the OS is unknown: it checks that no OS is asked, `pairing.create` is sent with `os: "unknown"` and step 2 names both installers. It also checks: one sidebar button with no dropdown; exactly one current step (`aria-current="step"`, acid only on its number) and focus on its main control; the review card in step 4 (fingerprint shown, no `pairing.complete` and no host in the tree before Finish; a name and folder changed on the card are what get saved; "Open this host" opens the host's page); the empty name sent as the address and the host shown in the tree after Finish; the `needs-attention` path (failures listed, address-only retry, no second installer); a code from another Master and a damaged code each shown as a plain sentence in place, with no submit button, one `pairing.preview` per distinct pasted value and no `pairing.complete`; an unfinished sibling pair listed as one row with Resume / Cancel, and Cancel cancelling both requests; a detected OS (192.0.2.32) making one request and one installer; the code of the replaced sibling shown as "This installer was replaced by the other one you already used"; "Add host here…" on a folder pre-selecting and applying it; the `new-host-master` scenario (Merged with no click: "This host is a Master · n hosts merged", only the new host asked by `merges.discover`, its hosts in the tree, Two-way from the card); opened from a datacenter, a Master filed under it and a non-Master left unfiled with no message; `merge-fails` (plain failure line, host kept, Merge offered on the Masters page); no motion under reduced motion; and no on-screen "target". It writes screenshots at 1180px and 820px and `add-host-results.json` under `artifacts/add-host-ui/`. Run it in the Playwright image:

    docker run --rm -v "$PWD":/src -v <node_modules with playwright>:/pw/node_modules:ro -e NODE_PATH=/pw/node_modules -w /src mcr.microsoft.com/playwright:v1.48.2-jammy node desktop/tests/add-host.cjs

`desktop/tests/masters-page.cjs` opens the Masters page on the `masters-page` scenario (plain Chromium). It checks both sections ("Controlled from here" and "This Master is controlled by"), that the unmerged Master is listed with Merge without any search and that no `merges.discover` request is sent, that `peer-both` is one row with "Direct" and "Through merge" badges, that "Allow another Master…" sits in the incoming section, and that Merge, Disable merging, Allow again and Revoke send their requests. Screenshots at 1180px and 820px go to `artifacts/add-host-ui/`.

`desktop/tests/language.cjs` checks the language switch EN | ไทย (plain Chromium). It checks that a `th-TH` browser opens in ไทย and `en-US` in EN; that one click on ไทย (1 decision) switches the open page, survives a reload and overrides the browser default; that switching keeps the page, the selected host, the open tab and typed input (Add host address, file filter); and that every page and dialog has no untranslated English in Thai: inventory, the tree's context menus, the current Master page, host summary with a connection check, the Jobs (with the new job form), Files, Containers and Monitoring tabs, the file menu, the delete and session dialogs, Service, Masters (`masters-page`) with the two-way dialog, and every Add host step (`empty`). The scan covers visible text plus `aria-label`, `title`, `placeholder` and select options, with every `<details>` opened. It skips code, logs, monospace values and text marked `data-raw` (shown as the service sends it). Master, Tag, Merge, SSH, technical words and every word in the fixture's own data are allowed. It also checks that a code from another Master shows the Thai sentence with the Master's English underneath (`.error-detail`), that an unmapped error (`merge-fails`) shows English with no detail line, and that every EN string key has a TH key and the other way round. It writes Thai screenshots at 1180px and 820px and `language-results.json` under `artifacts/language-ui/`.

`desktop/tests/backup.cjs` checks Backup and restore on the current Master's Summary tab (plain Chromium). It gates the core task, back up → wipe → restore, at 4 decisions or fewer: setting the passphrase (typed twice, counted once), choosing where to save, choosing the file and typing its passphrase. It checks that the section is on the Summary tab and not on the Service page; the last-backup line ("Never backed up", then the date); that Restore is unavailable on a Master with hosts; the passphrase rules (under 12 characters and two different passphrases each shown in place, Save unavailable until both match); both warnings; the default file name `sshcc-backup-<Master>-<date>.sshccbak` and the dialog title; that a cancelled save dialog keeps the typed passphrase; "Restore from a backup…" on an empty Master's Inventory next to "+ Add host"; the restore steps (choose the file, type its passphrase, a summary with the Master's name, date and counts, and Restore unavailable until "The old machine is off" is ticked); after the restore, the hosts back in the tree; and each wrong file shown as one plain sentence in place (not a backup, a newer Master, damaged, a wrong passphrase that keeps the chosen file, and a Master that is no longer empty); and after restoring a Master with a two-way Merge (`masters-page` + `two-way-merge`), the Masters that still use the old address ("Update this Master's address on …") and the Merge to enable again, with its Enable two-way button. For Change keys it checks: the choice offered after a restore (Change keys or Skip for now, both confirmations, not decisions), live progress ("n hosts still accept the old key"), "Keys changed on every host" with "Old backups no longer work. Back up now" and a Back up… button; and on a restored Master (`restored`), "Change keys…" in the section, a host that can't change yet named with the plain Owner sentence (`rotation-trouble`), merged hosts listed as changed by their own Master, and the last-backup line reading "Not backed up since keys were changed" until the next backup. `language.cjs` scans the same screens in Thai. It writes screenshots at 1180px and 820px and `backup-results.json` under `artifacts/backup-ui/`.

`desktop/tests/tags.cjs` checks Tags, the filter and group runs (plain Chromium). It checks that the filter box matches folder names, for direct and merged hosts, as well as names and addresses; Tags on the host page (adding with suggestions, case-insensitive with the first spelling kept, a merged host, each rule's error in place, × to remove); filtering by a typed Tag or a chip click; chips in host tables and on topology cards but not in the tree; "Tag…" in the tree menu; Tags kept through a (fixture) backup and restore; the New job form's working folder pre-filled with `/` or `C:\` by the host's OS; the core task (tag 3 hosts `web`, run one command on all three) gated at 5 decisions or fewer, with the Owner session field hidden for a group, nothing run before the confirmation, and one result row per host; and a folder group (`legacy-host`, `group-offline`) whose confirmation lists the skipped host and whose results show "Couldn't reach" with the reason, Skipped, and Open log; a mixed Linux + Windows group (`group-slow`) with one script box per OS and Shell and Owner session hidden, admin and env sent, a per-row cancel and Cancel all, "Try again on the hosts that failed", and the run on the current Master's Activity tab after a reload. It writes screenshots at 1180px and 820px and `tags-results.json` under `artifacts/tags-ui/`.

`desktop/tests/activity.cjs` checks Activity on the current Master (plain Chromium): the core task (find what ran on a host yesterday and export CSV) gated at 3 decisions or fewer, with the CSV's BOM, Thai times and filter checked; group runs as one row that expands, the status, 7-day and custom filters, a host filter that keeps its group run, a running job that finishes on its own, an offline host's recorded job, and Open on a reachable one; the audit view (translated actions, refused rows in rose, channels, a text search over actions, its CSV, and a change made in the page showing up); and "Recording since", the keep-days field (14 by default) and the warning. It writes screenshots at 1180, 820 and 390px and `activity-results.json` under `artifacts/activity-ui/`.

`desktop/tests/team-sharing.cjs` checks team sharing (plain Chromium). It checks:
- the core task (give a teammate's Master View only from "This Master is controlled by"), gated at 2 decisions or fewer;
- that a direct-only Master has no grant;
- the Tag limit with this Master's Tags suggested, the pass-through switch, and back to all hosts, each sent as one `relays.set`;
- a View-only host on the merging side, with the badge in the tree and on the page, and Shell, New job, Upload and New folder disabled with the reason, while a fully controlled host is untouched;
- a refusal shown as its sentence;
- multi-hop: Lab Master's branch nested in Branch Master's with nothing to do, a far host's "Path: … → Branch Master → Lab Master" and "View only (from Lab Master)", a Master on the way that didn't answer named in the notice, and the too-far count on the Masters page;
- an older merged Master marked "Outdated".

It writes screenshots at 1180, 820 and 390px and `team-sharing-results.json` under `artifacts/team-sharing-ui/`.

`desktop/tests/alarms.cjs` checks health alarms (plain Chromium). It checks:
- the core task (alert when any `web` host's disk is over 90%), gated at 4 decisions or fewer, with three rules from the start and disk / 90 / red pre-filled;
- the rule editor (chosen hosts, a container rule with names, edit, delete, a percentage outside 1–99 stopped in the field, the interval);
- active alarms (the sidebar count red then yellow, red first, Acknowledge greying a row, tree dots);
- the host page (alarms, OS updates with packages, Check now, "+ Add an alarm rule for this host" with the host chosen);
- the notice for a red alarm that appears while the window is open.

It writes screenshots at 1180, 820 and 390px and `alarms-results.json` under `artifacts/alarms-ui/`.

`desktop/tests/target-update.cjs` checks updating Targets (plain Chromium). It checks:
- "Needs update" badges in the tree and on the host page, with "by Branch Master" and no Update button for a merged host;
- Update on a host page: a timeout offers "Save the installer…", and a success clears the badge;
- "Update every outdated host" in the Inventory, one host at a time, with a failed row and the summary;
- a Master without templates showing the button disabled with the reason;
- partitions on the Monitoring tab (the fullest on the disk tile), a disk rule naming `/var`, and an older host's hint;
- adding a host with the 6-digit compare (1 decision, nothing pasted), and "No, it's different" cancelling the pair.

It writes screenshots under `artifacts/target-update-ui/`.

`node desktop/tests/strings-review.cjs` (no browser needed) writes `artifacts/language-ui/strings-review.html`: every string of `desktop/ui/strings.js` side by side in English and Thai, grouped by where it appears, with a box per row to write a fix and a button that copies all fixes as text. It is a translation review tool, not a check.

## Not covered

These checks validate UI behavior only. They do not exercise the native tray, native file dialogs, the real service, SSH, installer service lifetime or idle RAM/CPU. See [docs/VALIDATION.md](../../docs/VALIDATION.md) for the other test layers and their limits.
