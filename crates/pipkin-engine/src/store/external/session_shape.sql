-- Schema 23: permit only immutable fixed CLI aliases in the existing model column.
-- Backend identity keeps these aliases separate from native provider model IDs.
-- NULL remains valid only for legacy creation; any model is immutable after insertion.
DROP TRIGGER cli_session_shape_insert;
DROP TRIGGER cli_session_shape_update;
CREATE TRIGGER cli_session_shape_insert BEFORE INSERT ON sessions
WHEN NEW.execution_backend = 'localClaudeCli' AND
 (NEW.parent IS NOT NULL OR NEW.root IS NOT NULL OR NEW.subagents_enabled <> 0 OR
  (NEW.model IS NOT NULL AND NEW.model NOT IN ('haiku','sonnet','opus')) OR NEW.thinking IS NOT NULL)
BEGIN SELECT RAISE(ABORT, 'invalid CLI session'); END;
CREATE TRIGGER cli_session_shape_update BEFORE UPDATE ON sessions
WHEN OLD.execution_backend = 'localClaudeCli' AND
 (NEW.cwd <> OLD.cwd OR NEW.create_key IS NOT OLD.create_key OR NEW.parent IS NOT NULL OR
  NEW.root IS NOT NULL OR NEW.subagents_enabled <> 0 OR NEW.model IS NOT OLD.model OR NEW.thinking IS NOT NULL)
BEGIN SELECT RAISE(ABORT, 'immutable CLI session'); END;
