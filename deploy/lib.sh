#!/usr/bin/env bash
# ============================================================================
# deploy/lib.sh — shared helpers. Project-agnostic: copy verbatim between repos.
# Written for bash 3.2 (macOS default): no associative arrays, no ${var,,}.
# ============================================================================

LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$LIB_DIR/.." && pwd)"
cd "$PROJECT_ROOT" || exit 1

. "$LIB_DIR/config.sh"

# --- colour / logging -------------------------------------------------------
if [ -t 1 ] && [ "${TERM:-}" != "dumb" ] && [ -z "${NO_COLOR:-}" ]; then
  C_DIM=$'\033[2m'; C_RED=$'\033[31m'; C_GRN=$'\033[32m'; C_YEL=$'\033[33m'
  C_BLU=$'\033[34m'; C_MG=$'\033[35m'; C_BD=$'\033[1m'; C_RST=$'\033[0m'
else
  C_DIM=""; C_RED=""; C_GRN=""; C_YEL=""; C_BLU=""; C_MG=""; C_BD=""; C_RST=""
fi

mkdir -p "$LOG_DIR" 2>/dev/null || true
# Absolute, because `cd fuzz && run ...` would otherwise resolve a relative
# LOG_DIR against fuzz/ and every tee in run() would fail mid-step.
LOG_DIR_ABS="$(cd "$LOG_DIR" 2>/dev/null && pwd || printf '%s' "$LOG_DIR")"
RUN_LOG="$LOG_DIR_ABS/run-$(date -u +%Y%m%dT%H%M%SZ).log"

_log() { # level colour message
  local lvl="$1" col="$2"; shift 2
  printf '%s %s%-7s%s %s\n' "$(date +%H:%M:%S)" "$col" "$lvl" "$C_RST" "$*" | tee -a "$RUN_LOG"
}
info()  { _log info  "$C_BLU" "$@"; }
ok()    { _log ok    "$C_GRN" "$@"; }
warn()  { _log warn  "$C_YEL" "$@"; }
err()   { _log error "$C_RED" "$@" >&2; }
debug() { if [ -n "${DEPLOY_VERBOSE:-}" ]; then _log debug "$C_DIM" "$@"; fi; }
die()   { err "$@"; exit 1; }

# --- pass/fail accounting ---------------------------------------------------
# Shared so every script reports the same way. Defined here rather than per
# script: verify.sh called `bad` for a failing check and crashed with
# "command not found", which turns a reported failure into a silent abort.
# QUIET (set by a caller) suppresses the human lines but keeps the counters and
# the log file, so --quiet still yields an accurate exit code.
PASS=0; FAIL=0; SKIP=0
_emit() { # colour marker message
  local col="$1" mark="$2" msg="$3"
  printf '  %s%s%s %s\n' "$col" "$mark" "$C_RST" "$msg" | tee -a "$RUN_LOG"
}
pass() { PASS=$((PASS+1)); [ -n "${QUIET:-}" ] || _emit "$C_GRN" "✔" "$*"; }
bad()  { FAIL=$((FAIL+1)); _emit "$C_RED" "✘" "$*" >&2; }
skip() { SKIP=$((SKIP+1)); [ -n "${QUIET:-}" ] || _emit "$C_DIM" "–" "$*"; }
note() { [ -n "${QUIET:-}" ] || _emit "$C_YEL" "!" "$*"; }
say()  {
  if [ -n "${QUIET:-}" ]; then printf '  %s\n' "$*" >> "$RUN_LOG"
  else printf '  %b\n' "$*" | tee -a "$RUN_LOG"; fi
}

summary_and_exit() { # [fail_is_fatal]
  local fatal="${1:-1}"
  printf '\n' | tee -a "$RUN_LOG"
  printf '  %s%d passed%s, %s%d failed%s, %s%d skipped%s   %slog: %s%s\n' \
    "$C_GRN" "$PASS" "$C_RST" "$C_RED" "$FAIL" "$C_RST" \
    "$C_DIM" "$SKIP" "$C_RST" "$C_DIM" "$RUN_LOG" "$C_RST" | tee -a "$RUN_LOG"
  if [ "$FAIL" -gt 0 ] && [ "$fatal" = "1" ]; then
    err "found blockers"
    return 1
  fi
  return 0
}

