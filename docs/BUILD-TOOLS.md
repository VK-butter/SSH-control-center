# Reproducible packaging notes

**What this page is.** Notes on the tools and checks used to build the installers. **Who it is for:** whoever builds the packages from source. The build steps themselves are in [INSTALL.md](INSTALL.md#build-packages).

The Windows setup executables are compiled with `makensis` from NSIS. A portable NSIS 3.11 extracted to a temporary directory is enough; no NSIS installation and no host service change is needed. `scripts/build-windows.ps1` takes the compiler through `-NsisPath`, or finds `makensis.exe` on `PATH`.

- Official release metadata: [NSIS 3.11 ZIP and published SHA256](https://sourceforge.net/projects/nsis/files/NSIS%203/3.11/nsis-3.11.zip/download).
- Mirror that is also referenced by the [Tauri bundler source](https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/windows/nsis/mod.rs): [Tauri's NSIS mirror](https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip).
- ZIP SHA256 as recorded by the author of these notes (this repository cannot prove it): `c7d27f780ddb6cffb4730138cd1591e841f4b7edb155856901cdf5f214394fa1`. Compare it with the value the publisher lists on the NSIS release page before extracting or running anything. A mismatch (for example an HTML page saved instead of the ZIP) means the download must be discarded.
- Put the verified compiler directory on the build process `PATH`. If you want the build output somewhere other than `target`, set `CARGO_TARGET_DIR` to the Cargo target directory itself: `scripts/build-windows.ps1` and `scripts/build-deb.sh` add `release` on their own, so do not add it yourself. No persistent `PATH` change is needed.
- Rebuild an existing owned staging directory with `scripts/build-windows.ps1 -Desktop -SkipBuild -RefreshStage`. The script refuses to reuse an existing staging directory without `-RefreshStage`, and rejects unexpected files or subdirectories in staging instead of packaging them.
- Artifacts are unsigned. The generated `SHA256SUMS` verifies file integrity and does not establish publisher identity.
- Compiler success verifies installer construction only. Installing the result on a machine and exercising the supported OS matrix are separate steps; see [VALIDATION.md](VALIDATION.md) for what the repository tests cover and what they do not.

Debian packages are built with `dpkg-deb -Zgzip --root-owner-group` by `scripts/build-deb.sh` (Ubuntu 24.04 x64 is the supported target). Service/CLI and desktop are separate packages. Check package contents and runtime dependencies before release. Keep all executable shell sources LF-only; `.gitattributes` enforces LF for `*.sh`, `*.py`, `*.rs`, `*.service` and the extensionless shell scripts in `installers/ubuntu` (`preinst`, `postinst`, `prerm`, `postrm`, `setup-postinst`, `sshcc-setup`).
