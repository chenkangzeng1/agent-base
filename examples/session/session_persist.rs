//! Session Persist — demonstrate session management and persistence.
//!
//! Shows how to create, resume, and clean up sessions. Each session has a
//! dedicated directory with per-turn JSONL event logs, metadata, and file
//! locking to prevent concurrent access.
//!
//! Usage:
//!   cargo run --example session_persist

use phi_agent::session;

#[path = "../common/mod.rs"]
mod common;

fn main() -> anyhow::Result<()> {
    // ── 1. Session directory ──
    let base_dir = std::env::temp_dir().join("phi-agent-session-demo");
    println!("Session base dir: {}", base_dir.display());

    // ── 2. Create or resume a session ──
    let ctx = session::resolve_session(Some("demo-session-001"), &base_dir)?;
    println!("Session ID: {}", ctx.session_id);
    println!("Is new session: {}", ctx.is_new_session);
    println!("Session dir: {}", ctx.session_dir.display());
    println!("Log path: {}", ctx.log_path().display());
    println!("Turn path (turn 1): {}", ctx.turn_path(1).display());

    // ── 3. Session ID validation ──
    assert!(session::validate_session_id("valid-id-123").is_ok());
    assert!(session::validate_session_id("").is_err());
    assert!(session::validate_session_id("spaces not allowed").is_err());

    // ── 4. Auto-generate session ID ──
    let auto_id = session::generate_session_id();
    println!("Auto-generated ID: {}", auto_id);
    // Format: YYYYMMDD_first8ofUuid
    let parts: Vec<&str> = auto_id.split('_').collect();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].len(), 8); // date
    assert_eq!(parts[1].len(), 8); // uuid prefix

    // ── 5. Concurrent access prevention ──
    // The session directory is locked. Trying to lock again fails:
    let result = session::resolve_session(Some("demo-session-001"), &base_dir);
    match result {
        Ok(_) => println!("Unexpected: second lock succeeded"),
        Err(e) => {
            // AgentError::ResourceUnavailable — callers can match on this variant
            println!("Expected: second lock failed — {}", e);
            assert!(matches!(e, phi_agent::AgentError::ResourceUnavailable(_)));
        },
    }

    // ── 6. Cleanup expired sessions ──
    // Sessions older than N days are automatically removed.
    // Active (locked) sessions are skipped.
    let cleaned = session::cleanup_expired_sessions(&base_dir, 7)?;
    println!("Cleaned {} expired session(s)", cleaned);

    // ── 7. Clean up demo ──
    drop(ctx); // release the lock
    let _ = std::fs::remove_dir_all(&base_dir);

    println!("\n=== Session management demonstrated ===");
    Ok(())
}
