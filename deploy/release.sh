#!/usr/bin/env bash
# ============================================================================
# deploy/release.sh — orchestrator for the whole release.
#
#   ./deploy/release.sh                 dry-run: plans, gates, proposes version
#   ./deploy/release.sh --yes           execute everything, prompting on
#                                       irreversible steps (publish/release)
#   ./deploy/release.sh --bump patch    skip the LLM proposal, use this bump
#   ./deploy/release.sh --from publish  resume after an interruption
#   ./deploy/release.sh --only gates    run one step
#   ./deploy/release.sh --reset         forget recorded progress
#   ./deploy/release.sh --auto          no prompts (needs --yes; use only once
#                                       the flow is proven)
#
# Safety properties, deliberately:
#   * dry-run is the DEFAULT. Nothing is written or published without --yes.
#   * A dirty working tree aborts before anything else. The commit that gets
#     published must be exactly the commit tagged and the commit CI tested.
#   * Every irreversible step re-checks crates.io. Versions can never be
#     reclaimed there, so "already published" is a hard stop, not a warning.
#   * The LLM proposes a version bump; a human confirms it. The proposal never
#     reaches cargo without that confirmation.
#   * Each step records completion in deploy/.state, so an interrupted release
#     resumes instead of restarting and re-publishing.
# ============================================================================
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
. "$LIB_DIR/llm.sh"

DRY=1; FROM=""; ONLY=""; FORCED_BUMP=""; DONE_ANY=""
while [ $# -gt 0 ]; do
  case "$1" in
    --yes)    DRY=0 ;;
    --dry-run) DRY=1 ;;
    --from)   FROM="$2"; shift ;;
    --only)   ONLY="$2"; shift ;;
    --bump)   FORCED_BUMP="$2"; shift ;;
    --auto)   CONFIRM_MODE="auto" ;;
    --reset)  state_clear; info "state cleared"; exit 0 ;;
    -h|--help) sed -n '2,26p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
  shift
done

[ "$CONFIRM_MODE" = "auto" ] && [ "$DRY" = "1" ] \
  && die "--auto does nothing without --yes"

# A misspelled --from/--only silently matches no step and the release appears to
# succeed having done nothing at all. Validate against STEPS up front.
valid_step() {
  local want="$1" s
  for s in "${STEPS[@]}"; do [ "$s" = "$want" ] && return 0; done
  return 1
}
[ -n "$FROM" ] && { valid_step "$FROM" || die "--from '$FROM' is not a step. Valid: ${STEPS[*]}"; }
[ -n "$ONLY" ] && { valid_step "$ONLY" || die "--only '$ONLY' is not a step. Valid: ${STEPS[*]}"; }
if [ -n "$FORCED_BUMP" ]; then
  case "$FORCED_BUMP" in
    patch|minor|major) ;;
    *) die "--bump must be patch|minor|major, got '$FORCED_BUMP'" ;;
  esac
fi

# Lets confirm() skip prompting: a dry run has nothing to approve, and asking
# "Publish v0.1.1?" when no publish will happen trains you to answer yes.
[ "$DRY" = "1" ] && export DEPLOY_DRY=1

step_wanted() {
  [ -z "$ONLY" ] || { [ "$ONLY" = "$1" ] || return 1; }
  if [ -n "$FROM" ]; then
    # Note the `found` variable. Writing `if (...) exit 0 ... END { exit 1 }`
    # looks equivalent but is not: in awk a bare `exit` still runs END, and
    # END's exit status wins. That version always returned 1, so `--from <step>`
    # silently skipped every step including the named one.
    awk -v from="$FROM" -v cur="$1" '
      { if ($0 == from) p = 1; if (p == 1 && $0 == cur) { found = 1; exit } }
      END { exit(found ? 0 : 1) }
    ' <(printf '%s\n' "${STEPS[@]}") || return 1
  fi
  ! state_done "$1"
}

# gate_action <step> <irreversible?>  — decides whether to really run
gate_action() {
  local step="$1" irre="${2:-}"
  if [ "$DRY" = "1" ]; then
    info "[dry-run] would: $step"
    return 1
  fi
  if [ -n "$irre" ] && ! confirm "Execute '$step' — this cannot be undone" yes; then
    abort_or_skip "$step"
  fi
  return 0
}

