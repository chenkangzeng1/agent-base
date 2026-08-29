#!/usr/bin/env bash
# ============================================================================
# deploy/llm.sh — optional LLM assistance.
#
# Contract:
#   * NEVER fatal. If no key/endpoint is configured or the call fails, returns
#     non-zero so the caller falls back to deterministic rules.
#   * NEVER echoes the API key.
#   * Its output is a PROPOSAL for a human to confirm; it is never wired
#     directly into an irreversible action.
#
# Reads DEPLOY_LLM_* first. Falls back to LLM_* (the variable names our own
# library uses) only when LLM_FALLBACK_VARS=true.
# ============================================================================

# shellcheck source=/dev/null
[ -n "${LIB_DIR:-}" ] || { . "$(dirname "${BASH_SOURCE[0]}")/lib.sh"; }

llm_resolve() {
  LLM_KEY="${DEPLOY_LLM_API_KEY:-}"
  LLM_URL="${DEPLOY_LLM_BASE_URL:-}"
  LLM_MODEL_ID="${DEPLOY_LLM_MODEL:-}"
  LLM_STYLE="${DEPLOY_LLM_STYLE:-openai}"   # openai | anthropic

  if [ "${LLM_FALLBACK_VARS:-true}" = "true" ]; then
    [ -n "$LLM_KEY" ]      || LLM_KEY="${LLM_API_KEY:-}"
    [ -n "$LLM_URL" ]      || LLM_URL="${LLM_BASE_URL:-}"
    [ -n "$LLM_MODEL_ID" ] || LLM_MODEL_ID="${LLM_MODEL:-${LLM_MODEL_FALLBACK:-}}"
  fi
  [ -n "${LLM_KEY:-}" ] || return 1

  # A key that our own library would reject is worse than no key at all: it
  # makes a broken release path look configured. Validate the shape only, never
  # the value, and never print it.
  case "$LLM_KEY" in
    ""|*xxxxXXX*|*XXXX*|*"xxxxxxxx"*) debug "llm: key looks like a placeholder"; return 1 ;;
  esac
  [ -n "${LLM_URL:-}" ] || return 1
  return 0
}

llm_available() { llm_resolve >/dev/null 2>&1; }

# llm_chat <system> <user-text>  -> assistant text on stdout
llm_chat() {
  llm_resolve || { debug "llm: unavailable, skipping"; return 1; }
  local sys="$1" user="$2" body resp url
  local timeout="${LLM_TIMEOUT:-90}"

  if [ "$LLM_STYLE" = "anthropic" ]; then
    url="${LLM_URL%/}/messages"
    body=$(jq -n --arg m "$LLM_MODEL_ID" --arg s "$sys" --arg u "$user" \
      '{model:$m, max_tokens:1024, system:$s, messages:[{role:"user",content:$u}]}')
    resp=$(curl -s --max-time "$timeout" "$url" \
      -H "x-api-key: $LLM_KEY" -H "anthropic-version: 2023-06-01" \
      -H "content-type: application/json" -d "$body" 2>/dev/null)
    printf '%s' "$resp" | jq -r '[.content[]?|select(.type=="text")|.text]|join("\n")' 2>/dev/null | grep . || {
      printf '%s' "$resp" | jq -r '.error.message // empty' 2>/dev/null | head -1 \
        | sed 's/^/      llm: /' >&2
      return 1; }
  else
    url="${LLM_URL%/}/chat/completions"
    body=$(jq -n --arg m "$LLM_MODEL_ID" --arg s "$sys" --arg u "$user" \
      '{model:$m, max_tokens:1024, temperature:0, messages:[{role:"system",content:$s},{role:"user",content:$u}]}')
    resp=$(curl -s --max-time "$timeout" "$url" \
      -H "Authorization: Bearer $LLM_KEY" -H "content-type: application/json" \
      -d "$body" 2>/dev/null)
    # Surface provider errors (an expired key reads exactly like "no answer").
    if ! printf '%s' "$resp" | jq -e '.choices[0].message.content' >/dev/null 2>&1; then
      printf '%s' "$resp" | jq -r '.error.message // .message // empty' 2>/dev/null | head -1 \
        | sed 's/^/      llm: /' >&2
      return 1
    fi
    printf '%s' "$resp" | jq -r '.choices[0].message.content' 2>/dev/null | grep . || return 1
  fi
}

# --- the one decision we want help with: which version bump? ---------------
# Returns via globals: LLM_BUMP_KIND patch|minor|major, LLM_BUMP_WHY, LLM_BUMP_CONF
llm_propose_bump() { # <current> <published-list> <git-log> <api-diff>
  local cur="$1" published="$2" log="$3" diff="$4" out
  llm_available || return 1

  out=$(llm_chat \
"You advise on semantic version bumps for Rust crates published to crates.io.
Rules, in cargo's own semver model:
  - Version 0.MINOR.PATCH: the MINOR digit is the BREAKING position and PATCH is
    the safe additive position. This is inverted relative to 1.x and most people
    get it wrong. So for 0.1.0: additive new methods -> 0.1.1; removing or
    changing a public type/field/method signature -> 0.2.0.
  - Version 1.x and up: MAJOR breaking, MINOR additive, PATCH fixes.
  - A version already present on crates.io can never be reused, even after yank.
  - When genuinely unsure between two safe options, choose the smaller increment.
Reply in EXACTLY three lines, no prose, no markdown fences:
KIND: <patch|minor|major>
CONFIDENCE: <high|medium|low>
WHY: <one sentence naming the concrete change that decides it>" \
"Current version in the manifest: $cur
Already published on crates.io: $published

Commits since the last published tag (or repo start):
$log

Public API surface changes detected by diffing the re-exported items:
$diff

Recommend the next version bump.") || return 1

  LLM_BUMP_KIND=$(printf '%s' "$out" | sed -n 's/^KIND:[[:space:]]*//p' | tr -d '[:space:]' | tr 'A-Z' 'a-z')
  LLM_BUMP_CONF=$(printf '%s' "$out" | sed -n 's/^CONFIDENCE:[[:space:]]*//p' | tr -d '[:space:]' | tr 'A-Z' 'a-z')
  LLM_BUMP_WHY=$(printf '%s' "$out" | sed -n 's/^WHY:[[:space:]]*//p')

  case "$LLM_BUMP_KIND" in patch|minor|major) ;; *) return 1 ;; esac
  [ -n "$LLM_BUMP_WHY" ] || LLM_BUMP_WHY="(no reason given)"
  return 0
}

llm_draft_release_notes() { # <version> <git-log>
  llm_available || return 1
  llm_chat \
"You write release notes for a small open-source Rust crate. Be concrete and
terse. Group into Added / Changed / Fixed / Removed. Name the actual type or
method involved, never 'various improvements'. If a change breaks callers, say
so explicitly and show the before/after shape in one line of Rust. Mention
nothing that is not evidenced by the commits. Output markdown only, no H1, no
fences around the whole thing." \
"Release version: $1
Commits since the previous release:
$2" || return 1
}

# --- advisory: explain a CI failure ----------------------------------------
llm_explain_failure() { # <log-tail>
  llm_available || return 1
  llm_chat \
"You debug GitHub Actions for Rust projects. From the log excerpt, state in at
most 3 short bullets: what failed, the most likely cause, and the single next
command a maintainer should run locally to confirm it. Say 'unknown' rather than
inventing a cause. No preamble." "$1" || return 1
}
