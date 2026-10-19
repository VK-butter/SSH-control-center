# Linux / real SSH acceptance harness

Run from the repository root (Docker is required):

    docker build -f tests/containers/Dockerfile -t sshcc-e2e:local .
    python tests/ssh_e2e.py --image sshcc-e2e:local --output e2e-report

`--output` is any directory the harness may create; it defaults to `artifacts/linux`, which is git-ignored. The harness writes `ssh-e2e-report.json`, one service log per container and a copy of the release CLI there.

The Docker build runs the `sshcc-core` Rust tests (`cargo test --locked -p sshcc-core`) and produces a release CLI on Debian 12.
Runtime fixtures use Ubuntu 24.04 and real OpenSSH, with five Targets and two
independent Master containers on an internal Docker network. No host services,
existing registries, SSH settings, Tailscale network, or remote machines are used.

Every container/network has a unique sshcc.e2e.run label. Cleanup only removes
resources created and recorded by this invocation; unrelated Docker resources
are never inspected or changed. This verifies Linux service foreground behavior
and SSH RPC; it does not claim systemd boot, Windows SSH, real WAN/Tailscale,
desktop performance, signed installers, or physical reboot certification.