# The version being released. Prefer the value chosen in this process, then the
# recorded state, then the manifest — that order keeps a rehearsal truthful, since
# dry runs deliberately write no state and would otherwise show the *old* number
# in later step messages.
PROPOSED_VERSION=""
target_version() {
  local v
  v="${PROPOSED_VERSION:-}"
  [ -n "$v" ] || v=$(state_read "version")
  [ -n "$v" ] || v=$(manifest_version "$(crate_path_of "${CRATES[0]}")")
  printf '%s' "$v"
}

# ---------------------------------------------------------------------------
# 0. preflight
# ---------------------------------------------------------------------------
step_preflight() {
  section "0/10 preflight"
  if ! "$LIB_DIR/preflight.sh"; then
    die "preflight failed — fix the blockers above, or run deploy/preflight.sh to see them"
  fi
  git_ensure_clean
  ok "preflight"
}

# ---------------------------------------------------------------------------
# 1. local gates
# ---------------------------------------------------------------------------
step_gates() {
  section "1/10 local gates"
  need cargo rustc
  run "rustfmt"         cargo fmt --all --check                  || return 1
  run "clippy"          cargo clippy --workspace --all-targets -- -D warnings || return 1
  run "tests"           cargo test --workspace                   || return 1
  if [ "$RUN_DOC_GATE" = "true" ]; then
    run "rustdoc"       env RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace || return 1
  fi
  if [ "$RUN_FUZZ_BUILD" = "true" ] && [ -d fuzz ]; then
    if rustup toolchain list 2>/dev/null | grep -q nightly; then
      ( cd fuzz && run "fuzz build" cargo +nightly fuzz build ) || return 1
    else
      warn "fuzz/ present but no nightly toolchain; skipping fuzz build"
    fi
  fi
  state_write "gates.commit" "$(git rev-parse HEAD)"
  ok "all local gates passed"
}

# ---------------------------------------------------------------------------
# 2. version — LLM proposes, human confirms
# ---------------------------------------------------------------------------
collect_published() {
  local out="" entry n v st
  for entry in "${CRATES[@]}"; do
    n=$(crate_name_of "$entry"); v=$(manifest_version "$(crate_path_of "$entry")")
    st=$(crate_status "$n")
    case "$st" in
      published*)
        out="$out$n crates.io=$(printf '%s' "${st#published }") already_published=[$(crates_io_versions "$n" | tr '\n' ' ')] manifest=$v
" ;;
      absent)
        out="$out$n crates.io=absent (never published) manifest=$v
" ;;
      *)
        # Loud on purpose: an unreachable registry must not read as "safe to publish".
        out="$out$n crates.io=UNKNOWN (query failed; treat as possibly published) manifest=$v
" ;;
    esac
  done
  printf '%s' "$out"
}

api_surface_diff() { # <tag>
  # Diff the re-exported public items between the last tag and HEAD.
  local tag="$1" f out="" d
  for f in src/lib.rs llm-trait/src/lib.rs; do
    [ -f "$f" ] || continue
    if [ -n "$tag" ]; then
      d=$(git diff "$tag"..HEAD -- "$f" | grep -E '^[+-][[:space:]]*pub ' | head -40)
    else
      d=$(grep -E '^[[:space:]]*pub ' "$f" | head -40)
    fi
    [ -n "$d" ] && out="$out
--- $f ---
$d"
  done
  printf '%s' "${out:-(no public re-export changes detected)}"
}

