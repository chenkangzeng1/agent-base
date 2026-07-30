use std::path::{Path, PathBuf};

use agent_base::{AgentError, AgentResult};

/// Validate and resolve a user-supplied path against the workspace root.
/// Prevents directory traversal attacks (e.g. `../../etc/passwd`).
///
/// Returns the canonicalized absolute path on success.
pub fn validate_path(workspace: &Path, user_path: &str) -> AgentResult<PathBuf> {
    if user_path.is_empty() {
        return Err(AgentError::internal("path must not be empty"));
    }

    // Reject null bytes which can bypass OS-level checks
    if user_path.contains('\0') {
        return Err(AgentError::internal("path contains null byte"));
    }

    let full_path = workspace.join(user_path);

    // Canonicalize to resolve `..`, `.`, symlinks, etc.
    // If the file doesn't exist yet (for writes), canonicalize the parent.
    let canonical = if full_path.exists() {
        full_path.canonicalize().map_err(|e| {
            AgentError::internal(format!(
                "failed to resolve path {}: {e}",
                full_path.display()
            ))
        })?
    } else {
        // For new files, canonicalize the parent directory and append the filename
        let parent = full_path.parent().unwrap_or(Path::new("."));
        let file_name = full_path.file_name().ok_or_else(|| {
            AgentError::internal(format!("invalid path: {}", full_path.display()))
        })?;
        let canonical_parent = parent.canonicalize().map_err(|e| {
            AgentError::internal(format!(
                "failed to resolve parent directory {}: {e}",
                parent.display()
            ))
        })?;
        canonical_parent.join(file_name)
    };

    // Ensure the canonical path is inside the workspace
    let canonical_workspace = workspace.canonicalize().map_err(|e| {
        AgentError::internal(format!(
            "failed to resolve workspace {}: {e}",
            workspace.display()
        ))
    })?;

    if !canonical.starts_with(&canonical_workspace) {
        return Err(AgentError::internal(format!(
            "path '{}' resolves outside workspace (resolved to {})",
            user_path,
            canonical.display()
        )));
    }

    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_valid_path() {
        let workspace = std::env::temp_dir().join("agent_works_test_validate");
        let _ = fs::create_dir_all(&workspace);

        let result = validate_path(&workspace, "hello.txt");
        assert!(result.is_ok(), "simple path should be valid: {result:?}");

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn test_traversal_rejected() {
        let workspace = std::env::temp_dir().join("agent_works_test_traversal");
        let _ = fs::create_dir_all(&workspace);

        let result = validate_path(&workspace, "../../etc/passwd");
        assert!(result.is_err(), "traversal path should be rejected");

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn test_null_byte_rejected() {
        let workspace = std::env::temp_dir().join("agent_works_test_null");
        let _ = fs::create_dir_all(&workspace);

        let result = validate_path(&workspace, "hello\0world");
        assert!(result.is_err(), "null byte should be rejected");

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn test_empty_path_rejected() {
        let workspace = std::env::temp_dir().join("agent_works_test_empty");
        let _ = fs::create_dir_all(&workspace);

        let result = validate_path(&workspace, "");
        assert!(result.is_err(), "empty path should be rejected");

        let _ = fs::remove_dir_all(&workspace);
    }
}
