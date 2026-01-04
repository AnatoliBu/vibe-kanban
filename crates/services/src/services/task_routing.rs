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

#[cfg(test)]
mod tests {
    use super::*;
    use db::models::{project::Project, task::CreateTask};
    use sqlx::SqlitePool;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        // Run migrations - note: using relative path from the services crate
        sqlx::migrate!("../db/migrations")
            .run(&pool)
            .await
            .unwrap();

        pool
    }

    async fn create_test_project(pool: &SqlitePool) -> Project {
        let project_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO projects (id, name, created_at, updated_at)
               VALUES (?1, ?2, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"#,
        )
        .bind(project_id)
        .bind("Test Project")
        .execute(pool)
        .await
        .unwrap();

        Project::find_by_id(pool, project_id)
            .await
            .unwrap()
            .unwrap()
    }

    async fn create_test_task(
        pool: &SqlitePool,
        project_id: Uuid,
        track: TaskTrack,
        parent_task_id: Option<Uuid>,
        phase_key: Option<PhaseKey>,
    ) -> Task {
        let task = CreateTask {
            project_id,
            title: "Test Task".to_string(),
            description: Some("Test description".to_string()),
            status: Some(TaskStatus::Todo),
            track: Some(track),
            parent_workspace_id: None,
            parent_task_id,
            phase_key,
            image_ids: None,
            shared_task_id: None,
        };

        let task_id = Uuid::new_v4();
        Task::create(pool, &task, task_id).await.unwrap()
    }

    #[tokio::test]
    async fn test_ensure_bmad_phases_creates_all_phases() {
        let pool = setup_test_db().await;
        let project = create_test_project(&pool).await;
        let parent = create_test_task(&pool, project.id, TaskTrack::Bmad, None, None).await;

        let phases = ensure_bmad_phases(&pool, &parent).await.unwrap();

        assert_eq!(phases.len(), 7, "Should create 7 BMAD phases");

        let phase_keys: Vec<PhaseKey> = phases.iter().map(|p| p.phase_key.unwrap()).collect();
        assert!(phase_keys.contains(&PhaseKey::Intake));
        assert!(phase_keys.contains(&PhaseKey::Prd));
        assert!(phase_keys.contains(&PhaseKey::Arch));
        assert!(phase_keys.contains(&PhaseKey::Stories));
        assert!(phase_keys.contains(&PhaseKey::Impl));
        assert!(phase_keys.contains(&PhaseKey::Qa));
        assert!(phase_keys.contains(&PhaseKey::Review));

        // Verify all phases have the correct parent
        for phase in &phases {
            assert_eq!(phase.parent_task_id, Some(parent.id));
            assert_eq!(phase.track, TaskTrack::Bmad);
            assert_eq!(phase.status, TaskStatus::Todo);
        }
    }

    #[tokio::test]
    async fn test_ensure_bmad_phases_is_idempotent() {
        let pool = setup_test_db().await;
        let project = create_test_project(&pool).await;
        let parent = create_test_task(&pool, project.id, TaskTrack::Bmad, None, None).await;

        // Call ensure_bmad_phases twice
        let phases1 = ensure_bmad_phases(&pool, &parent).await.unwrap();
        let phases2 = ensure_bmad_phases(&pool, &parent).await.unwrap();

        assert_eq!(phases1.len(), 7, "First call should create 7 phases");
        assert_eq!(phases2.len(), 7, "Second call should return same 7 phases");

        // Verify IDs are the same (phases weren't duplicated)
        let ids1: Vec<Uuid> = phases1.iter().map(|p| p.id).collect();
        let ids2: Vec<Uuid> = phases2.iter().map(|p| p.id).collect();
        
        for id in ids1 {
            assert!(ids2.contains(&id), "Phase IDs should match between calls");
        }
    }

    #[tokio::test]
    async fn test_ensure_bmad_phases_skips_quick_tasks() {
        let pool = setup_test_db().await;
        let project = create_test_project(&pool).await;
        let parent = create_test_task(&pool, project.id, TaskTrack::Quick, None, None).await;

        let phases = ensure_bmad_phases(&pool, &parent).await.unwrap();

        assert_eq!(phases.len(), 0, "Should not create phases for Quick tasks");
    }

    #[tokio::test]
    async fn test_ensure_bmad_phases_skips_child_tasks() {
        let pool = setup_test_db().await;
        let project = create_test_project(&pool).await;
        let parent = create_test_task(&pool, project.id, TaskTrack::Bmad, None, None).await;
        let child = create_test_task(
            &pool,
            project.id,
            TaskTrack::Bmad,
            Some(parent.id),
            Some(PhaseKey::Intake),
        )
        .await;

        let phases = ensure_bmad_phases(&pool, &child).await.unwrap();

        assert_eq!(
            phases.len(),
            0,
            "Should not create phases for tasks that are already children"
        );
    }

    #[tokio::test]
    async fn test_ensure_bmad_phases_skips_phase_tasks() {
        let pool = setup_test_db().await;
        let project = create_test_project(&pool).await;
        let task = create_test_task(
            &pool,
            project.id,
            TaskTrack::Bmad,
            None,
            Some(PhaseKey::Prd),
        )
        .await;

        let phases = ensure_bmad_phases(&pool, &task).await.unwrap();

        assert_eq!(
            phases.len(),
            0,
            "Should not create phases for tasks that already have a phase_key"
        );
    }

    #[tokio::test]
    async fn test_ensure_bmad_phases_works_for_enterprise() {
        let pool = setup_test_db().await;
        let project = create_test_project(&pool).await;
        let parent = create_test_task(&pool, project.id, TaskTrack::Enterprise, None, None).await;

        let phases = ensure_bmad_phases(&pool, &parent).await.unwrap();

        assert_eq!(phases.len(), 7, "Should create 7 phases for Enterprise tasks");

        // Verify all phases have Enterprise track
        for phase in &phases {
            assert_eq!(phase.track, TaskTrack::Enterprise);
        }
    }

    #[tokio::test]
    async fn test_ensure_bmad_phases_correct_titles() {
        let pool = setup_test_db().await;
        let project = create_test_project(&pool).await;
        let parent = create_test_task(&pool, project.id, TaskTrack::Bmad, None, None).await;

        let phases = ensure_bmad_phases(&pool, &parent).await.unwrap();

        let expected_titles = vec![
            "Intake",
            "PRD",
            "Architecture",
            "Stories",
            "Implementation",
            "QA",
            "Review",
        ];

        let actual_titles: Vec<String> = phases.iter().map(|p| p.title.clone()).collect();

        for title in expected_titles {
            assert!(
                actual_titles.contains(&title.to_string()),
                "Should have phase with title '{}'",
                title
            );
        }
    }
}
