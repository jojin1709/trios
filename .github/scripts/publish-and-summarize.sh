#!/usr/bin/env bash
# Publishes the captured PNG screenshots to the `ci-screenshots` branch
# (force-pushed each run, so the branch only ever holds the latest run's
# images) and writes the GitHub Actions job summary ($GITHUB_STEP_SUMMARY)
# with the screenshots embedded inline plus the tail of the serial log.
#
# Usage: publish-and-summarize.sh <boot-artifacts-dir>
# Env:   GITHUB_REPOSITORY, GITHUB_SHA, GITHUB_STEP_SUMMARY (set by Actions)
#        GH_TOKEN  - github.token, needed to push the ci-screenshots branch
#
# Private-repo note: raw.githubusercontent.com embeds are fetched by GitHub
# Camo without auth, which can break on private repos; every image is also a
# hyperlink to its github.com blob page (session-authenticated) as a fallback.
set -euo pipefail

BOOT_DIR="${1:-boot-artifacts}"
REPO="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must be set}"
SHA="${GITHUB_SHA:-unknown}"
SHORT_SHA="${SHA:0:7}"
BRANCH="ci-screenshots"
RAW_BASE="https://raw.githubusercontent.com/${REPO}/${BRANCH}"
BLOB_BASE="https://github.com/${REPO}/blob/${BRANCH}"

shopt -s nullglob
PNGS=( "$BOOT_DIR"/*.png )

# --- 1. push screenshots to the ci-screenshots branch ------------------------
if [ "${#PNGS[@]}" -gt 0 ] && [ -n "${GH_TOKEN:-}" ]; then
  STAGING="$(mktemp -d)"
  cp "${PNGS[@]}" "$STAGING/"
  git -C "$STAGING" init --quiet --initial-branch="$BRANCH"
  git -C "$STAGING" config user.name "github-actions[bot]"
  git -C "$STAGING" config user.email "41898282+github-actions[bot]@users.noreply.github.com"
  git -C "$STAGING" add -A
  git -C "$STAGING" commit --quiet -m "CI boot screenshots for ${SHORT_SHA} (run ${GITHUB_RUN_ID:-local})"
  # Auth via -c http.extraheader (same mechanism actions/checkout uses) so the
  # token never appears in the remote URL or in any error output.
  AUTH="$(printf 'x-access-token:%s' "$GH_TOKEN" | base64 -w0)"
  git -C "$STAGING" -c "http.extraheader=AUTHORIZATION: basic ${AUTH}" \
    push --quiet --force "https://github.com/${REPO}.git" "HEAD:refs/heads/${BRANCH}"
  rm -rf "$STAGING"
  echo "Pushed ${#PNGS[@]} screenshot(s) to branch '${BRANCH}'"
elif [ "${#PNGS[@]}" -eq 0 ]; then
  echo "::warning::no PNG screenshots found in ${BOOT_DIR}; summary will note the gap"
else
  echo "::warning::GH_TOKEN not set; skipped pushing screenshots to '${BRANCH}'"
fi

# --- 2. write the job summary ------------------------------------------------
SUMMARY="${GITHUB_STEP_SUMMARY:-/dev/stdout}"
SERIAL="$BOOT_DIR/serial.log"
IMAGE="target/release/trios-uefi-release.img"

{
  echo "## TriOS boot report — \`${SHORT_SHA}\`"
  echo
  echo "Commit \`${SHA}\` · run [${GITHUB_RUN_ID:-local}](${GITHUB_SERVER_URL:-https://github.com}/${REPO}/actions/runs/${GITHUB_RUN_ID:-0}) · image \`trios-uefi-release.img\` booted headless in QEMU (OVMF UEFI firmware, TCG)."
  echo

  echo "### Boot screenshots (VGA framebuffer via QEMU \`screendump\`)"
  echo
  if [ "${#PNGS[@]}" -eq 0 ]; then
    echo "_No screenshots were captured — check the serial log below and the step log for QEMU/OVMF errors._"
    echo
  else
    for png in "${PNGS[@]}"; do
      name="$(basename "$png")"
      # shot-3s.png -> 3
      secs="$(basename "$png" .png)"
      secs="${secs#shot-}"
      secs="${secs%s}"
      echo "**\`${name}\`** — ~${secs} s after power-on ([full size](${BLOB_BASE}/${name}))"
      echo
      echo "![${name}](${RAW_BASE}/${name})"
      echo
    done
  fi

  if [ -f "$IMAGE" ]; then
    echo "Disk image: \`trios-uefi-release.img\` ($(du -h "$IMAGE" | cut -f1)) — downloadable from this run's **Artifacts**."
    echo
  fi

  echo "### Serial console — last 100 lines (COM1 / 0x3F8)"
  echo
  echo '```text'
  if [ -s "$SERIAL" ]; then
    # Strip ANSI/VT100 escapes (kernel emits clears like [2J) and CRs so the
    # summary code block stays readable.
    perl -pe 's/\e\[[0-9;?]*[ -\/]*[@-~]//g; s/\e\][^\a\e]*(?:\a|\e\\)//g; s/\r//g' \
      "$SERIAL" | tail -n 100
  elif [ -f "$SERIAL" ]; then
    echo "(serial.log exists but is empty — the kernel never reached the UART,"
    echo " or UEFI did not boot the image. See the serial.log artifact.)"
  else
    echo "(no serial.log was produced — QEMU likely failed to start; see the step log.)"
  fi
  echo '```'
  echo
  echo "_Full serial log, disk image and PNGs are in the **Artifacts** section of this run._"
} >> "$SUMMARY"

echo "Job summary written to ${SUMMARY}"
