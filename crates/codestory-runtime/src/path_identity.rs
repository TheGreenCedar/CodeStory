use codestory_workspace::{WorkspacePathIdentity, workspace_path_identity};
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

type NativePathIdentityResolver = fn(&Path) -> io::Result<WorkspacePathIdentity>;

#[derive(Debug, Clone)]
enum CachedPathIdentity {
    Available(WorkspacePathIdentity),
    Unavailable {
        kind: io::ErrorKind,
        message: String,
    },
}

/// One operation's native path observations.
///
/// The cache is deliberately owned by a single runtime operation. Keeping it
/// any longer could reuse an identity after a file is replaced.
pub(crate) struct OperationPathIdentityResolver<R = NativePathIdentityResolver> {
    resolver: R,
    observations: HashMap<PathBuf, CachedPathIdentity>,
}

#[derive(Debug, Clone)]
pub(crate) struct PathIdentityUnavailable {
    pub(crate) path: PathBuf,
    pub(crate) kind: io::ErrorKind,
    pub(crate) message: String,
}

impl fmt::Display for PathIdentityUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({:?}): {}",
            self.path.display(),
            self.kind,
            self.message
        )
    }
}

impl OperationPathIdentityResolver<NativePathIdentityResolver> {
    pub(crate) fn native() -> Self {
        Self::with_resolver(workspace_path_identity)
    }
}

impl<R> OperationPathIdentityResolver<R>
where
    R: FnMut(&Path) -> io::Result<WorkspacePathIdentity>,
{
    pub(crate) fn with_resolver(resolver: R) -> Self {
        Self {
            resolver,
            observations: HashMap::new(),
        }
    }

    pub(crate) fn resolve(
        &mut self,
        path: &Path,
    ) -> Result<WorkspacePathIdentity, PathIdentityUnavailable> {
        if let Some(observation) = self.observations.get(path) {
            return cached_result(path, observation);
        }

        let observation = match (self.resolver)(path) {
            Ok(identity) => CachedPathIdentity::Available(identity),
            Err(error) => CachedPathIdentity::Unavailable {
                kind: error.kind(),
                message: error.to_string(),
            },
        };
        let result = cached_result(path, &observation);
        self.observations.insert(path.to_path_buf(), observation);
        result
    }
}

fn cached_result(
    path: &Path,
    observation: &CachedPathIdentity,
) -> Result<WorkspacePathIdentity, PathIdentityUnavailable> {
    match observation {
        CachedPathIdentity::Available(identity) => Ok(identity.clone()),
        CachedPathIdentity::Unavailable { kind, message } => Err(PathIdentityUnavailable {
            path: path.to_path_buf(),
            kind: *kind,
            message: message.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn unavailable_identity_is_cached_only_for_the_operation() {
        let calls = Cell::new(0_usize);
        let mut resolver = OperationPathIdentityResolver::with_resolver(|path: &Path| {
            calls.set(calls.get() + 1);
            workspace_path_identity(path)
        });
        let malformed = Path::new("identity\0unavailable");

        assert!(resolver.resolve(malformed).is_err());
        assert!(resolver.resolve(malformed).is_err());
        assert_eq!(calls.get(), 1);
    }
}
