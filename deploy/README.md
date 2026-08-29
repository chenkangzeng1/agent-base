# Release automation

One script does the whole release: local gates → version decision → publish →
push → CI → tag → GitHub release → independent verification. It is deliberately
conservative, because publishing to crates.io cannot be undone.

```bash
./deploy/preflight.sh           # anytime: is this repo releasable right now?
./deploy/release.sh             # dry run: plan, gates, proposed version
./deploy/release.sh --yes       # do it, prompting at each irreversible step
```

**Dry run is the default.** Nothing is written, published, or pushed without
`--yes`.

## Files

| File | Purpose |
|------|---------|
| `config.sh` | **The only project-specific file.** Crates, repo slug, gates, timeouts. |
| `lib.sh` | Shared helpers: logging, state, Cargo.toml editing, crates.io/GitHub queries, semver. |
| `llm.sh` | Optional LLM advice. Never fatal, never on a critical path alone. |
| `preflight.sh` | Read-only readiness check. |
| `release.sh` | The orchestrator. |
| `verify.sh` | Builds a throwaway project against the published versions. |

## release.sh options

```
--yes                 execute (default is a rehearsal that changes nothing)
--dry-run             explicit rehearsal
--bump patch|minor|major   skip the LLM proposal and force this increment
--from <step>         resume after a failure, e.g. --from publish
--only <step>         run a single step
--auto                never prompt (requires --yes; use once proven)
--reset               discard recorded progress
--help                show this list
```

Steps, in order:

```
0 preflight  1 gates  2 version  3 commit  4 publish
5 push       6 ci     7 tag      8 release  9 verify
```

Rehearsals record no progress, so a dry run never makes a later real run skip
steps. `target_version()` prefers the value chosen in the current process, so a
rehearsal still prints the new number in steps 3–9 instead of the old one.

## Why publish happens before push

The default order is gates → version → commit → publish → push → CI → tag → release.

Two things were deliberately fixed along the way:

* **`commit` runs before `publish`.** Publishing from a dirty tree would let
  crates.io hold bytes that exist in no commit — unrecoverable if the run dies on
  the next line. Committing first makes every published artifact reproducible
  from pushed history.
* **The push step refuses a dirty tree for real runs**, since by then the crate
  may already be published and a mismatch would be silently permanent.

The remaining trade-off, stated plainly: if CI rejects the commit *after*
crates.io accepted the version, that version number is burned. A yank does not
free it, so the only recovery is releasing a new number. Local gates run first,
so this needs a CI-only failure — a flaky runner, or something nightly-specific
that stable does not catch.

To invert the trade-off, edit `STEPS` in `config.sh` to

```bash
STEPS=("preflight" "gates" "version" "commit" "push" "ci" "publish" "verify" "tag" "release")
```

Then a rejected commit never reaches the registry, at the cost of CI needing to
pass before anything is published.

## Confirmation points

`config.sh` sets `CONFIRM_MODE="interactive"` and lists the steps that always
ask, no matter what else is assumed:

| Step | Asks | Why |
|------|------|-----|
| `version` | "Publish v0.1.1 (patch)?" | the number is permanent |
| `publish` | `cargo publish <crate>@<ver>` | irreversible, one prompt per crate |
| `release` | approve the drafted notes | public artifact |

Reversible steps (fmt, clippy, tests, docs, fuzz build, local commit) never
block on a prompt, and `DEPLOY_ASSUME_YES=1` skips prompts for those while still
confirming the three above.

Once the flow is proven, switch to `CONFIRM_MODE="auto"` plus `--yes --auto`,
and the prompts disappear. Keep `--yes`: it is the difference between "I read
the plan" and "send it".

## The LLM's role

It advises; it never decides. Two places:

1. **Version proposal** (`version` step). It is given the currently published
   versions, the commit log since the last tag, and a diff of re-exported `pub`
   items. The prompt encodes cargo's inverted 0.x rule — for `0.MINOR.PATCH`,
   the **minor** digit is the breaking position, so an additive change is a
   *patch* bump. It answers `KIND` / `CONFIDENCE` / `WHY`, all three of which are
   printed before you are asked to approve. `low` confidence gets a warning line.
   The returned kind is re-validated against `patch|minor|major` before use, and
   an LLM that cannot answer falls back to conventional-commit rules, so no
   release depends on the model being reachable.
2. **Release notes** (`release` step): drafts from the commit list; you approve
   the text before it is created.

3. **CI failures**: `llm_explain_failure` summarises the failing log tail when
   one exists.

Set `LLM_REQUIRED=true` in `config.sh` if you would rather a missing LLM abort
the release than silently downgrade to the rule-based bump.