section() {
  local pad=$(( 44 - ${#1} )); [ "$pad" -lt 0 ] && pad=0
  printf '\n%s═══ %s %s%s\n' "$C_BD$C_MG" "$*" \
    "$(printf '═%.0s' $(seq 1 "$pad" 2>/dev/null))" "$C_RST" | tee -a "$RUN_LOG"
}

# --- .env loading -----------------------------------------------------------
# Dotenv semantics: variables already present in the environment win. Plain
# `set -a; . .env` would CLOBBER a live exported key with whatever is in the
# file — a repo's .env may hold a revoked key while the shell holds a working
# one, and a release tool must not silently pick the dead value. Parsing rather
# than sourcing also means .env cannot execute code.
load_dotenv() { # <file>
  local f="$1" line key val
  [ -f "$f" ] || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in ''|'#'*) continue ;; esac
    case "$line" in 'export '*) line="${line#export }" ;; esac
    case "$line" in *=*) ;; *) continue ;; esac
    key="${line%%=*}"
    val="${line#*=}"
    key="$(printf '%s' "$key" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
    case "$key" in ''|*[!A-Za-z0-9_]*) continue ;; esac
    case "$key" in [0-9]*) continue ;; esac
    val="$(printf '%s' "$val" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
    case "$val" in
      \"*\") val="${val#\"}"; val="${val%\"}" ;;
      \'*\') val="${val#\'}"; val="${val%\'}" ;;
    esac
    if [ -n "$(eval "printf '%s' \"\${$key+set}\"")" ]; then
      debug ".env: $key already set in environment; keeping the existing value"
    else
      eval "export $key=\"\$val\""
    fi
  done < "$f"
}
load_dotenv "$PROJECT_ROOT/.env"

# --- state (for --from / --only resumability) ------------------------------
# Absolute for the same reason as RUN_LOG: steps that cd into fuzz/ must still
# be able to record progress.
STATE_DIR_ABS="$(mkdir -p "$STATE_DIR" 2>/dev/null && cd "$STATE_DIR" && pwd || printf '%s' "$STATE_DIR")"
STATE_DIR="$STATE_DIR_ABS"
state_init() { mkdir -p "$STATE_DIR" 2>/dev/null || true; }
state_mark() { state_init; printf '%s\t%s\n' "$1" "$(date -u +%FT%TZ)" >> "$STATE_DIR/steps.done"; }
state_done() {
  [ -f "$STATE_DIR/steps.done" ] || return 1
  awk -F'\t' -v s="$1" '$1==s{f=1} END{exit !f}' "$STATE_DIR/steps.done"
}
state_clear() { rm -rf "$STATE_DIR"; }
state_write() {
  # Guarded here too, not just in the caller loop: a rehearsal that records a
  # version or commit would mislead the next real run.
  if [ -n "${DEPLOY_DRY:-}" ]; then debug "dry run: not recording state '$1'"; return 0; fi
  state_init
  printf '%s\n' "$2" > "$STATE_DIR/$1"
}
state_read()  { [ -f "$STATE_DIR/$1" ] && cat "$STATE_DIR/$1" || printf ''; }

# --- command availability ---------------------------------------------------
have() { command -v "$1" >/dev/null 2>&1; }
need() {
  local missing="" t
  for t in "$@"; do have "$t" || missing="$missing $t"; done
  [ -z "$missing" ] || die "missing required tools:$missing"
}

# --- run a command, capturing its output ------------------------------------
# usage: run <label> <cmd...>
run() {
  local label="$1"; shift
  debug "$label: $*"
  if "$@" >>"$RUN_LOG" 2>&1; then
    ok "$label"
    return 0
  fi
  local st=$?
  err "$label failed (exit $st)"
  tail -n 25 "$RUN_LOG" | sed 's/^/      │ /' >&2
  return $st
}

# --- confirmation -----------------------------------------------------------
# Returns 0 to proceed. In non-interactive mode it never prompts, which is why
# that mode requires an explicit --yes for anything irreversible. A dry run also
# never prompts: there is nothing to approve, and gate_action still prints what
# it would have done.
confirm() { # prompt [irreversible]
  local prompt="$1" irreversible="${2:-}"
  if [ -n "${DEPLOY_DRY:-}" ]; then
    debug "dry run, not prompting: $prompt"
    return 0
  fi
  if [ "$CONFIRM_MODE" != "interactive" ]; then
    debug "auto mode, not prompting: $prompt"
    return 0
  fi
  if [ -z "$irreversible" ] && [ -n "${DEPLOY_ASSUME_YES:-}" ]; then
    debug "assumed yes (reversible step): $prompt"
    return 0
  fi
  [ -r /dev/tty ] || { warn "no tty available to confirm: $prompt"; return 1; }
  local ans
  printf '%s%s%s [y/N] ' "$C_YEL$C_BD" "$prompt" "$C_RST" > /dev/tty
  read -r ans < /dev/tty || return 1
  case "$ans" in [yY]|[yY][eE][sS]) return 0 ;; *) return 1 ;; esac
}

