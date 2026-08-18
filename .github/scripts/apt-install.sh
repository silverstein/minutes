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

attempts=3
for attempt in $(seq 1 "${attempts}"); do
  if sudo timeout 300 apt-get update \
    && sudo timeout 600 apt-get install -y "$@"; then
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
