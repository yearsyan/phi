use std::{
    env, fmt,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// Immutable working-directory identity associated with an agent session.
///
/// Relative roots are resolved against the process current directory when the
/// value is created. Persisted workspaces must be absolute so restoring a
/// session cannot silently reinterpret its root under a different process cwd.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = if root.is_absolute() {
            root
        } else {
            env::current_dir().map_or(root.clone(), |current| current.join(root))
        };
        Self {
            root: lexical_normalize(&root),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a path relative to this workspace without treating the
    /// workspace as a filesystem sandbox.
    pub fn resolve(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        if path.is_absolute() {
            lexical_normalize(path)
        } else {
            lexical_normalize(&self.root.join(path))
        }
    }
}

impl AsRef<Path> for Workspace {
    fn as_ref(&self) -> &Path {
        self.root()
    }
}

impl fmt::Debug for Workspace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Workspace")
            .field(&self.root)
            .finish()
    }
}

impl fmt::Display for Workspace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.root.display().fmt(formatter)
    }
}

impl Serialize for Workspace {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.root.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Workspace {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let root = PathBuf::deserialize(deserializer)?;
        if root.as_os_str().is_empty() {
            return Err(de::Error::custom("workspace root must not be empty"));
        }
        if !root.is_absolute() {
            return Err(de::Error::custom(
                "persisted workspace root must be absolute",
            ));
        }
        Ok(Self {
            root: lexical_normalize(&root),
        })
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !path.is_absolute() {
                    normalized.push(component.as_os_str());
                }
            }
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_roots_and_paths_once() {
        let current = env::current_dir().unwrap();
        let workspace = Workspace::new("project/./src/..");

        assert_eq!(workspace.root(), current.join("project"));
        assert_eq!(
            workspace.resolve("src/../Cargo.toml"),
            current.join("project/Cargo.toml")
        );
    }

    #[test]
    fn persisted_workspace_must_be_absolute() {
        let error = serde_json::from_str::<Workspace>("\"relative\"").unwrap_err();
        assert!(error.to_string().contains("must be absolute"));
    }
}
