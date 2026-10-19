#!/usr/bin/env bash
set -euo pipefail
[[ $EUID -eq 0 ]] || { echo 'Run with sudo.' >&2; exit 2; }
manifest=/var/lib/sshcc/installation.json
[[ -f "$manifest" ]] || { echo 'No owned installation manifest; nothing changed.' >&2; exit 2; }
owner_uid="$(python3 -c 'import json;print(json.load(open("/var/lib/sshcc/installation.json"))["owner"])')"
owner_home="$(getent passwd "$owner_uid" | cut -d: -f6)"
systemctl stop sshcc || true
systemctl disable sshcc || true
if [[ -f /etc/ssh/sshd_config.d/90-sshcc.conf ]]; then
  grep -q '^# SSHCC managed bridge configuration$' /etc/ssh/sshd_config.d/90-sshcc.conf || { echo 'Refusing to remove unowned SSH configuration.' >&2; exit 1; }
  cp -p /etc/ssh/sshd_config.d/90-sshcc.conf /var/lib/sshcc/uninstalled-sshcc.conf
  rm -- /etc/ssh/sshd_config.d/90-sshcc.conf
  if ! /usr/sbin/sshd -t; then
    cp -p /var/lib/sshcc/uninstalled-sshcc.conf /etc/ssh/sshd_config.d/90-sshcc.conf
    echo 'sshd validation failed; owned block restored.' >&2; exit 1
  fi
  systemctl reload ssh
fi
if [[ -n "$owner_home" && "$owner_home" == /* ]]; then
  rm -f -- "$owner_home/.config/autostart/sshcc-tray.desktop"
fi
packages=(sshcc)
dpkg-query -W sshcc-desktop >/dev/null 2>&1 && packages+=(sshcc-desktop)
apt-get remove -y "${packages[@]}"
echo 'Removed product packages; state, keys, logs, bridge account and prerequisites retained for rollback.'
