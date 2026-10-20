-- Recovery records authorize a new binding generation, never replay or rewrite a prompt.
CREATE TABLE cli_stop_recoveries (
 prompt INTEGER PRIMARY KEY REFERENCES external_prompts(id) ON DELETE CASCADE,
 session TEXT NOT NULL REFERENCES cli_bindings(session),
 cli_session_id TEXT NOT NULL,
 previous_generation INTEGER NOT NULL,
 generation INTEGER NOT NULL CHECK(generation=previous_generation+1),
 setup TEXT NOT NULL CHECK(length(setup) BETWEEN 1 AND 256),
 recovered_at INTEGER NOT NULL,
 UNIQUE(session,generation)
) STRICT;
CREATE TRIGGER cli_stop_recovery_immutable BEFORE UPDATE ON cli_stop_recoveries
BEGIN SELECT RAISE(ABORT, 'immutable CLI recovery'); END;
CREATE TRIGGER cli_stop_recovery_shape BEFORE INSERT ON cli_stop_recoveries
WHEN NOT EXISTS (
 SELECT 1 FROM cli_bindings b JOIN external_prompts p ON p.session=b.session
 JOIN external_runs r ON r.session=p.session AND r.generation=p.generation AND r.op=p.op
 WHERE p.id=NEW.prompt AND b.session=NEW.session AND b.cli_session_id=NEW.cli_session_id
 AND b.context_valid=0 AND b.generation=NEW.previous_generation AND b.generation=p.generation+1
 AND b.launch_policy='cli-unattended-coding-v1' AND r.state='interrupted' AND p.cursor>0
 AND json_extract(p.outcome,'$.reason') IS 'aborted'
 AND json_extract(p.result,'$.status') IS 'unanswered' AND json_extract(p.result,'$.reason') IS 'aborted'
 AND json_type(p.evidence,'$.continuation_authorized') IS NULL
 AND json_extract(p.evidence,'$.input_delivered') IS 1
 AND json_extract(p.evidence,'$.root_reaped') IS 1 AND json_extract(p.evidence,'$.pipes_drained') IS 1
 AND json_extract(p.evidence,'$.cleanup_timed_out') IS 0
 AND json_extract(p.evidence,'$.stop_requested') IS 1
 AND json_extract(p.evidence,'$.cancelled_clean_confirmed') IS 1
 AND json_extract(p.evidence,'$.supervisor_stop') IS 'cancelled'
 AND json_extract(p.evidence,'$.parse_failure') IS NULL
 AND (json_extract(p.evidence,'$.projection_error') IS NULL OR json_extract(p.evidence,'$.projection_error') IS 'Incomplete')
 AND (json_extract(p.evidence,'$.rejected_record') IS NULL OR json_extract(p.evidence,'$.rejected_record') IS 'end_of_stream')
 AND NOT EXISTS(SELECT 1 FROM external_prompts newer WHERE newer.session=b.session AND newer.id>p.id)
)
BEGIN SELECT RAISE(ABORT, 'invalid CLI recovery'); END;
DROP TRIGGER cli_binding_immutable;
CREATE TRIGGER cli_binding_immutable BEFORE UPDATE ON cli_bindings
WHEN NEW.session <> OLD.session OR NEW.cli_session_id <> OLD.cli_session_id OR
 NEW.project_path <> OLD.project_path OR NEW.executable_path <> OLD.executable_path OR
 NEW.cli_version <> OLD.cli_version OR NEW.config_ref <> OLD.config_ref OR
 NEW.account_scope <> OLD.account_scope OR NEW.launch_policy <> OLD.launch_policy OR
 NOT ((NEW.generation=OLD.generation AND NEW.context_valid=OLD.context_valid) OR
      (OLD.context_valid=1 AND NEW.context_valid=0 AND NEW.generation=OLD.generation+1) OR
      (OLD.context_valid=0 AND NEW.context_valid=1 AND NEW.generation=OLD.generation+1 AND EXISTS(
         SELECT 1 FROM cli_stop_recoveries a WHERE a.session=OLD.session
         AND a.cli_session_id=OLD.cli_session_id AND a.previous_generation=OLD.generation
         AND a.generation=NEW.generation)))
BEGIN SELECT RAISE(ABORT, 'immutable CLI binding'); END;
