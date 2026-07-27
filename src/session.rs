use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Result, bail};
use fs2::FileExt;
use regex::Regex;

static SESSION_ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9\-_]+$").unwrap());

/// Session context — holds the session ID, directory, and file lock.
///
/// Created via [`resolve_session`]. The lock is released when the struct is dropped.
pub struct SessionContext {
    pub session_id: String,
    pub session_dir: PathBuf,
    pub is_new_session: bool,
    _lock: Option<File>,
}

impl SessionContext {
    /// Path to the session ID marker file.
    #[allow(dead_code)]
    pub fn session_id_path(&self) -> PathBuf {
        self.session_dir.join("session_id")
    }

    /// Path to the session metadata JSON file.
    #[allow(dead_code)]
    pub fn metadata_path(&self) -> PathBuf {
        self.session_dir.join("session_meta.json")
    }

    /// Path to the session log file (human-readable).
    pub fn log_path(&self) -> PathBuf {
        self.session_dir.join("session.log")
    }

    /// Path to the per-turn JSONL event log. `turn` is 1-indexed.
    pub fn turn_path(&self, turn: usize) -> PathBuf {
        self.session_dir.join(format!("turn_{:03}.jsonl", turn))
    }
}

/// Validate session ID format.
///
/// Allowed: alphanumerics, hyphens, underscores. 1–128 characters.
pub fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty() || session_id.len() > 128 {
        bail!("Session ID must be 1-128 characters, got {}", session_id.len());
    }

    if !SESSION_ID_RE.is_match(session_id) {
        bail!("Invalid session_id format '{}'. Only alphanumeric, hyphens, and underscores allowed.", session_id);
    }

    Ok(())
}

/// Resolve session ID (priority: CLI arg → `PHI_SESSION_ID` env var → auto-generate).
pub fn resolve_session_id(cli_session_id: Option<&str>) -> Result<String> {
    if let Some(id) = cli_session_id {
        validate_session_id(id)?;
        return Ok(id.to_string());
    }

    if let Ok(id) = std::env::var("PHI_SESSION_ID")
        && !id.is_empty()
    {
        validate_session_id(&id)?;
        return Ok(id);
    }

    Ok(generate_session_id())
}

/// Generate a new session_id (format: YYYYMMDD_first8ofUuid)
pub fn generate_session_id() -> String {
    let now = chrono::Local::now();
    let uuid = uuid::Uuid::new_v4().to_string();
    let uuid_short = &uuid[..8.min(uuid.len())];
    format!("{}_{}", now.format("%Y%m%d"), uuid_short)
}

/// Get or create a session directory under `base_dir/sessions/<session_id>`.
///
/// Returns the directory path and whether it was newly created.
pub fn get_or_create_session_dir(session_id: &str, base_dir: &Path) -> Result<(PathBuf, bool)> {
    let session_dir = base_dir.join("sessions").join(session_id);
    let is_new = !session_dir.exists();

    if is_new {
        std::fs::create_dir_all(&session_dir)?;
        tracing::info!(session_id = %session_id, path = %session_dir.display(), "created new session directory");
    } else {
        tracing::info!(session_id = %session_id, path = %session_dir.display(), "reusing existing session directory");
    }

    // Write session_id file
    std::fs::write(session_dir.join("session_id"), session_id)?;

    // Update session_meta.json
    update_session_meta(&session_dir, session_id)?;

    Ok((session_dir, is_new))
}

/// Acquire an exclusive file lock on the session directory.
///
/// Prevents concurrent access from other processes. Returns an error if the
/// session is already in use.
pub fn acquire_session_lock(session_dir: &Path) -> Result<File> {
    let lock_path = session_dir.join("session.lock");
    let file = File::create(&lock_path)?;

    file.try_lock_exclusive().map_err(|_| {
        anyhow::anyhow!(
            "Session '{}' is currently in use by another process",
            session_dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        )
    })?;

    Ok(file)
}

/// Update session_meta.json
fn update_session_meta(session_dir: &Path, session_id: &str) -> Result<()> {
    let meta_path = session_dir.join("session_meta.json");

    let mut meta = if meta_path.exists() {
        let content = std::fs::read_to_string(&meta_path)?;
        serde_json::from_str::<serde_json::Value>(&content)?
    } else {
        serde_json::json!({
            "session_id": session_id,
            "created_at": chrono::Utc::now().to_rfc3339(),
        })
    };

    meta["last_active_at"] = serde_json::json!(chrono::Utc::now().to_rfc3339());

    std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
    Ok(())
}

