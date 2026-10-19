#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
version="$(python3 -c 'import tomllib;print(tomllib.load(open("Cargo.toml","rb"))["workspace"]["package"]["version"])')"
desktop=0
skip=0
full_setup=0
for arg in "$@"; do
  case "$arg" in
    --desktop) desktop=1;;
    --skip-build) skip=1;;
    --setup) full_setup=1; desktop=1;;
    *) echo "Unknown option $arg" >&2; exit 2;;
  esac
done
. /etc/os-release
[[ "$ID" == ubuntu && "$VERSION_ID" == 24.04 && "$(uname -m)" == x86_64 ]] || {
  echo 'Build on Ubuntu 24.04 x86_64 to match glibc.' >&2; exit 2;
}
command -v dpkg-deb >/dev/null
# FULL is assembled only from the frozen published thin pair and its exact manifest.
# It never rebuilds a thin package or creates a different template manifest.
if ((full_setup)); then
  for name in sshcc-client-ubuntu-template.deb sshcc-client-windows-template.exe template-manifest.json; do
    [[ -f "$root/dist/$name" ]] || { echo "Missing frozen release asset $name; build both thin templates and scripts/build-templates.py first." >&2; exit 2; }
  done
  python3 - "$root/dist" <<'PYVERIFY'
import hashlib,json,pathlib,sys
root=pathlib.Path(sys.argv[1]); manifest=json.loads((root/"template-manifest.json").read_text())
assert manifest["schemaVersion"]==1
expected={"windows":"sshcc-client-windows-template.exe","linux":"sshcc-client-ubuntu-template.deb"}
assert len(manifest["templates"])==2
for item in manifest["templates"]:
    assert expected.pop(item["os"])==item["fileName"]
    assert hashlib.sha256((root/item["fileName"]).read_bytes()).hexdigest()==item["sha256"], "Published template hash mismatch"
assert not expected
PYVERIFY
  stage="$(mktemp -d "$root/dist/deb-stage.XXXXXX")"
  bootstrap="$stage/setup"
  dpkg-deb -R "$root/dist/sshcc-client-ubuntu-template.deb" "$bootstrap"
  helper="$bootstrap/usr/lib/sshcc-setup"
  [[ ! -e "$helper/assets/envelope.json" ]] || { echo 'A personalized package cannot be used as the generic thin template.' >&2; exit 2; }
  [[ "$(dpkg-deb -f "$root/dist/sshcc-client-ubuntu-template.deb" Package)" == sshcc-setup && "$(dpkg-deb -f "$root/dist/sshcc-client-ubuntu-template.deb" Version)" == "$version" ]] || { echo 'Thin package name/version mismatch.' >&2; exit 2; }
  install -d -m 0755 "$helper/assets/release-templates"
  for name in sshcc-client-ubuntu-template.deb sshcc-client-windows-template.exe template-manifest.json; do
    install -m 0644 "$root/dist/$name" "$helper/assets/release-templates/$name"
  done
  python3 - "$helper/assets" "$version" <<'PYMANIFEST'
