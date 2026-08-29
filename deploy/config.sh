#!/usr/bin/env bash
# ============================================================================
# deploy/config.sh — the ONLY file that differs between projects.
#
# Everything else in deploy/ (lib.sh, llm.sh, preflight.sh, release.sh,
# verify.sh, README.md) is copied byte-identical from llm-providers/deploy/
# so a bug fixed there is fixed here too. Check with:
#   for f in lib.sh llm.sh preflight.sh release.sh verify.sh; do
#     cmp ../llm-providers/deploy/$f deploy/$f || echo "DRIFT: $f"
#   done
# ============================================================================

# --- identity ---------------------------------------------------------------
PROJECT_NAME="agent-base"
REPO_SLUG="hibuka-labs/agent-base"
DEFAULT_BRANCH="master"

# --- crates, in publish order ----------------------------------------------
# agent-types must go first: agent-base depends on it, and cargo cannot resolve
# a version requirement for something that is not on the registry yet.
# Format: "<crate-name>:<path-to-its-Cargo.toml>"
CRATES=(
  "agent-types:crates/agent-types/Cargo.toml"
  "agent-base:Cargo.toml"
)

# Both crates version together. If you ever release agent-types on its own
# schedule, set this false — otherwise the lockstep check becomes noise.
VERSION_LOCKSTEP=false

# Path deps that must carry a `version` requirement, or cargo publish refuses:
#   "dependent-toml|dependency-name"
#
# CURRENT STATE — this repo cannot be published until these are satisfied:
#   agent-types       never published; must be published first (it is CRATES[0])
#   llm-trait         0.1.0 on crates.io   -> add version = "0.1"
#   llm-unified       0.1.0 on crates.io   -> add version = "0.1"
#   anthropic-rs-api  its ONLY version (0.1.0) is YANKED, and a yanked version
#                     cannot be expressed as a normal requirement. Either the
#                     upstream ships an unyanked release or this dependency has
#                     to be vendored, replaced, or dropped.
# preflight.sh reports all of these; run it before planning a release.
INTERNAL_DEPS=(
  "Cargo.toml|agent-types"
  "Cargo.toml|llm-trait"
  "Cargo.toml|llm-unified"
  "Cargo.toml|anthropic-rs-api"
  "crates/agent-types/Cargo.toml|llm-trait"
)

# --- gates ------------------------------------------------------------------
# agent-base declares no `rust-version`, so preflight reports MSRV as skipped
# rather than inventing one. Declare it in Cargo.toml to get the gate.
MSRV=""
RUN_DOC_GATE=true
# fuzz/ exists but is NOT a workspace member, so `cargo build --workspace` never
# compiles it. The gate catches drift in the harnesses that CI would miss.
RUN_FUZZ_BUILD=true
FUZZ_SECONDS_ON_RELEASE=0

# --- CI ---------------------------------------------------------------------
# Only a CI workflow here; there is no fuzz workflow like llm-providers has.
CI_WORKFLOWS=("CI")
CI_POLL_SECONDS=20
CI_TIMEOUT_SECONDS=1800

# --- LLM (optional) --------------------------------------------------------
# Uses DEPLOY_LLM_* first on purpose: LLM_API_KEY / LLM_MODEL / LLM_BASE_URL /
# LLM_PROTOCOL are the environment contract of agent-base itself (and of the
# llm-* crates it consumes), so sharing those names with release tooling makes
# an accidental `cargo run` talk to the wrong provider.
LLM_FALLBACK_VARS=true
LLM_MODEL_FALLBACK=""
LLM_TIMEOUT=90
LLM_REQUIRED=false

# --- confirmation policy ---------------------------------------------------
#   interactive  : ask before each irreversible action (default, safest)
#   auto         : no prompts; requires --yes to do anything irreversible
CONFIRM_MODE="interactive"

IRREVERSIBLE_STEPS=("version" "publish" "release")

# --- step order ------------------------------------------------------------
# Iterated in this sequence by release.sh.
#
#   preflight  checks tools/auth/tree cleanliness, never writes anything
#   gates      fmt, clippy, tests, docs, (fuzz build)
#   version    LLM proposes the bump from online versions + git log; you confirm
#   commit     commit the bump locally, before anything is published
#   publish    cargo publish, sequentially, then confirm crates.io agrees
#   push       push the commit that was published
#   ci         wait for the workflows to go green
#   tag        create and push v<version>
#   release    create the GitHub release (LLM drafts the notes, you confirm)
#   verify     fresh throwaway project that depends only on the published versions
#
# `commit` precedes `publish` so crates.io never holds bytes that appear in no
# commit. Publishing still precedes CI: a version can be burned by a commit that
# CI later rejects. To avoid that, reorder to
#   preflight gates version commit push ci publish verify tag release
STEPS=("preflight" "gates" "version" "commit" "publish" "push" "ci" "tag" "release" "verify")

# --- layout -----------------------------------------------------------------
STATE_DIR="deploy/.state"
LOG_DIR="deploy/logs"