/// Clean up expired sessions.
///
/// Sessions inactive for more than `max_age_days` are removed from disk.
/// Active (locked) sessions are skipped.
pub fn cleanup_expired_sessions(base_dir: &Path, max_age_days: i64) -> Result<u32> {
    let sessions_dir = base_dir.join("sessions");
    if !sessions_dir.exists() {
        return Ok(0);
    }

    let now = chrono::Utc::now();
    let mut cleaned = 0;

    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let lock_path = path.join("session.lock");
        if lock_path.exists()
            && let Ok(file) = File::open(&lock_path)
            && file.try_lock_shared().is_err()
        {
            continue; // locked, skip
        }

        let meta_path = path.join("session_meta.json");
        if !meta_path.exists() {
            std::fs::remove_dir_all(&path)?;
            cleaned += 1;
            continue;
        }

        let content = std::fs::read_to_string(&meta_path)?;
        let meta: serde_json::Value = serde_json::from_str(&content)?;

        if let Some(last_active) = meta["last_active_at"].as_str()
            && let Ok(last_active) = chrono::DateTime::parse_from_rfc3339(last_active)
        {
            let age = now - last_active.with_timezone(&chrono::Utc);
            if age.num_days() > max_age_days {
                tracing::info!(path = %path.display(), age_days = age.num_days(), "removing expired session");
                std::fs::remove_dir_all(&path)?;
                cleaned += 1;
            }
        }
    }

    if cleaned > 0 {
        tracing::info!(count = cleaned, "cleaned up expired sessions");
    }

    Ok(cleaned)
}

/// Resolve and create a session context — session ID, directory, and file lock.
///
/// This is the primary entry point for session setup. It combines ID resolution,
/// directory creation, and lock acquisition into a single call.
pub fn resolve_session(cli_session_id: Option<&str>, base_dir: &Path) -> Result<SessionContext> {
    let session_id = resolve_session_id(cli_session_id)?;
    let (session_dir, is_new) = get_or_create_session_dir(&session_id, base_dir)?;
    let lock = acquire_session_lock(&session_dir)?;

    Ok(SessionContext { session_id, session_dir, is_new_session: is_new, _lock: Some(lock) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_validate_session_id_valid() {
        assert!(validate_session_id("my-session-123").is_ok());
        assert!(validate_session_id("test_456").is_ok());
        assert!(validate_session_id("a").is_ok());
    }

    #[test]
    fn test_validate_session_id_invalid() {
        assert!(validate_session_id("").is_err());
        assert!(validate_session_id("my session").is_err());
        assert!(validate_session_id("../etc").is_err());
        assert!(validate_session_id("path/traversal").is_err());
    }

    #[test]
    fn test_generate_session_id() {
        let id = generate_session_id();
        assert!(id.contains('_'));
        let parts: Vec<&str> = id.split('_').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 8);
    }

    #[test]
    fn test_session_context_methods() {
        let tmp = TempDir::new().unwrap();
        let ctx = resolve_session(Some("test-ctx"), tmp.path()).unwrap();

        assert_eq!(ctx.session_id, "test-ctx");
        assert!(ctx.session_id_path().exists());
        assert!(ctx.metadata_path().exists());
        assert_eq!(ctx.log_path(), ctx.session_dir.join("session.log"));
        assert_eq!(ctx.turn_path(1), ctx.session_dir.join("turn_001.jsonl"));
    }

    #[test]
    fn test_cleanup_expired_sessions() {
        let tmp = TempDir::new().unwrap();
        let (dir, _) = get_or_create_session_dir("old-session", tmp.path()).unwrap();

        let meta_path = dir.join("session_meta.json");
        let mut meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
        let old = (chrono::Utc::now() - chrono::Duration::days(8)).to_rfc3339();
        meta["last_active_at"] = serde_json::json!(old);
        std::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();

        get_or_create_session_dir("new-session", tmp.path()).unwrap();
        let cleaned = cleanup_expired_sessions(tmp.path(), 7).unwrap();
        assert_eq!(cleaned, 1);
        assert!(!dir.exists());
    }
}