step_version() {
  section "2/10 version"
  local cur tag log published apidiff kind why conf proposed llm_src

  cur=$(manifest_version "$(crate_path_of "${CRATES[0]}")")
  tag=$(git tag -l 'v*' --sort=-v:refname | head -1)
  if [ -n "$tag" ]; then
    log=$(git log "$tag"..HEAD --no-merges --pretty='%s%n%b' | head -160)
  else
    log=$(git log --no-merges --pretty='%s%n%b' | head -160)
  fi
  [ -n "$log" ] || warn "no commits since ${tag:-the beginning}; nothing may need publishing"
  published=$(collect_published)
  apidiff=$(api_surface_diff "$tag")

  info "current version : $cur"
  info "last tag        : ${tag:-none}"
  # $() strips the trailing newline, so re-add it before piping to sed.
  printf '%s\n' "$published" | sed 's/^/  crates.io       /'

  kind="$FORCED_BUMP"
  if [ -z "$kind" ]; then
    if llm_propose_bump "$cur" "$published" "$log" "$apidiff"; then
      kind="$LLM_BUMP_KIND"; why="$LLM_BUMP_WHY"; conf="$LLM_BUMP_CONF"; llm_src="LLM"
    else
      kind=$(bump_from_commits "$log")
      why="deterministic conventional-commit rule (LLM unavailable)"
      conf="rule"; llm_src="rule"
    fi
    # An LLM answer is a suggestion, never a value handed straight to cargo.
    case "$kind" in patch|minor|major) ;; *) die "invalid bump kind '$kind'" ;; esac
    proposed=$(semver_bump "$cur" "$kind") || die "could not compute bump for '$kind'"
    echo
    info "proposed bump   : $kind  ->  $proposed   (source: $llm_src)"
    info "reasoning       : $why"
    [ -n "$conf" ] && info "confidence      : $conf"
    [ "$conf" = "low" ] && note "low confidence — read the commits before approving"
    printf '%s\n' "$log" | head -12 | sed 's/^/    │ /'
    echo
    confirm "Publish $PROJECT_NAME v$proposed ($kind)" yes \
      || die "release aborted at the version gate"
  else
    proposed=$(semver_bump "$cur" "$kind") || die "bad --bump value: $kind"
    info "--bump $kind -> $proposed (override in effect, LLM not consulted)"
    confirm "Publish $PROJECT_NAME v$proposed ($kind)" yes || die "aborted"
  fi
  PROPOSED_VERSION="$proposed"

  gate_action "write version $proposed into the manifests" || return 0
  local entry p spec f dep
  for entry in "${CRATES[@]}"; do
    p=$(crate_path_of "$entry")
    manifest_set_version "$p" "$proposed" || die "could not set version in $p"
    ok "$(crate_name_of "$entry") -> $proposed ($p)"
  done
  # Internal path deps must carry a matching version or cargo refuses to publish.
  if [ "${#INTERNAL_DEPS[@]}" -gt 0 ]; then
    for spec in "${INTERNAL_DEPS[@]}"; do
      f="${spec%%|*}"; dep="$(printf '%s' "$spec" | cut -d'|' -f2)"
      manifest_set_dep_version "$f" "$dep" "$proposed" || die "could not pin $dep in $f"
      ok "$f: $dep version = \"$proposed\""
    done
  fi
  cargo update --workspace >/dev/null 2>&1 || warn "cargo update failed (offline?)"
  state_write "version" "$proposed"
  state_write "bump_kind" "$kind"
  ok "version set to $proposed"
}

# Deterministic fallback when no LLM is configured.
bump_from_commits() { # <log>
  local log="$1"
  if printf '%s' "$log" | grep -qiE '^[a-z]+(\([^)]*\))?!:|BREAKING CHANGE'; then
    printf 'minor'   # in 0.x the minor digit is the breaking position
  elif printf '%s' "$log" | grep -qiE '^[a-z]+(\([^)]*\))?: (feat|fix|perf|docs|test|refactor)'; then
    printf 'patch'
  else
    printf 'patch'
  fi
}

# ---------------------------------------------------------------------------
# 3. publish to crates.io
# ---------------------------------------------------------------------------
step_publish() {
  section "4/10 publish crates.io"
  local ver; ver=$(target_version)
  info "target version: $ver"

  # A registry we cannot reach must stop us, not wave us through: publishing a
  # number that already exists is unrecoverable, since crates.io never allows a
  # version to be reused even after yank.
  local entry n p st
  for entry in "${CRATES[@]}"; do
    n=$(crate_name_of "$entry")
    st=$(crate_status "$n")
    case "$st" in
      unknown) die "cannot determine whether $n is published (registry unreachable). Refusing to guess. Retry shortly." ;;
    esac
  done

  for entry in "${CRATES[@]}"; do
    n=$(crate_name_of "$entry"); p=$(crate_path_of "$entry")
    if crates_io_exists "$n" && index_has_version "$n" "$ver"; then
      ok "$n@$ver already in the index; skipping (never re-publish)"
      continue
    fi
    if ! gate_action "cargo publish $n@$ver" yes; then continue; fi
    # The cache must not survive a publish: a warm entry still saying
    # "published 0.1.0" would make the confirmation loop below re-check stale
    # data, and preflight could then tell you a fresh version is free to take.
    cache_clear
    run "dry-run $n" cargo publish -p "$n" --dry-run --allow-dirty || return 1
    run "publish $n" cargo publish -p "$n" --allow-dirty || return 1
    cache_clear
    info "waiting for $n@$ver to appear in the sparse index..."
    if wait_for_index "$n" "$ver" 240; then
      ok "$n@$ver confirmed in the index"
    else
      die "$n@$ver uploaded but not visible in the index after 240s.
     The version is ALREADY BURNED — do not reuse it.
     Check https://crates.io/crates/$n, then resume with --from push"
    fi
  done
  ok "publish step complete"
}