## Configuration

Credentials resolve in this order, first match wins:

```
DEPLOY_LLM_API_KEY   →  LLM_API_KEY
DEPLOY_LLM_BASE_URL  →  LLM_BASE_URL
DEPLOY_LLM_MODEL     →  LLM_MODEL
```

**Use `DEPLOY_LLM_*`.** `LLM_API_KEY`/`LLM_MODEL`/`LLM_BASE_URL`/`LLM_PROTOCOL`
are the environment contract of the library being shipped here — `LlmConfig::from_env()`
and `llm-cli` read exactly those names. If the deploy tool shares them, pointing
`llm-cli` at one provider and the release tool at another becomes impossible,
and a stray `cargo run --bin llm-cli` can talk to the wrong endpoint with the
wrong key.

```bash
# .env (git-ignored) or your shell
DEPLOY_LLM_API_KEY=sk-or-...
DEPLOY_LLM_BASE_URL=https://api.openai.com/v1
DEPLOY_LLM_MODEL=gpt-4o-mini
DEPLOY_LLM_STYLE=openai          # or: anthropic
```

`llm.sh` probes the channel lazily and every call is bounded by `LLM_TIMEOUT`.
`.env` is parsed rather than sourced, and variables already exported in the
shell win over it — the reverse would let a stale file silently override a live
key.

## Registry checks are three-state

`index_probe` returns *present*, *absent*, or *unknown*, and unknown is treated
as "stop", never as "safe to publish". This is not paranoia: index.crates.io was
observed stalling ~35s on TLS from this machine, which under a timeout reads as
"not published" — and publishing an already-burned version is unrecoverable.

Responses are cached in `$TMPDIR/deploy-http-cache` for `DEPLOY_CACHE_TTL`
seconds (default 300) because each crates.io call measures ~4.7s from here. The
cache is cleared on `publish` and on exit. `./deploy/preflight.sh --clear-cache`
forces a fresh look.

Both registries are overridable, which also makes the failure paths testable
without waiting for an outage:

```bash
DEPLOY_CRATES_API=https://crates.io/api/v1    # or a mirror
DEPLOY_SPARSE_INDEX=https://index.crates.io
DEPLOY_CACHE_TTL=300
DEPLOY_UA="deploy-script/1.0 (+https://github.com/you/repo)"   # crates.io prefers contact info
```

Measured cost of a preflight: ~10s cold, ~7s warm. Pointing `DEPLOY_CRATES_API`
at an unreachable host takes ~29s and blocks the release, which is the intended
behaviour.

## Porting to another project

Copy the whole `deploy/` directory, then rewrite **only `config.sh`**:

- `PROJECT_NAME`, `REPO_SLUG`, `DEFAULT_BRANCH`
- `CRATES` — `"name:path/to/Cargo.toml"`, **in publish order**
- `VERSION_LOCKSTEP`, `INTERNAL_DEPS` — `"<toml>|<dep>"` for path deps
- `MSRV`, gate toggles, `CI_WORKFLOWS`, step order

Keep `lib.sh` and `llm.sh` byte-identical across projects so a bug fixed in one
is fixed in all; `config.sh` is where the projects differ. `diff` the two
`config.sh` files to review a port in one screen.

### Then add the generated dirs to .gitignore

Append to the target repo's `.gitignore`:

```gitignore
deploy/logs/
deploy/.state/
```

This is not cosmetic. A crate that declares no `include`/`exclude` list in
`Cargo.toml` packages according to `.gitignore`, so `cargo package` will put
`deploy/logs/*.log` — every run log, including local paths — inside the
published tarball. That is unrecoverable once the version ships. Check the
result before releasing:

```bash
cargo package --list --allow-dirty --no-verify | grep -E "logs|\.state"
```

Prints nothing when correct. `git add deploy` copies whatever logs happen to
exist at that moment, so do this *before* the first commit, or run
`git rm -r --cached deploy/logs` afterwards — `.gitignore` has no effect on
files that are already tracked.

### The `INTERNAL_DEPS` list is the one that bites

`cargo publish` refuses a path dependency with no version requirement:

```
error: failed to verify manifest
all dependencies must have a version requirement specified when publishing.
dependency `agent-types` does not specify a version
```

So a workspace that has only ever been built locally will fail here. For each
entry, either give it `version = "x.y.z"` or drop it from the list. The
dependency being pointed at must already be on crates.io.

## Local state

`deploy/.state/` and `deploy/logs/` are generated and git-ignored. Delete
`.state` to re-run steps; `--reset` does it for you.
