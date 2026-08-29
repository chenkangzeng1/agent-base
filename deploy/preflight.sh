#!/usr/bin/env bash
# ============================================================================
# deploy/preflight.sh — read-only readiness check. Writes nothing, publishes
# nothing, pushes nothing. Run it any time, and again before a release.
#
#   ./deploy/preflight.sh            human output
#   ./deploy/preflight.sh --quiet    silent, exit code only
# ============================================================================
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
. "$LIB_DIR/llm.sh"

QUIET=""
[ "${1:-}" = "--quiet" ] && QUIET=1
[ "${1:-}" = "--clear-cache" ] && { cache_clear; info "http cache cleared"; exit 0; }


[ -n "$QUIET" ] || section "preflight: $PROJECT_NAME"

# --- tools ------------------------------------------------------------------
say "${C_BD}tools${C_RST}"
for t in git cargo rustc curl jq awk sed grep; do
  have "$t" && pass "$t" || bad "$t not on PATH"
done
if have gh && gh auth status >/dev/null 2>&1; then
  pass "gh authenticated as $(gh api user --jq .login 2>/dev/null || echo '?')"
else
  skip "gh missing or unauthenticated (tag/release/CI steps need it)"
fi

# --- toolchain vs MSRV ------------------------------------------------------
say "${C_BD}toolchain${C_RST}"
if [ -f Cargo.toml ] && grep -qE '^rust-version' Cargo.toml 2>/dev/null; then
  declared=$(grep -m1 -E '^rust-version' Cargo.toml | sed -E 's/.*"([^"]*)".*/\1/')
  installed=$(rustc -V | awk '{print $2}')
  if semver_lte "$declared" "$installed"; then
    pass "rustc $installed satisfies declared rust-version $declared"
  else
    bad "rustc $installed is older than declared rust-version $declared"
  fi
  [ "$declared" = "$MSRV" ] || note "config MSRV=$MSRV but manifest says $declared"
else
  skip "no rust-version declared in the root manifest"
fi

# --- git state --------------------------------------------------------------
say "${C_BD}git${C_RST}"
if git_clean; then pass "working tree clean"; else
  bad "working tree dirty — a release must name an exact commit"
  # Honour --quiet: the detail belongs in the log, not stdout.
  if [ -z "$QUIET" ]; then git status --short | sed 's/^/       /' | tee -a "$RUN_LOG"; else git status --short | sed 's/^/       /' >> "$RUN_LOG"; fi
fi
br="$(current_branch)"
[ "$br" = "$DEFAULT_BRANCH" ] && pass "on branch $br" || bad "on branch $br, expected $DEFAULT_BRANCH"

if git rev-parse --verify "origin/$DEFAULT_BRANCH" >/dev/null 2>&1; then
  ahead=$(git rev-list --count "origin/$DEFAULT_BRANCH..HEAD")
  behind=$(git rev-list --count "HEAD..origin/$DEFAULT_BRANCH")
  [ "$ahead$behind" = "00" ] && pass "in sync with origin/$DEFAULT_BRANCH" \
    || note "$ahead commit(s) ahead, $behind behind origin"
else
  note "no origin/$DEFAULT_BRANCH ref locally (never pushed, or remote missing)"
fi

# --- remote repo ------------------------------------------------------------
if have gh && gh auth status >/dev/null 2>&1; then
  if gh repo view "$REPO_SLUG" >/dev/null 2>&1; then
    pub=$(gh repo view "$REPO_SLUG" --json visibility --jq .visibility 2>/dev/null || echo "?")
    pass "github.com/$REPO_SLUG exists ($pub)"
    [ "$pub" = "PUBLIC" ] || note "repo is not public; crates.io will link to a private repo"
  else
    bad "github.com/$REPO_SLUG not found or not visible to this token"
  fi
fi

# --- manifests --------------------------------------------------------------
say "${C_BD}crates${C_RST}"
versions=""
for entry in "${CRATES[@]}"; do
  name=$(crate_name_of "$entry"); path=$(crate_path_of "$entry")
  if [ ! -f "$path" ]; then bad "$name: manifest not found at $path"; continue; fi
  ver=$(manifest_version "$path")
  if [ -z "$ver" ]; then bad "$name: could not read [package] version from $path"; continue; fi
  pass "$name v$ver ($path)"
  versions="$versions $name=$ver"
  for field in license description; do
    grep -qE "^$field[[:space:]]*=" "$path" && : || bad "$name: manifest has no '$field' (crates.io requires it)"
  done
  grep -qE '^repository[[:space:]]*=' "$path" || note "$name: no repository field (crates.io page will have no link)"
done