# ---------------------------------------------------------------------------
# 4. commit the version bump
# ---------------------------------------------------------------------------
step_commit() {
  section "3/10 commit"
  local ver entry; ver=$(target_version)
  if git_clean; then
    ok "nothing to commit (version step was a rehearsal or already committed)"
    return 0
  fi
  if ! gate_action "commit the version bump for v$ver"; then
    git status --short | sed 's/^/    would commit: /'
    return 0
  fi
  # Stage only what a release commit should contain. `git add -A` would sweep in
  # anything the repo forgot to ignore, and this commit is about to be the one
  # the published crate must reproduce.
  local paths=""
  for entry in "${CRATES[@]}"; do paths="$paths $(crate_path_of "$entry")"; done
  [ -f Cargo.lock ] && paths="$paths Cargo.lock"
  paths="$paths CHANGELOG.md"
  # shellcheck disable=SC2086
  run "git add" git add $paths || return 1
  run "commit" git commit -q -m "chore(release): v$ver" || return 1
  git_clean || { err "release commit left changes staged or untracked"; git status --short | sed 's/^/  /' >&2; return 1; }
  state_write "release.sha" "$(git rev-parse HEAD)"
  ok "committed $(git rev-parse --short HEAD) v$ver"
}

# ---------------------------------------------------------------------------
# 5. push
# ---------------------------------------------------------------------------
step_push() {
  section "5/10 push"
  local ver; ver=$(target_version)
  # By now the crate may already be on crates.io. A dirty tree here means the
  # published bytes are not in any commit, so say so loudly rather than
  # continuing. Not applicable to a rehearsal, which commits nothing.
  if [ -z "${DEPLOY_DRY:-}" ] && ! git_clean; then
    err "tree is dirty at push time — the published version would not match any commit"
    git status --short | sed 's/^/      /' >&2
    die "push manually after inspecting, then resume with --from ci"
  fi
  gate_action "push v$ver to origin/$DEFAULT_BRANCH" || return 0
  run "push" git push origin "$DEFAULT_BRANCH" || return 1
  local local_sha remote_sha
  local_sha=$(git rev-parse HEAD)
  remote_sha=$(git ls-remote origin "refs/heads/$DEFAULT_BRANCH" | awk '{print $1}')
  [ "$local_sha" = "$remote_sha" ] || die "push did not land: local=$local_sha remote=$remote_sha"
  state_write "pushed.sha" "$local_sha"
  ok "origin/$DEFAULT_BRANCH == $local_sha"
}

# ---------------------------------------------------------------------------
# 5. CI
# ---------------------------------------------------------------------------
step_ci() {
  section "6/10 CI"
  if ! gh_ready; then warn "gh unavailable; cannot verify CI"; return 0; fi
  gate_action "wait for CI" || return 0
  local sha; sha=$(git rev-parse HEAD)
  local waited=0 run_id status conclusion
  info "watching CI for $sha"
  while [ "$waited" -lt "$CI_TIMEOUT_SECONDS" ]; do
    run_id=$(gh api "repos/$REPO_SLUG/commits/$sha/check-runs" \
      --jq '.check_runs[0].run_id // empty' 2>/dev/null || true)
    if [ -n "$run_id" ]; then
      status=$(gh api "repos/$REPO_SLUG/actions/runs/$run_id" --jq .status 2>/dev/null || echo "")
      if [ "$status" = "completed" ]; then
        conclusion=$(gh api "repos/$REPO_SLUG/actions/runs/$run_id" --jq .conclusion 2>/dev/null)
        if [ "$conclusion" = "success" ]; then
          ok "CI run $run_id succeeded"
          return 0
        fi
        err "CI run $run_id concluded: $conclusion"
        ci_explain "$run_id"
        return 1
      fi
      debug "CI $run_id status=$status (${waited}s)"
    else
      debug "no CI run visible yet for $sha"
    fi
    sleep "$CI_POLL_SECONDS"; waited=$((waited + CI_POLL_SECONDS))
  done
  die "CI did not finish within ${CI_TIMEOUT_SECONDS}s"
}

ci_explain() { # <run_id>
  local rid="$1" tail
  tail=$(gh run view "$rid" --log-failed 2>/dev/null | tail -60 || true)
  [ -n "$tail" ] || return 0
  if llm_explain_failure "$tail"; then
    :
  else
    info "failing log tail:"
    printf '%s\n' "$tail" | tail -12 | sed 's/^/    │ /'
  fi
}