# --- git --------------------------------------------------------------------
git_clean() { [ -z "$(git status --porcelain)" ]; }

git_ensure_clean() {
  if ! git_clean; then
    err "working tree is dirty; refusing to release"
    git status --short | sed 's/^/      /' >&2
    die "a release must name an exact commit — commit or stash the above first"
  fi
}

current_branch() { git rev-parse --abbrev-ref HEAD; }
head_subject()   { git log -1 --format=%s; }

# --- crate metadata ---------------------------------------------------------
crate_name_of() { printf '%s' "${1%%:*}"; }
crate_path_of() { printf '%s' "${1#*:}"; }

manifest_version() { # <path-to-Cargo.toml>
  awk -F'"' '
    BEGIN{inpkg=0}
    /^\[package\]/ {inpkg=1; next}
    /^\[/          {if (inpkg) inpkg=0; next}
    inpkg && /^version[[:space:]]*=/ {print $2; exit}
  ' "$1"
}

# Rewrite version = "x" inside [package] only. Leaves [workspace] and
# [dependencies] alone, which is what makes this safe on a root manifest.
manifest_set_version() { # <toml> <new-version>
  local f="$1" v="$2" tmp
  tmp="$(mktemp)" || return 1
  awk -v v="$v" '
    BEGIN{inpkg=0; done=0}
    /^\[package\]/ {inpkg=1; print; next}
    /^\[/          {if (inpkg) inpkg=0; print; next}
    inpkg && !done && /^version[[:space:]]*=/ {print "version = \"" v "\""; done=1; next}
    {print}
  ' "$f" > "$tmp" && mv "$tmp" "$f" && rm -f "$tmp.x"
  grep -q "^version = \"$v\"" "$f" || { err "failed to rewrite version in $f"; return 1; }
}

# Give a path dependency a version requirement, or update the one it has.
# cargo refuses to publish a crate whose path deps lack `version`.
#
# Handles the shapes that occur in practice:
#   dep = { path = "p", version = "1" }     -> replace the version
#   dep = { path = "p" }                    -> insert after path
#   dep = { path = "p", features = [...] }  -> insert after path
# The path text is never re-quoted, only the version key is written.
manifest_set_dep_version() { # <toml> <dep> <new-version>
  local f="$1" dep="$2" v="$3" tmp
  tmp="$(mktemp)" || return 1
  awk -v d="$dep" -v v="$v" '
    {
      if ($0 ~ "^" d "[ \t]*=") {
        if ($0 ~ /version[ \t]*=[ \t]*"[^"]*"/) {
          sub(/version[ \t]*=[ \t]*"[^"]*"/, "version = \"" v "\"")
          print; next
        }
        if ($0 ~ /path[ \t]*=[ \t]*"[^"]*"/) {
          # append a comma after the path entry, then the version
          sub(/path[ \t]*=[ \t]*"[^"]*"/, "&, version = \"" v "\"")
          print; next
        }
        print; next
      }
      print
    }
  ' "$f" > "$tmp" && mv "$tmp" "$f"

  # Refuse to leave a broken manifest behind.
  if ! grep -qE "^$dep[ \t]*=[ \t]*\{.*version[ \t]*=[ \t]*\"$v\"" "$f"; then
    err "could not set version for $dep in $f"
    grep -E "^$dep[ \t]*=" "$f" | sed 's/^/      /' >&2
    return 1
  fi
  return 0
}

# --- crates.io --------------------------------------------------------------
# The sparse index is the authoritative machine view. The crates.io website is a
# SPA that can 404 for minutes after a successful publish, and its API lags too;
# never judge a publish by an HTML fetch.
# Overridable so the tool can target a private mirror, and so the failure paths
# can be tested without waiting for a real outage. Assigning these unconditionally
# silently discards any value set in the environment.
CRATES_API="${DEPLOY_CRATES_API:-https://crates.io/api/v1}"
SPARSE_INDEX="${DEPLOY_SPARSE_INDEX:-https://index.crates.io}"
# crates.io rejects a bare `curl/8.x` default User-Agent with 403, so every
# request must carry one. Override via DEPLOY_UA if you add contact info.
UA="${DEPLOY_UA:-deploy-script/1.0}"
# Every network call is bounded: an unbounded curl turns a stalled endpoint
# into a script that appears to hang forever.
HTTP_TIMEOUT="${HTTP_TIMEOUT:-30}"

http() { curl -s --max-time "$HTTP_TIMEOUT" -H "User-Agent: $UA" "$@"; }

