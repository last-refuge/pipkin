-- Storage-only execution identity. Unknown identities remain readable but never executable.
ALTER TABLE sessions ADD COLUMN execution_backend TEXT NOT NULL DEFAULT 'nativeAgent';
CREATE TRIGGER session_execution_immutable BEFORE UPDATE OF execution_backend ON sessions
WHEN NEW.execution_backend <> OLD.execution_backend
BEGIN SELECT RAISE(ABORT, 'immutable execution identity'); END;
CREATE TRIGGER cli_session_shape_insert BEFORE INSERT ON sessions
WHEN NEW.execution_backend = 'localClaudeCli' AND
 (NEW.parent IS NOT NULL OR NEW.root IS NOT NULL OR NEW.subagents_enabled <> 0 OR NEW.model IS NOT NULL OR NEW.thinking IS NOT NULL)
BEGIN SELECT RAISE(ABORT, 'invalid CLI session'); END;
CREATE TRIGGER cli_session_shape_update BEFORE UPDATE ON sessions
WHEN OLD.execution_backend = 'localClaudeCli' AND
 (NEW.cwd <> OLD.cwd OR NEW.create_key IS NOT OLD.create_key OR NEW.parent IS NOT NULL OR NEW.root IS NOT NULL OR NEW.subagents_enabled <> 0 OR NEW.model IS NOT NULL OR NEW.thinking IS NOT NULL)
BEGIN SELECT RAISE(ABORT, 'immutable CLI session'); END;
-- Permanent, nonsecret UUID ownership receipts. Deliberately no session foreign key:
-- removal deletes the binding/history, never permission to reuse an old CLI context.
CREATE TABLE cli_binding_receipts (
 cli_session_id TEXT PRIMARY KEY NOT NULL CHECK(
  length(cli_session_id)=36 AND
  substr(cli_session_id,9,1)='-' AND substr(cli_session_id,14,1)='-' AND
  substr(cli_session_id,19,1)='-' AND substr(cli_session_id,24,1)='-' AND
  length(replace(cli_session_id,'-',''))=32 AND
  replace(cli_session_id,'-','') NOT GLOB '*[^0-9a-f]*'),
 session TEXT NOT NULL UNIQUE CHECK(length(session) BETWEEN 1 AND 128 AND session NOT GLOB '*[^a-zA-Z0-9_.:-]*')
) STRICT;
CREATE TRIGGER cli_receipt_shape BEFORE INSERT ON cli_binding_receipts
WHEN EXISTS(SELECT 1 FROM cli_binding_receipts WHERE cli_session_id=NEW.cli_session_id OR session=NEW.session)
 OR NOT EXISTS(SELECT 1 FROM cli_bindings WHERE session=NEW.session AND cli_session_id=NEW.cli_session_id)
BEGIN SELECT RAISE(ABORT, 'invalid CLI receipt'); END;
CREATE TRIGGER cli_receipt_immutable BEFORE UPDATE ON cli_binding_receipts
BEGIN SELECT RAISE(ABORT, 'immutable CLI receipt'); END;
CREATE TRIGGER cli_receipt_permanent BEFORE DELETE ON cli_binding_receipts
BEGIN SELECT RAISE(ABORT, 'permanent CLI receipt'); END;
CREATE TABLE cli_bindings (
 session TEXT PRIMARY KEY REFERENCES sessions(id),
 cli_session_id TEXT NOT NULL UNIQUE CHECK(length(cli_session_id)=36),
 project_path TEXT NOT NULL CHECK(length(project_path) BETWEEN 1 AND 4096),
 executable_path TEXT NOT NULL CHECK(length(executable_path) BETWEEN 1 AND 4096),
 cli_version TEXT NOT NULL CHECK(length(cli_version) BETWEEN 1 AND 128),
 config_ref TEXT NOT NULL CHECK(length(config_ref) BETWEEN 1 AND 128),
 account_scope TEXT NOT NULL CHECK(length(account_scope) BETWEEN 1 AND 128),
 launch_policy TEXT NOT NULL CHECK(length(launch_policy) BETWEEN 1 AND 128),
 generation INTEGER NOT NULL DEFAULT 1 CHECK(generation > 0),
 context_valid INTEGER NOT NULL DEFAULT 1 CHECK(context_valid IN (0,1))
) STRICT;
CREATE TRIGGER cli_binding_shape BEFORE INSERT ON cli_bindings
WHEN NOT EXISTS(SELECT 1 FROM sessions WHERE id=NEW.session AND execution_backend='localClaudeCli' AND cwd=NEW.project_path AND create_key IS NOT NULL)
BEGIN SELECT RAISE(ABORT, 'invalid CLI binding'); END;
-- The first binding consumes the UUID even if no run is ever admitted. A reused UUID
-- fails the receipt insert and rolls back the entire binding/session creation.
CREATE TRIGGER cli_binding_receipt AFTER INSERT ON cli_bindings
BEGIN
 INSERT INTO cli_binding_receipts(cli_session_id,session) VALUES(NEW.cli_session_id,NEW.session);
END;
CREATE TRIGGER cli_binding_immutable BEFORE UPDATE ON cli_bindings
WHEN NEW.session <> OLD.session OR NEW.cli_session_id <> OLD.cli_session_id OR
 NEW.project_path <> OLD.project_path OR NEW.executable_path <> OLD.executable_path OR
 NEW.cli_version <> OLD.cli_version OR NEW.config_ref <> OLD.config_ref OR
 NEW.account_scope <> OLD.account_scope OR NEW.launch_policy <> OLD.launch_policy OR
 NOT ((NEW.generation=OLD.generation AND NEW.context_valid=OLD.context_valid) OR
      (OLD.context_valid=1 AND NEW.context_valid=0 AND NEW.generation=OLD.generation+1))
BEGIN SELECT RAISE(ABORT, 'immutable CLI binding'); END;
CREATE TABLE external_runs (
 session TEXT NOT NULL REFERENCES cli_bindings(session),
 generation INTEGER NOT NULL CHECK(generation > 0),
 op TEXT NOT NULL CHECK(length(op) BETWEEN 1 AND 128),
 state TEXT NOT NULL CHECK(state IN ('admitted','launchIntent','running','stopping','completed','interrupted','unknown')),
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL,
 PRIMARY KEY(session,generation,op),
 UNIQUE(session,op)
) STRICT;
CREATE UNIQUE INDEX external_run_active ON external_runs(session)
 WHERE state IN ('admitted','launchIntent','running','stopping');
CREATE TABLE external_actions (
 session TEXT NOT NULL,
 generation INTEGER NOT NULL,
 op TEXT NOT NULL,
 call_id TEXT NOT NULL CHECK(length(call_id) BETWEEN 1 AND 128),
 tool_name TEXT NOT NULL CHECK(length(tool_name) BETWEEN 1 AND 128),
 status TEXT NOT NULL CHECK(status IN ('observed','running','completed','failed','unknown')),
 provenance TEXT NOT NULL DEFAULT 'localClaudeCli' CHECK(provenance='localClaudeCli'),
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL,
 PRIMARY KEY(session,generation,op,call_id),
 FOREIGN KEY(session,generation,op) REFERENCES external_runs(session,generation,op)
) STRICT;
CREATE TRIGGER external_run_completion_settled BEFORE UPDATE OF state ON external_runs
WHEN NEW.state='completed' AND EXISTS(
 SELECT 1 FROM external_actions WHERE session=NEW.session AND generation=NEW.generation
 AND op=NEW.op AND status NOT IN ('completed','failed'))
BEGIN SELECT RAISE(ABORT, 'unsettled external actions'); END;