# ---------------------------------------------------------------------------
# 6/7. tag + release
# ---------------------------------------------------------------------------
step_tag() {
  section "7/10 tag"
  local ver; ver=$(target_version)
  local tag="v$ver"
  if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    ok "tag $tag already exists locally; skipping"
    return 0
  fi
  gate_action "create and push tag $tag" || return 0
  git_ensure_clean
  run "annotate $tag" git tag -a "$tag" -m "v$tag
$(head_subject)" || return 1
  run "push tag" git push origin "$tag" || return 1
  state_write "tag" "$tag"
  ok "tagged $tag"
}

step_release() {
  section "8/10 GitHub release"
  local ver tag notes
  tag=$(state_read "tag"); ver="${tag#v}"
  if ! gh_ready; then warn "gh unavailable; skipping release"; return 0; fi
  if gh release view "$tag" >/dev/null 2>&1; then
    ok "release $tag already exists; skipping"; return 0
  fi
  gate_action "create GitHub release $tag" || return 0
  notes=$(release_notes "$ver" "$tag")
  printf '\n%s\n' "──── proposed release notes ────"
  printf '%s\n' "$notes" | sed 's/^/  /'
  printf '%s\n' "─────────────────────────────────"
  if ! confirm "Create this release" yes; then
    warn "release not created; tag exists. Re-run with --only release."
    return 0
  fi
  printf '%s\n' "$notes" > /tmp/notes.$$ 
  run "gh release create" gh release create "$tag" \
      --title "$PROJECT_NAME $ver" --notes-file "/tmp/notes.$$" || { rm -f /tmp/notes.$$; return 1; }
  rm -f /tmp/notes.$$
  ok "release $tag created"
}

release_notes() { # <ver> <tag>
  local ver="$1" tag="$2" prev log base
  base=$(git tag -l 'v*' --sort=-v:refname | grep -v "^$tag$" | head -1)
  if [ -n "$base" ]; then log=$(git log "$base".."$tag" --no-merges --pretty='- %s')
  else log=$(git log "$tag" --no-merges --pretty='- %s' | head -60); fi
  if llm_draft_release_notes "$ver" "$log"; then return 0; fi
  printf '## %s\n\n%s\n' "$ver" "${log:-(no commits)}"
}

# ---------------------------------------------------------------------------
# 8. end-to-end verification against what the public actually sees
# ---------------------------------------------------------------------------
step_verify() {
  section "9/10 verify install from crates.io"
  "$LIB_DIR/verify.sh" || return 1
}

# ---------------------------------------------------------------------------
main() {
  need git cargo curl jq awk sed grep
  state_init
  # One combined trap. `trap A EXIT; trap B EXIT` replaces A — only B would run
  # — so the lock release and the cache clear have to live in the same handler
  # or the lock is left behind for the next run to have to steal.
  trap 'lock_release; cache_clear' EXIT INT TERM
  # Refuse to start a second release while one is in progress: two runs can both
  # see the same free version and race to publish it.
  lock_acquire || exit 1
  info "$PROJECT_NAME release"
  [ "$DRY" = "1" ] && warn "DRY RUN — nothing will be written or published. Use --yes to execute."
  info "steps: ${STEPS[*]}"
  [ -n "$FROM" ] && info "resuming from: $FROM"
  [ -n "$ONLY" ] && info "only: $ONLY"

  local s
  for s in "${STEPS[@]}"; do
    DONE_ANY="$DONE_ANY $s"
    step_wanted "$s" || { debug "skip $s"; continue; }
    "step_$s" || {
      [ "$s" = "gates" ] && warn "gates failed — nothing has been published, safe to fix and re-run"
      die "step '$s' failed. Progress is in $STATE_DIR; resume with --from $s"
    }
    # A dry run must leave no progress behind. Marking steps complete during a
    # rehearsal made the next real run skip them — including `version`, whose
    # result the publish/tag/release steps read back from state.
    if [ "$DRY" != "1" ]; then state_mark "$s"; fi
  done

  section "done"
  if [ "$DRY" = "1" ]; then
    ok "dry run complete — nothing was changed. Re-run with --yes."
  else
    ok "release finished. Verify at https://crates.io and https://github.com/$REPO_SLUG/releases"
  fi
  info "log: $RUN_LOG"
}
main "$@"
