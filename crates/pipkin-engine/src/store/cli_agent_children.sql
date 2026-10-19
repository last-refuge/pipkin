-- Native child snapshots are deliberately outside subagent_tasks.launch: ordinary views
-- must not load the complete instruction/context snapshot on each task-list refresh.
CREATE TABLE cli_agent_child_policies (
 child TEXT PRIMARY KEY NOT NULL REFERENCES cli_agent_bindings(session) ON DELETE CASCADE,
 parent TEXT NOT NULL,
 parent_call TEXT NOT NULL,
 digest BLOB NOT NULL CHECK(length(digest)=32),
 snapshot TEXT NOT NULL CHECK(length(CAST(snapshot AS BLOB))<=2097152),
 UNIQUE(parent,parent_call)
) STRICT;
CREATE TRIGGER cli_agent_child_policy_shape BEFORE INSERT ON cli_agent_child_policies
WHEN NOT EXISTS(SELECT 1 FROM sessions s JOIN subagent_tasks t ON t.child_session=s.id
 WHERE s.id=NEW.child AND s.parent=NEW.parent AND s.root=NEW.parent
 AND s.execution_backend='claudeCliAgent' AND s.subagents_enabled=0
 AND s.create_key='cli-child-' || lower(hex(NEW.digest))
 AND t.root=NEW.parent AND t.parent_session=NEW.parent AND t.parent_call=NEW.parent_call
 AND t.origin='model' AND t.launch IS NOT NULL)
BEGIN SELECT RAISE(ABORT,'invalid native child policy'); END;
CREATE TRIGGER cli_agent_child_policy_immutable BEFORE UPDATE ON cli_agent_child_policies
BEGIN SELECT RAISE(ABORT,'immutable native child policy'); END;
CREATE TRIGGER cli_agent_child_policy_retained BEFORE DELETE ON cli_agent_child_policies
WHEN EXISTS(SELECT 1 FROM cli_agent_bindings WHERE session=OLD.child)
BEGIN SELECT RAISE(ABORT,'native child policy belongs to its binding'); END;
