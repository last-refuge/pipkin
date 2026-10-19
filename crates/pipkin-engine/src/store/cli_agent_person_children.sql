DROP TRIGGER cli_agent_child_policy_shape;
CREATE TRIGGER cli_agent_child_policy_shape BEFORE INSERT ON cli_agent_child_policies
WHEN NOT EXISTS(SELECT 1 FROM sessions s JOIN subagent_tasks t ON t.child_session=s.id
 WHERE s.id=NEW.child AND s.parent=NEW.parent AND s.root=NEW.parent
 AND s.execution_backend='claudeCliAgent' AND s.subagents_enabled=0
 AND s.create_key='cli-child-' || lower(hex(NEW.digest))
 AND t.root=NEW.parent AND t.parent_session=NEW.parent AND t.parent_call=NEW.parent_call
 AND (t.origin='model' OR (t.origin='user' AND json_extract(NEW.snapshot,'$.cliNative.origin')='user' AND json_extract(NEW.snapshot,'$.cliNative.parent.run')=0)) AND t.launch IS NOT NULL)
BEGIN SELECT RAISE(ABORT,'invalid native child policy'); END;
