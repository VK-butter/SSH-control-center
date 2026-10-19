# Security policy

SSH Control Center is a personal, open-source showcase project. It is not a commercial product and it has no support contract. Please read this page before you report a problem or rely on the tool.

## Supported version

Only the latest version gets security fixes. That is currently the **0.4.0** source tree (see `Cargo.toml`), and its installers are not code-signed. Older versions are not patched.

## How to report a vulnerability

Please use **GitHub private vulnerability reporting** on the repository [VK-butter/SSH-control-center](https://github.com/VK-butter/SSH-control-center):

1. Open the repository on GitHub.
2. Go to the **Security** tab.
3. Choose **Report a vulnerability** and describe what you found.

Please do not open a public issue or pull request with details of a security problem, and please do not publish details before it is fixed. No e-mail address is offered for reports. If the **Report a vulnerability** button is not shown, open a public issue that says only that you have a security report and need a private way to send it. Do not put any details in that issue.

Helpful details: the version, the operating system of the Master and of the Target, the steps to reproduce, and what an attacker would gain.

## What to expect

This project is run by one person, under the handle `VK-butter`, in spare time. Replies and fixes are **best effort**, with no promised response time. There is no bug bounty.

## Known limits come first

Some risks are known and are written down on purpose in the [security model](docs/security-model.md) (Thai: [docs/th/security-model.md](docs/th/security-model.md)). For example, the tool gives administrator-level power over every connected machine by design, it asks no extra permission before running a command, and this repository contains no third-party security audit. A report about a documented limit is still welcome, but a new finding helps more.

## Scope

In scope: the code, installers and scripts in this repository.

Out of scope: the operating systems, OpenSSH, Tailscale and other software you run next to it, and mistakes in how you set up your own network.
