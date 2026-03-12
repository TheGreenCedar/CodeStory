use super::*;

pub(super) fn resolve_project_file_path(
    controller: &AppController,
    path: &str,
    allow_missing_leaf: bool,
) -> Result<PathBuf, ApiError> {
    let root = controller.require_project_root()?;
    let root = root
        .canonicalize()
        .map_err(|e| ApiError::internal(format!("Failed to resolve project root: {e}")))?;

    let raw = PathBuf::from(path);
    let candidate = if raw.is_absolute() {
        raw
    } else {
        root.join(raw)
    };

    let resolved = match candidate.canonicalize() {
        Ok(canonical) => canonical,
        Err(err) if allow_missing_leaf && err.kind() == io::ErrorKind::NotFound => {
            let Some(parent) = candidate.parent() else {
                return Err(ApiError::invalid_argument(format!(
                    "Invalid file path: {}",
                    candidate.display()
                )));
            };
            let Some(file_name) = candidate.file_name() else {
                return Err(ApiError::invalid_argument(format!(
                    "Invalid file path: {}",
                    candidate.display()
                )));
            };

            let parent = parent.canonicalize().map_err(|e| {
                if e.kind() == io::ErrorKind::NotFound {
                    ApiError::not_found(format!("Parent directory not found: {}", parent.display()))
                } else {
                    ApiError::internal(format!("Failed to resolve parent path: {e}"))
                }
            })?;
            parent.join(file_name)
        }
        Err(err) => {
            return Err(if err.kind() == io::ErrorKind::NotFound {
                ApiError::not_found(format!("File not found: {}", candidate.display()))
            } else {
                ApiError::internal(format!("Failed to resolve file path: {err}"))
            });
        }
    };

    if !resolved.starts_with(&root) {
        return Err(ApiError::invalid_argument(
            "Refusing to access file outside project root.",
        ));
    }

    Ok(resolved)
}
