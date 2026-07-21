-- Worker ids are task-index scoped (for example `mock-1`) and repeat across
-- missions in the same repository. Bundle identity must therefore include the
-- mission or later missions silently collide with earlier turn-zero rows.
DROP INDEX IF EXISTS idx_mem_bundles_w_turn;
CREATE UNIQUE INDEX idx_mem_bundles_mission_worker_turn
    ON memory_bundles(mission_id, worker_id, turn);
