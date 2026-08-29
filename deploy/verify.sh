#!/usr/bin/env bash
# ============================================================================
# deploy/verify.sh — post-publish proof that the public can actually use it.
#
# Builds a throwaway project in a temp dir whose Cargo.toml lists ONLY version
# requirements, no path dependencies, then compiles a snippet exercising the
# documented API. This is the strongest available signal: it is literally what a
# stranger gets when they type `cargo add <crate>`.
#
# Independent of release.sh on purpose — run it any time to confirm nothing has
# rotted on the registry side (yanked dep, index lag, bad metadata).
# ============================================================================
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

REQ_VERSION="${1:-$(manifest_version Cargo.toml)}"
[ -n "$REQ_VERSION" ] || die "cannot determine a version to verify"

section "verify $PROJECT_NAME@$REQ_VERSION from crates.io"

# --- 1. index presence ------------------------------------------------------
for entry in "${CRATES[@]}"; do
  n=$(crate_name_of "$entry")
  if index_has_version "$n" "$REQ_VERSION"; then
    ok "$n@$REQ_VERSION present in the sparse index"
  else
    if crates_io_exists "$n"; then
      bad "$n@$REQ_VERSION not in the index (latest published: $(crates_io_latest "$n"))"
    else
      bad "$n has never been published"
    fi
    FAIL_INDEX=1
  fi
done
[ "${FAIL_INDEX:-}" = "1" ] && die "index check failed; nothing else is meaningful"

# --- 2. a stranger's build --------------------------------------------------
TMP="$(mktemp -d)"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

{
  printf '[package]\nname = "consumer"\nversion = "0.0.0"\nedition = "2021"\n\n'
  printf '[dependencies]\n'
  for entry in "${CRATES[@]}"; do
    printf '%s = "%s"\n' "$(crate_name_of "$entry")" "$REQ_VERSION"
  done
  printf 'tokio = { version = "1", features = ["macros", "rt-multi-thread"] }\n'
} > "$TMP/Cargo.toml"

# No [workspace] marker means cargo could walk up and find one; pin it out.
printf '[workspace]\n' >> "$TMP/Cargo.toml"

mkdir -p "$TMP/src"
cat > "$TMP/src/main.rs" <<'RS'
//! Compile-and-run against the published crate. Keep in sync with the README.
#![allow(dead_code, unused_variables)]

use llm_trait::{
    Capabilities, ChatMessage, ChatRequest, LlmConfig, LlmError, LlmProvider, Protocol,
    ReasoningMode, StreamChunk, UsageInfo,
};
use llm_unified::model_registry::ModelRegistry;
use llm_unified::{create, create_provider, GenericProvider, OpenAiProtocol};
fn config_shape() {
    // The README quick start promises exactly these five fields.
    let c = LlmConfig {
        protocol: Some(Protocol::OpenAi),
        api_key: "sk-test".into(),
        model: "gpt-4o-mini".into(),
        base_url: "https://api.openai.com/v1".into(),
        options: Default::default(),
    };
    // Credential redaction is a documented guarantee, so assert it here too.
    let rendered = format!("{c:?}");
    assert!(!rendered.contains("sk-test"), "api key leaked through Debug");
}

fn factories() -> Result<(), LlmError> {
    // Offline: construction never touches the network, so these are safe.
    let p = create_provider(&LlmConfig {
        protocol: Some(Protocol::OpenAi),
        api_key: "sk-test".into(),
        model: "gpt-4o-mini".into(),
        base_url: "https://api.openai.com/v1".into(),
        options: Default::default(),
    })?;
    let _: &dyn LlmProvider = &*p;
    let _ = p.info();
    let _ = p.capabilities();
    // The 3-argument convenience constructor is documented in the README.
    let q = create("sk-test", "gpt-4o", "https://api.openai.com/v1")?;
    assert_eq!(q.info().model, "gpt-4o");
    Ok(())
}