import hashlib,json,pathlib,sys
assets=pathlib.Path(sys.argv[1])
files={str(p.relative_to(assets)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(assets.rglob("*")) if p.is_file() and p.name not in ("manifest.json","envelope.json")}
(assets/"manifest.json").write_text(json.dumps({"schemaVersion":1,"version":sys.argv[2],"files":files},indent=2)+"\n",encoding="utf-8")
PYMANIFEST
  chmod 0644 "$helper/assets/manifest.json"
  (cd "$bootstrap" && find usr -type f -print0 | sort -z | xargs -0 md5sum > DEBIAN/md5sums)
  find "$bootstrap" -type d -exec chmod 00755 {} +
  dpkg-deb -Zgzip --root-owner-group --build "$bootstrap" "$root/dist/sshcc-${version}-ubuntu-setup.deb"
  (cd "$root/dist" && sha256sum ./*.deb > SHA256SUMS)
  echo "Generic Master installer: $root/dist/sshcc-${version}-ubuntu-setup.deb (published template pair unchanged)"
  exit 0
fi
release="${CARGO_TARGET_DIR:-$root/target}/release"
if ((!skip)); then
  cargo build --release --locked -p sshcc
  ((!desktop)) || cargo build --release --locked -p sshcc-desktop
fi
mkdir -p "$root/dist"
stage="$(mktemp -d "$root/dist/deb-stage.XXXXXX")"
# Keep the owned staging tree inspectable; no recursive delete of computed paths.
install -D -m 0755 "$release/sshcc" "$stage/sshcc/usr/bin/sshcc"
install -D -m 0644 installers/ubuntu/sshcc.service "$stage/sshcc/lib/systemd/system/sshcc.service"
install -D -m 0755 installers/ubuntu/setup.sh "$stage/sshcc/usr/lib/sshcc/setup.sh"
install -D -m 0755 installers/ubuntu/uninstall.sh "$stage/sshcc/usr/lib/sshcc/uninstall.sh"
install -D -m 0644 docs/INSTALL.md "$stage/sshcc/usr/share/doc/sshcc/INSTALL.md"
mkdir -p "$stage/sshcc/DEBIAN"
for script in preinst postinst prerm postrm; do install -m 0755 "installers/ubuntu/$script" "$stage/sshcc/DEBIAN/$script"; done
cat > "$stage/sshcc/DEBIAN/control" <<EOF
Package: sshcc
Version: $version
Section: admin
Priority: optional
Architecture: amd64
Maintainer: SSH Control Center maintainers
Depends: libc6 (>= 2.39), libgcc-s1, libsystemd0, openssh-client, python3, systemd
Recommends: openssh-server
Description: SSH Control Center service and agent CLI
 Durable privileged Target jobs, independent Masters and stdio MCP.
 Headless package; desktop dependencies are distributed separately.
EOF
find "$stage/sshcc" -type d -exec chmod 00755 {} +
dpkg-deb -Zgzip --root-owner-group --build "$stage/sshcc" "$root/dist/sshcc_${version}_amd64.deb"
if ((desktop)); then
  install -D -m 0755 "$release/sshcc-desktop" "$stage/desktop/usr/bin/sshcc-desktop"
  install -D -m 0755 "$release/sshcc-window" "$stage/desktop/usr/bin/sshcc-window"
  install -D -m 0644 desktop/src-tauri/icons/icon.png "$stage/desktop/usr/share/icons/hicolor/128x128/apps/sshcc.png"
  mkdir -p "$stage/desktop/usr/share/applications" "$stage/desktop/DEBIAN"
  cat > "$stage/desktop/usr/share/applications/sshcc.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=SSH Control Center
Exec=/usr/bin/sshcc-desktop --show
Icon=sshcc
Terminal=false
Categories=System;RemoteAccess;
EOF
  cat > "$stage/desktop/usr/share/applications/sshcc-tray.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=SSH Control Center tray
Exec=/usr/bin/sshcc-desktop
Icon=sshcc
Terminal=false
NoDisplay=true
X-GNOME-Autostart-enabled=true
EOF
  cat > "$stage/desktop/DEBIAN/control" <<EOF
Package: sshcc-desktop
Version: $version
Section: admin
Priority: optional
Architecture: amd64
Maintainer: SSH Control Center maintainers
Depends: sshcc (= $version), libc6 (>= 2.39), libgcc-s1, libgtk-3-0t64, libwebkit2gtk-4.1-0, libayatana-appindicator3-1, x-terminal-emulator
Description: SSH Control Center desktop tray
 Optional on-demand management window; system service is independent.
EOF
  find "$stage/desktop" -type d -exec chmod 00755 {} +
  dpkg-deb -Zgzip --root-owner-group --build "$stage/desktop" "$root/dist/sshcc-desktop_${version}_amd64.deb"
fi
# Thin client template: service payload + helper only, no recursively nested templates.
bootstrap="$stage/setup"
helper="$bootstrap/usr/lib/sshcc-setup"
install -d -m 0755 "$helper/assets" "$bootstrap/DEBIAN" "$bootstrap/usr/share/applications"
install -m 0755 installers/ubuntu/setup-helper.py "$helper/setup-helper.py"
install -m 0755 installers/ubuntu/setup-worker.sh "$helper/setup-worker.sh"
install -m 0644 installers/ubuntu/setup-util.py "$helper/setup-util.py"
install -m 0755 "$release/sshcc" "$helper/assets/sshcc"
install -m 0644 "$root/dist/sshcc_${version}_amd64.deb" "$helper/assets/sshcc-service.deb"
if ((desktop)); then install -m 0644 "$root/dist/sshcc-desktop_${version}_amd64.deb" "$helper/assets/sshcc-desktop.deb"; fi
install -D -m 0755 installers/ubuntu/sshcc-setup "$bootstrap/usr/bin/sshcc-setup"
install -m 0644 installers/ubuntu/sshcc-setup.desktop "$bootstrap/usr/share/applications/sshcc-setup.desktop"
install -m 0755 installers/ubuntu/setup-postinst "$bootstrap/DEBIAN/postinst"
install -D -m 0644 installers/ubuntu/org.sshcc.setup.policy "$bootstrap/usr/share/polkit-1/actions/org.sshcc.setup.policy"
cat > "$bootstrap/DEBIAN/control" <<EOF
Package: sshcc-setup
Version: $version
Section: admin
Priority: optional
Architecture: amd64
Maintainer: SSH Control Center maintainers
Depends: libc6 (>= 2.39), libgcc-s1, python3, util-linux, policykit-1
Recommends: zenity, xclip, wl-clipboard
Description: SSH Control Center approval and pairing setup
 Installing this helper does not authorize any Master or configure SSH.
 Open SSH Control Center Setup to approve; headless: sshcc-setup --terminal.
EOF
manifest() {
  python3 - "$helper/assets" "$version" <<'PY'
import hashlib,json,pathlib,sys
assets=pathlib.Path(sys.argv[1])
files={str(p.relative_to(assets)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(assets.rglob("*")) if p.is_file() and p.name not in ("manifest.json","envelope.json")}
(assets/"manifest.json").write_text(json.dumps({"schemaVersion":1,"version":sys.argv[2],"files":files},indent=2)+"\n",encoding="utf-8")
PY
  chmod 0644 "$helper/assets/manifest.json"
  (cd "$bootstrap" && find usr -type f -print0 | sort -z | xargs -0 md5sum > DEBIAN/md5sums)
}
manifest
find "$bootstrap" -type d -exec chmod 00755 {} +
dpkg-deb -Zgzip --root-owner-group --build "$bootstrap" "$root/dist/sshcc-client-ubuntu-template.deb"
(cd "$root/dist" && sha256sum ./*.deb > SHA256SUMS)
echo "Packages in $root/dist; inspectable staging in $stage"
