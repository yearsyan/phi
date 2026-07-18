use std::{io, path::Path};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
};
use phi::Workspace;
use serde::Deserialize;
use tokio::fs;

use super::{
    ApiError, AppState,
    dto::{WorkspaceBrowseResponse, WorkspaceDirectoryDto},
};

const MAX_SCANNED_ENTRIES: usize = 10_000;
const MAX_DIRECTORY_RESULTS: usize = 2_000;

pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/v1/workspaces/browse", get(browse_workspace))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowseWorkspaceQuery {
    path: Option<String>,
}

async fn browse_workspace(
    State(state): State<AppState>,
    Query(query): Query<BrowseWorkspaceQuery>,
) -> Result<Json<WorkspaceBrowseResponse>, ApiError> {
    let requested = query
        .path
        .as_deref()
        .map(Path::new)
        .unwrap_or_else(|| state.default_workspace().root());
    browse_directory(requested).await.map(Json)
}

pub(super) async fn resolve_workspace_path(path: &Path) -> Result<Workspace, ApiError> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(ApiError::bad_request(
            "invalid_workspace",
            "workspace path must be a non-empty absolute path",
        ));
    }

    let canonical = fs::canonicalize(path)
        .await
        .map_err(|error| workspace_io_error(path, &error))?;
    let metadata = fs::metadata(&canonical)
        .await
        .map_err(|error| workspace_io_error(&canonical, &error))?;
    if !metadata.is_dir() {
        return Err(ApiError::bad_request(
            "invalid_workspace",
            format!("workspace is not a directory: {}", canonical.display()),
        ));
    }
    drop(
        fs::read_dir(&canonical)
            .await
            .map_err(|error| workspace_io_error(&canonical, &error))?,
    );
    require_utf8_path(&canonical)?;
    Ok(Workspace::new(canonical))
}

async fn browse_directory(path: &Path) -> Result<WorkspaceBrowseResponse, ApiError> {
    let workspace = resolve_workspace_path(path).await?;
    let canonical = workspace.root();
    let mut reader = fs::read_dir(canonical)
        .await
        .map_err(|error| workspace_io_error(canonical, &error))?;
    let mut directories = Vec::new();
    let mut scanned = 0;
    let mut truncated = false;

    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| workspace_io_error(canonical, &error))?
    {
        scanned += 1;
        if scanned > MAX_SCANNED_ENTRIES {
            truncated = true;
            break;
        }

        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        let is_directory = if file_type.is_dir() {
            true
        } else if file_type.is_symlink() {
            fs::metadata(entry.path())
                .await
                .is_ok_and(|metadata| metadata.is_dir())
        } else {
            false
        };
        if !is_directory {
            continue;
        }
        if directories.len() == MAX_DIRECTORY_RESULTS {
            truncated = true;
            break;
        }

        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(path) = entry.path().to_str().map(str::to_owned) else {
            continue;
        };
        directories.push(WorkspaceDirectoryDto { name, path });
    }

    directories.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(WorkspaceBrowseResponse {
        path: require_utf8_path(canonical)?,
        parent: canonical.parent().map(require_utf8_path).transpose()?,
        directories,
        truncated,
    })
}

fn require_utf8_path(path: &Path) -> Result<String, ApiError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        ApiError::bad_request(
            "unsupported_workspace_path",
            "workspace paths exposed through the Web API must be valid UTF-8",
        )
    })
}

fn workspace_io_error(path: &Path, error: &io::Error) -> ApiError {
    let (status, code) = match error.kind() {
        io::ErrorKind::NotFound => (StatusCode::NOT_FOUND, "workspace_not_found"),
        io::ErrorKind::PermissionDenied => (StatusCode::FORBIDDEN, "workspace_unreadable"),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "workspace_io_error"),
    };
    ApiError::new(
        status,
        code,
        format!("could not access workspace {}: {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use std::{fs as std_fs, path::PathBuf};

    use uuid::Uuid;

    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "phi-daemon-workspace-browser-test-{}",
                Uuid::now_v7()
            ));
            std_fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std_fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn lists_only_directories_in_stable_order() {
        let root = TestDirectory::new();
        std_fs::create_dir(root.0.join("Zulu")).unwrap();
        std_fs::create_dir(root.0.join("alpha")).unwrap();
        std_fs::write(root.0.join("file.txt"), "not a directory").unwrap();

        let response = browse_directory(&root.0).await.unwrap();

        assert_eq!(
            response.path,
            std_fs::canonicalize(&root.0).unwrap().to_string_lossy()
        );
        assert_eq!(
            response
                .directories
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "Zulu"]
        );
        assert!(!response.truncated);
    }

    #[tokio::test]
    async fn rejects_relative_and_non_directory_workspaces() {
        let relative = resolve_workspace_path(Path::new("relative"))
            .await
            .unwrap_err();
        assert_eq!(relative.status, StatusCode::BAD_REQUEST);
        assert_eq!(relative.code, "invalid_workspace");

        let root = TestDirectory::new();
        let file = root.0.join("file.txt");
        std_fs::write(&file, "file").unwrap();
        let file = resolve_workspace_path(&file).await.unwrap_err();
        assert_eq!(file.status, StatusCode::BAD_REQUEST);
        assert_eq!(file.code, "invalid_workspace");
    }
}
