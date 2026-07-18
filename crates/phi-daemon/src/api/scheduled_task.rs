use std::{path::Path as FilePath, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    routing::get,
};

use super::{
    ApiError, AppState,
    dto::{
        CreateScheduledTaskRequest, ScheduledTaskDto, ScheduledTasksResponse,
        UpdateScheduledTaskRequest,
    },
    workspace::resolve_workspace_path,
};
use crate::{
    runtime::DEFAULT_AGENT_PROFILE_ID,
    scheduled_task::{ScheduledTaskId, ScheduledTaskManager},
    store::DEFAULT_PROFILE_ID,
};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/scheduled-tasks",
            get(list_scheduled_tasks).post(create_scheduled_task),
        )
        .route(
            "/v1/scheduled-tasks/{task_id}",
            get(get_scheduled_task)
                .patch(update_scheduled_task)
                .delete(delete_scheduled_task),
        )
        .route(
            "/v1/scheduled-tasks/{task_id}/run",
            axum::routing::post(run_scheduled_task),
        )
}

async fn list_scheduled_tasks(
    State(state): State<AppState>,
) -> Result<Json<ScheduledTasksResponse>, ApiError> {
    let tasks = manager(&state)?
        .list_tasks()
        .await
        .map_err(ApiError::scheduled_task)?
        .into_iter()
        .map(ScheduledTaskDto::from)
        .collect();
    Ok(Json(ScheduledTasksResponse { tasks }))
}

async fn create_scheduled_task(
    State(state): State<AppState>,
    request: Result<Json<CreateScheduledTaskRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ScheduledTaskDto>), ApiError> {
    let Json(request) = request
        .map_err(|error| ApiError::bad_request("invalid_scheduled_task", error.body_text()))?;
    let workspace = match request.workspace.as_deref() {
        Some(path) => resolve_workspace_path(FilePath::new(path)).await?,
        None => state.default_workspace().clone(),
    };
    let task = manager(&state)?
        .create_task(request.into_create(workspace, DEFAULT_PROFILE_ID, DEFAULT_AGENT_PROFILE_ID))
        .await
        .map_err(ApiError::scheduled_task)?;
    Ok((StatusCode::CREATED, Json(ScheduledTaskDto::from(task))))
}

async fn get_scheduled_task(
    State(state): State<AppState>,
    Path(task_id): Path<ScheduledTaskId>,
) -> Result<Json<ScheduledTaskDto>, ApiError> {
    let task = manager(&state)?
        .get_task(task_id)
        .await
        .map_err(ApiError::scheduled_task)?;
    Ok(Json(ScheduledTaskDto::from(task)))
}

async fn update_scheduled_task(
    State(state): State<AppState>,
    Path(task_id): Path<ScheduledTaskId>,
    request: Result<Json<UpdateScheduledTaskRequest>, JsonRejection>,
) -> Result<Json<ScheduledTaskDto>, ApiError> {
    let Json(request) = request
        .map_err(|error| ApiError::bad_request("invalid_scheduled_task", error.body_text()))?;
    let task = manager(&state)?
        .update_task(task_id, request.into())
        .await
        .map_err(ApiError::scheduled_task)?;
    Ok(Json(ScheduledTaskDto::from(task)))
}

async fn delete_scheduled_task(
    State(state): State<AppState>,
    Path(task_id): Path<ScheduledTaskId>,
) -> Result<StatusCode, ApiError> {
    manager(&state)?
        .delete_task(task_id)
        .await
        .map_err(ApiError::scheduled_task)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn run_scheduled_task(
    State(state): State<AppState>,
    Path(task_id): Path<ScheduledTaskId>,
) -> Result<StatusCode, ApiError> {
    manager(&state)?
        .run_now(task_id)
        .await
        .map_err(ApiError::scheduled_task)?;
    Ok(StatusCode::ACCEPTED)
}

fn manager(state: &AppState) -> Result<Arc<ScheduledTaskManager>, ApiError> {
    state.scheduled_tasks().cloned().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "scheduled_tasks_unavailable",
            "scheduled-task management is unavailable for this embedded daemon",
        )
    })
}