fn registry_and_types() {
    let r = ModelRegistry::builtin();
    let profile = r.lookup("gpt-4o", Some("https://api.openai.com/v1"), None);
    let caps: Capabilities = profile.capabilities.clone();
    let mode: ReasoningMode = profile.reasoning_mode;
    let mut usage = UsageInfo::default();
    usage.merge(&UsageInfo { prompt_tokens: Some(7), ..Default::default() });
    assert_eq!(usage.prompt_tokens, Some(7));
    let req = ChatRequest::new(vec![ChatMessage::system("s"), ChatMessage::user("u")])
        .with_tools(vec![]);
    fn _arms(c: StreamChunk) {
        match c {
            StreamChunk::Text(_) | StreamChunk::Thought(_) | StreamChunk::Stop { .. } => {}
            _ => {}
        }
    }
    let _ = req.clone();
}

fn error_and_parse() {
    assert_eq!(LlmError::api(429, "slow").status(), Some(429));
    assert_eq!(LlmError::config("x").status(), None);
    let fr: llm_trait::FinishReason = "max_tokens".parse().unwrap();
    assert_eq!(fr, llm_trait::FinishReason::Length);
    // Injecting a transport is part of the published surface.
    let _ = GenericProvider::new(Box::new(OpenAiProtocol::new("k", "m", Some("http://localhost"))));
}

fn main() {
    config_shape();
    factories().expect("create_provider failed");
    registry_and_types();
    error_and_parse();
    println!("verified: public API matches the documented shape");
}
RS

info "building a fresh consumer project in $(basename "$TMP")..."
if (cd "$TMP" && cargo run --quiet 2>&1 | tail -3) | tee -a "$RUN_LOG"; then
  ok "consumer built and ran against $PROJECT_NAME@$REQ_VERSION"
else
  err "the published crates do not work from a clean project"
  err "manifest kept at: $TMP (trap will remove it; re-run with DEPLOY_KEEP=1)"
  [ -n "${DEPLOY_KEEP:-}" ] || trap - EXIT
  exit 1
fi

# --- 3. metadata the website shows -----------------------------------------
# Note the endpoint shape: at /crates/<name>/<version> the `crate` key is null
# and everything lives under `version` — reading .crate.repository there yields
# empty and looks like missing metadata when it is not.
for entry in "${CRATES[@]}"; do
  n=$(crate_name_of "$entry")
  meta=$(curl -s --max-time "$HTTP_TIMEOUT" -H "User-Agent: $UA" \
    "$CRATES_API/crates/$n/$REQ_VERSION" 2>/dev/null || true)
  lic=$(printf '%s' "$meta" | jq -r '.version.license // empty' 2>/dev/null)
  repo=$(printf '%s' "$meta" | jq -r '.version.repository // empty' 2>/dev/null)
  yanked=$(printf '%s' "$meta" | jq -r '.version.yanked' 2>/dev/null)
  [ "$lic" = "MIT" ] && ok "$n: license=$lic" || bad "$n: license='$lic' (expected MIT)"
  [ "$repo" = "https://github.com/$REPO_SLUG" ] \
    && ok "$n: repository=$repo" \
    || bad "$n: repository='$repo', expected https://github.com/$REPO_SLUG"
  [ "$yanked" = "false" ] && ok "$n@$REQ_VERSION not yanked" || bad "$n@$REQ_VERSION is YANKED"
  # keywords/categories live on the CRATE object, not the version object — the
  # /crates/<name>/<version> response has no keywords field at all, so querying
  # it there always looks like the metadata is missing.
  cmap=$(curl -s --max-time "$HTTP_TIMEOUT" -H "User-Agent: $UA" \
    "$CRATES_API/crates/$n" 2>/dev/null || true)
  kw=$(printf '%s' "$cmap" | jq -r '(.keywords // []) | map(.id) | join(",")' 2>/dev/null)
  cat_=$(printf '%s' "$cmap" | jq -r '(.categories // []) | map(.id) | join(",")' 2>/dev/null)
  [ -n "$kw" ] && ok "$n: keywords = $kw" || warn "$n: no keywords registered"
  [ -n "$cat_" ] && ok "$n: categories = $cat_" || warn "$n: no categories registered"
  desc=$(printf '%s' "$cmap" | jq -r '.crate.description // empty' 2>/dev/null)
  [ -n "$desc" ] && ok "$n: description present" || bad "$n: no description on crates.io"
done

ok "verification complete"