sparse_index_path() { # <name>
  # 1/, 2/, 3/<first>/, else <first-2>/<chars-3-4>/<name>.
  # "llm-trait" is ll/m-/llm-trait. A wrong path 404s, which would make a
  # published crate look unpublished.
  local n="$1" len=${#1}
  case "$len" in
    1) printf '1/%s' "$n" ;;
    2) printf '2/%s' "$n" ;;
    3) printf '3/%s/%s' "${n:0:1}" "$n" ;;
    *) printf '%s/%s/%s' "${n:0:2}" "${n:2:2}" "$n" ;;
  esac
}

# One crates.io request per crate per process, cached on disk. Without this a
# preflight asks the same question two or three times per crate, and against an
# API measured at ~4.7s per call from here the check takes ~37s.
# TTL, not per-process: keying the directory by PID meant every invocation
# re-fetched everything and a casual preflight still cost ~21s. Staleness is
# bounded by DEPLOY_CACHE_TTL (default 5 min) because a cached "absent" read
# after a publish is exactly the wrong thing to trust.
# Fixed name, not derived from $UA: that value contains a slash
# ("deploy-script/1.0"), which would silently nest the path into a directory
# that does not exist and make every cache write fail.
CRATE_CACHE_DIR="${TMPDIR:-/tmp}/deploy-http-cache"
CRATE_CACHE_TTL="${DEPLOY_CACHE_TTL:-300}"
mkdir -p "$CRATE_CACHE_DIR" 2>/dev/null || CRATE_CACHE_DIR=""
crate_json() { # <name> -> cached JSON body; non-zero if it could not be fetched
  local n="$1" f age
  if [ -z "$CRATE_CACHE_DIR" ]; then
    http "$CRATES_API/crates/$n" 2>/dev/null
    return $?
  fi
  f="$CRATE_CACHE_DIR/$n"
  if [ -f "$f" ]; then
    age=$(( $(date +%s) - $(stat -f %m "$f" 2>/dev/null || stat -c %Y "$f" 2>/dev/null || echo 0) ))
    [ "$age" -gt "$CRATE_CACHE_TTL" ] && rm -f "$f"
  fi
  # Retry, like index_probe does. crates.io answers non-200 for ordinary
  # reasons under load, and a single blip would otherwise make preflight report
  # "unreachable" and block a release that is perfectly fine.
  if [ ! -s "$f" ]; then
    local try
    for try in 1 2 3; do
      http "$CRATES_API/crates/$n" > "$f" 2>/dev/null
      [ -s "$f" ] && break
      sleep $(( try * 2 ))
    done
  fi
  [ -s "$f" ] || return 1
  # Accept either shape: a hit has .crate, a 404 has .errors. Anything else is
  # an HTML block page or a truncated body, which must not be parsed as truth.
  jq -e '(.crate.name // .errors) != null' "$f" >/dev/null 2>&1 || { rm -f "$f"; return 1; }
  cat "$f"
}
cache_clear() { rm -rf "${CRATE_CACHE_DIR:-/nonexistent-deploy-cache}" 2>/dev/null || true; }

# --- concurrency lock -------------------------------------------------------
# Two overlapping releases can both pass the "is 0.1.1 free?" check and then
# both try to publish it. Only one wins; the other burns time and leaves a
# half-finished tag. Cheap to prevent, so prevent it.
LOCK_DIR="${TMPDIR:-/tmp}/deploy-lock-$(basename "$PROJECT_ROOT")"
lock_acquire() {
  local attempt holder
  for attempt in 1 2; do
    if mkdir "$LOCK_DIR" 2>/dev/null; then
      printf '%s
' "$$" > "$LOCK_DIR/pid" 2>/dev/null || true
      trap 'lock_release' EXIT INT TERM
      return 0
    fi
    holder=$(cat "$LOCK_DIR/pid" 2>/dev/null || echo "")
    # Steal a lock whose owner is no longer alive, then retry once.
    if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
      warn "clearing stale lock from dead pid $holder"
      rm -rf "$LOCK_DIR"
      continue
    fi
    err "another release is running (pid ${holder:-unknown}); lock at $LOCK_DIR"
    err "if that process is really gone: rm -rf '$LOCK_DIR'"
    return 1
  done
  err "could not acquire lock after retry"
  return 1
}
lock_release() { rm -rf "$LOCK_DIR" 2>/dev/null || true; }

crates_io_exists() { # <name> -> true ONLY on a definitive "published"
  crate_json "$1" 2>/dev/null | jq -e '.crate.name != null' >/dev/null 2>&1
}

