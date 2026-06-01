-- Enforce the (workspace_id, project_id) identity invariant on write.
--
-- The schema denormalises workspace_id onto every domain row but only ever
-- enforced `project_id → projects(id)` and `workspace_id → workspaces(id)`
-- INDEPENDENTLY — never that the pair is consistent (that the project actually
-- lives in that workspace). That gap lets a stale writer (e.g. a hook router
-- whose per-cwd cache still holds the OLD workspace_id for a project that was
-- just moved to another workspace) silently insert a split-brain row instead
-- of failing cleanly.
--
-- These BEFORE INSERT triggers close the gap: an INSERT whose workspace_id
-- disagrees with the project's actual workspace ABORTs. The hook router already
-- evicts its cache and re-resolves on a write error, so a clean abort turns the
-- worst case (silent corruption) into self-healing.
--
-- INSERT only — never UPDATE — so the move-project re-stamp (which UPDATEs
-- workspace_id across these tables in one transaction) is unaffected.

CREATE TRIGGER pages_ws_proj_pairing_ai
BEFORE INSERT ON pages
FOR EACH ROW
WHEN NEW.workspace_id IS NOT (SELECT workspace_id FROM projects WHERE id = NEW.project_id)
BEGIN
    SELECT RAISE(ABORT, 'pages.workspace_id does not match the project''s workspace');
END;

CREATE TRIGGER sessions_ws_proj_pairing_ai
BEFORE INSERT ON sessions
FOR EACH ROW
WHEN NEW.workspace_id IS NOT (SELECT workspace_id FROM projects WHERE id = NEW.project_id)
BEGIN
    SELECT RAISE(ABORT, 'sessions.workspace_id does not match the project''s workspace');
END;

CREATE TRIGGER observations_ws_proj_pairing_ai
BEFORE INSERT ON observations
FOR EACH ROW
WHEN NEW.workspace_id IS NOT (SELECT workspace_id FROM projects WHERE id = NEW.project_id)
BEGIN
    SELECT RAISE(ABORT, 'observations.workspace_id does not match the project''s workspace');
END;

CREATE TRIGGER handoffs_ws_proj_pairing_ai
BEFORE INSERT ON handoffs
FOR EACH ROW
WHEN NEW.workspace_id IS NOT (SELECT workspace_id FROM projects WHERE id = NEW.project_id)
BEGIN
    SELECT RAISE(ABORT, 'handoffs.workspace_id does not match the project''s workspace');
END;
