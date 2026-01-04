use db::models::task::{PhaseKey, Task, TaskStatus, TaskTrack};
use sqlx::{Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

const BMAD_PHASES: &[(PhaseKey, &str)] = &[
    (PhaseKey::Intake, "Intake"),
    (PhaseKey::Prd, "PRD"),
    (PhaseKey::Arch, "Architecture"),
    (PhaseKey::Stories, "Stories"),
    (PhaseKey::Impl, "Implementation"),
    (PhaseKey::Qa, "QA"),
    (PhaseKey::Review, "Review"),
];

pub async fn ensure_bmad_phases(pool: &SqlitePool, parent: &Task) -> Result<Vec<Task>, sqlx::Error> {
    if parent.track == TaskTrack::Quick || parent.parent_task_id.is_some() || parent.phase_key.is_some() {
        return Ok(vec![]);
    }

    let mut tx: Transaction<'_, Sqlite> = pool.begin().await?;

    for (phase_key, title) in BMAD_PHASES {
        let id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT OR IGNORE INTO tasks
               (id, project_id, title, description, status, track, parent_workspace_id, parent_task_id, phase_key, shared_task_id)
               VALUES (?1, ?2, ?3, NULL, ?4, ?5, NULL, ?6, ?7, NULL)"#,
        )
        .bind(id)
        .bind(parent.project_id)
        .bind(*title)
        .bind(TaskStatus::Todo)
        .bind(parent.track)
        .bind(parent.id)
        .bind(*phase_key)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Task::find_children_by_task_id(pool, parent.id).await
}
