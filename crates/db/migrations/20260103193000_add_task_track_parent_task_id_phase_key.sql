-- Add task-level workflow fields for BMAD (and future Enterprise).
-- Note: Task IDs are stored as UUID bytes (BLOB) in SQLite, so parent_task_id is also BLOB.

ALTER TABLE tasks ADD COLUMN track TEXT NOT NULL DEFAULT 'quick';
ALTER TABLE tasks ADD COLUMN parent_task_id BLOB NULL;
ALTER TABLE tasks ADD COLUMN phase_key TEXT NULL;

CREATE INDEX IF NOT EXISTS idx_tasks_parent_task_id ON tasks(parent_task_id);
CREATE INDEX IF NOT EXISTS idx_tasks_track ON tasks(track);

-- Prevent duplicate phases under concurrency. Partial index keeps non-phase tasks unrestricted.
CREATE UNIQUE INDEX IF NOT EXISTS ux_tasks_parent_phase
ON tasks(parent_task_id, phase_key)
WHERE parent_task_id IS NOT NULL AND phase_key IS NOT NULL;