# lockstep check
if [ "${#CRATES[@]}" -gt 1 ] && [ "$VERSION_LOCKSTEP" = "true" ]; then
  uniq_v=$(for v in $versions; do printf '%s\n' "${v#*=}"; done | sort -u | wc -l | tr -d ' ')
  if [ "$uniq_v" = "1" ]; then pass "all crates share one version (lockstep)"
  else bad "VERSION_LOCKSTEP=true but crates disagree: $versions"; fi
fi

# --- path dependencies that would block publish -----------------------------
say "${C_BD}publishability${C_RST}"
if [ "${#INTERNAL_DEPS[@]}" -gt 0 ]; then
  for spec in "${INTERNAL_DEPS[@]}"; do
    f="${spec%%|*}"; dep="$(printf '%s' "$spec" | cut -d'|' -f2)"
    [ -f "$f" ] || { bad "$f missing (from INTERNAL_DEPS)"; continue; }
    line=$(grep -E "^$dep[[:space:]]*=" "$f" | head -1 || true)
    if [ -z "$line" ]; then
      note "$f: no dependency named $dep (INTERNAL_DEPS may be stale)"
    elif printf '%s' "$line" | grep -qE 'version[[:space:]]*='; then
      pass "$f: $dep has a version requirement"
    else
      bad "$f: $dep is a path dep with no 'version' — cargo publish will refuse"
      printf '       %s\n' "$line" | tee -a "$RUN_LOG"
      note "$f: add version = \"<published>\" to the $dep dependency"
    fi
  done
fi

# Registry reachability and version freshness.
#
# "the manifest version is already published" is INFORMATION, not a blocker:
# release.sh's version step exists to pick the next number, so making this a
# failure produced a deadlock — preflight ran as step 0, failed, and the release
# aborted before the step that resolves it could run. Only an undecidable
# registry query is a real blocker, because publishing a burned version is
# unrecoverable.
for entry in "${CRATES[@]}"; do
  name=$(crate_name_of "$entry"); ver=$(manifest_version "$(crate_path_of "$entry")")
  st=$(crate_status "$name")
  case "$st" in
    published*)
      if index_has_version "$name" "$ver"; then
        note "$name@$ver is already published; a bump will be proposed (latest ${st#published })"
      else
        pass "$name: manifest $ver is new and publishable (latest online ${st#published })"
      fi ;;
    absent)
      pass "$name never published — $ver would be its first release" ;;
    *)
      # Unreachable is not absent: never report an all-clear on a crate we could
      # not look up.
      bad "$name: crates.io unreachable (not 404) — cannot decide publishability" ;;
  esac
done

# --- LLM --------------------------------------------------------------------
say "${C_BD}llm assistance${C_RST}"
if llm_resolve; then
  # Show the host only; never print a key or a full URL with embedded creds.
  host=$(printf '%s' "${LLM_URL#*://}" | cut -d/ -f1)
  pass "LLM configured (model ${LLM_MODEL_ID:-auto}, host $host)"
  # A live round trip costs 15-20s on a reasoning model and preflight should be
  # something you run casually, so it is opt-in rather than automatic.
  if [ "${PREFLIGHT_LLM_PING:-}" = "1" ]; then
    if llm_chat "Answer with exactly: OK" "ping" >/dev/null 2>&1; then
      pass "LLM round trip succeeded"
    else
      # A configured-but-dead key is worse than no key: it looks set up.
      bad "LLM configured but the call failed (revoked key or wrong endpoint)"
      note "release will fall back to deterministic conventional-commit rules"
    fi
  else
    note "not pinging the LLM (set PREFLIGHT_LLM_PING=1 to test the key)"
  fi
else
  if [ "$LLM_REQUIRED" = "true" ]; then
    bad "LLM_REQUIRED=true but no usable DEPLOY_LLM_API_KEY"
  else
    skip "no LLM key; version bump falls back to deterministic rules"
  fi
fi

# --- gates we can check cheaply right now ----------------------------------
if [ -f CHANGELOG.md ]; then
  grep -qi "unreleased" CHANGELOG.md && note "CHANGELOG.md has an Unreleased section (roll it into the release notes)"
fi
[ -f LICENSE ] && pass "LICENSE present" || bad "no LICENSE file"

say ""
say "${C_BD}$PASS passed${C_RST}, ${C_RED}$FAIL failed${C_RST}, ${C_DIM}$SKIP skipped${C_RST}   ${C_DIM}log: $RUN_LOG${C_RST}"
[ "$FAIL" -eq 0 ] || { [ -n "$QUIET" ] || err "preflight found blockers — resolve them before releasing"; exit 1; }
ok "preflight clean"
