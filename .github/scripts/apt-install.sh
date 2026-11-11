#!/usr/bin/env bash
# Install apt packages on a CI runner, tolerating the two ways this step fails
# on GitHub's Linux images.
#
# The first is a transient mirror or DNS error, which exits non-zero quickly.
# The second is apt blocking on a dpkg or apt lock still held by the runner's
# own unattended-upgrades, which does not exit at all: the step simply produces
# no further output and the job sits there until the six hour job timeout. On
# 2026-08-18 that held every Linux job on the repo for over half an hour at a
# stretch, across three separate pull requests, with no failure to react to.
#
# Both cases are handled the same way: bound each apt invocation with `timeout`
# so a hang becomes a failure, then retry with a widening pause.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <package> [package...]" >&2
  exit 2
fi

export DEBIAN_FRONTEND=noninteractive

# GitHub's Ubuntu image ships third-party apt sources (Google Chrome, Microsoft
# Edge) whose mirrors intermittently answer `apt-get update` with "Hash Sum
# mismatch" and take the whole update down with them; on 2026-09-09 that
# failed every Linux job across four pull requests three attempts in a row.
# Nothing this repo installs comes from those sources, so drop them first.
sudo rm -f /etc/apt/sources.list.d/google-chrome*.list \
  /etc/apt/sources.list.d/google-chrome*.sources \
  /etc/apt/sources.list.d/microsoft-edge*.list \
  /etc/apt/sources.list.d/microsoft-edge*.sources

# Hosted runners can lose the route to the configured Ubuntu archive. Select
# a reachable mirror over IPv4 before apt starts; apt still verifies Ubuntu's
# signed indexes using the existing keys. Probe time is bounded independently
# of the apt lock/retry limits below.
choose_ubuntu_mirror() {
  local codename="$1" mirror
  for mirror in https://mirrors.edge.kernel.org/ubuntu https://archive.ubuntu.com/ubuntu; do
    if curl -4 --fail --silent --show-error --connect-timeout 5 --max-time 10 \
      --output /dev/null "$mirror/dists/$codename/InRelease"; then
      printf '%s\n' "$mirror"
      return 0
    fi
    echo "::warning::Ubuntu mirror probe failed: $mirror" >&2
  done
  echo "::error::No reachable Ubuntu archive mirror" >&2
  return 1
}

ubuntu_codename="$(awk -F= '/^VERSION_CODENAME=/ {gsub(/"/, "", $2); print $2}' /etc/os-release)"
ubuntu_mirror="$(choose_ubuntu_mirror "$ubuntu_codename")"
echo "::notice::Using Ubuntu mirror $ubuntu_mirror ($ubuntu_codename), with IPv4"
ubuntu_rewrite="s#https?://(azure\\.archive|archive|security)\\.ubuntu\\.com/ubuntu([/[:space:]]|$)#${ubuntu_mirror}\\2#g"
# Recent images use deb822 sources or mirror+file lists instead of sources.list.
for source in /etc/apt/sources.list /etc/apt/sources.list.d/*.list \
  /etc/apt/sources.list.d/*.sources /etc/apt/apt-mirrors*.txt; do
  [ -f "$source" ] || continue
  sudo sed -E -i "$ubuntu_rewrite" "$source"
done
apt_network=(-o Acquire::ForceIPv4=true -o Acquire::Retries=1 \
  -o Acquire::http::Timeout=15 -o Acquire::https::Timeout=15)

# Sized so all three attempts fit inside the twenty minute step backstop:
# 3 x (120 + 240) seconds of work plus 45 seconds of pauses is 18m45s. Without
# that the backstop would fire mid-retry and the third attempt would never run,
# which is the opposite of the point.
attempts=3
for attempt in $(seq 1 "${attempts}"); do
  if sudo timeout 120 apt-get "${apt_network[@]}" update \
    && sudo timeout 240 apt-get "${apt_network[@]}" install -y "$@"; then
    exit 0
  fi

  if [ "${attempt}" -lt "${attempts}" ]; then
    pause=$((attempt * 15))
    echo "::warning::apt attempt ${attempt} of ${attempts} failed or timed out, retrying in ${pause}s"
    sleep "${pause}"
  fi
done

echo "::error::apt failed after ${attempts} attempts: $*"
exit 1