crates_io_latest() { # <name>
  crate_json "$1" 2>/dev/null | jq -r '.crate.newest_version // empty' 2>/dev/null
}

crates_io_versions() { # <name>
  crate_json "$1" 2>/dev/null | jq -r '.versions[].num' 2>/dev/null
}

# crates.io can rate-limit or flap, and treating any failure as "not published"
# is unsafe in both directions: it hands the LLM a false history, and it could
# let a burned version be reused. Distinguish "404, definitely absent" from
# "could not tell".
crate_status() { # <name> -> "published <latest>" | "absent" | "unknown"
  local j
  j=$(crate_json "$1") || { printf 'unknown'; return; }
  if printf '%s' "$j" | jq -e '.crate.name != null' >/dev/null 2>&1; then
    printf 'published %s' "$(printf '%s' "$j" | jq -r '.crate.newest_version // "?"')"
  elif printf '%s' "$j" | jq -e '.errors' >/dev/null 2>&1; then
    printf 'absent'
  else
    printf 'unknown'
  fi
}

# Three-state on purpose. A binary yes/no conflates "the index says it is not
# there" with "the request timed out or failed" — and index.crates.io has been
# observed stalling 35s on TLS here, which under a timeout would read as
# "not published" and let a burned version be reused.
#   0 = present   1 = definitively absent   2 = could not tell
index_probe() { # <name> <version>
  local body code rc attempt
  # index.crates.io has been seen returning 000 (connection failure) here while
  # working fine seconds later, so one retry before declaring "unknown".
  for attempt in 1 2; do
    body=$(http -w '
%{http_code}' "$SPARSE_INDEX/$(sparse_index_path "$1")" 2>/dev/null)
    rc=$?
    # Explicit rc capture: `body=$(...) || return 2` swallows the status of the
    # whole assignment expression and made a present version read as unknown.
    if [ "$rc" -eq 0 ]; then
      code=$(printf '%s' "$body" | tail -n1)
      case "$code" in
        200) : ;;
        404) return 1 ;;                  # no entry for the crate at all
        *)   [ "$attempt" = 2 ] && return 2; sleep 2; continue ;;
      esac
      if printf '%s' "$body" | sed '$d' | jq -r --arg v "$2" 'select(.vers==$v) | .vers' 2>/dev/null | grep -q .; then
        return 0
      fi
      return 1
    fi
    [ "$attempt" = 2 ] && return 2
    sleep 2
  done
  return 2
}

index_has_version() { # <name> <version> -> true only when definitively present
  index_probe "$1" "$2" = 0
}

index_is_known() { # <name> <version> -> "yes" | "no" | "unknown"
  index_probe "$1" "$2"
  case $? in
    0) printf 'yes' ;;
    1) printf 'no' ;;
    *) printf 'unknown' ;;
  esac
}

wait_for_index() { # <name> <version> [max-seconds]
  local n="$1" v="$2" max="${3:-180}" waited=0
  while [ "$waited" -lt "$max" ]; do
    index_probe "$n" "$v" && return 0
    sleep 5
    waited=$((waited + 5))
    debug "index: $n@$v not confirmed yet (${waited}s)"
  done
  return 1
}

# --- github -----------------------------------------------------------------
gh_ready() { have gh && gh auth status >/dev/null 2>&1; }
repo_is_public() {
  gh api "repos/$REPO_SLUG" --jq '.visibility' 2>/dev/null | grep -qi public
}

# --- semver -----------------------------------------------------------------
semver_split() {
  SV_MAJOR=$(printf '%s' "$1" | cut -d. -f1)
  SV_MINOR=$(printf '%s' "$1" | cut -d. -f2)
  SV_PATCH=$(printf '%s' "$1" | cut -d. -f3)
  SV_MAJOR=${SV_MAJOR:-0}; SV_MINOR=${SV_MINOR:-0}; SV_PATCH=${SV_PATCH:-0}
}

# cargo's semver model: for 0.MINOR.PATCH the MINOR digit is the breaking
# position, so PATCH is the safe increment. Inverted relative to 1.x.
semver_bump() { # <current> <patch|minor|major>
  semver_split "$1"
  case "$2" in
    patch) printf '%s.%s.%s\n' "$SV_MAJOR" "$SV_MINOR" "$((SV_PATCH + 1))" ;;
    minor) printf '%s.%s.0\n' "$SV_MAJOR" "$((SV_MINOR + 1))" ;;
    major) printf '%s.0.0\n' "$((SV_MAJOR + 1))" ;;
    *) return 1 ;;
  esac
}

semver_lte() { [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -n1)" = "$1" ]; }
