-- Storage identity only. Does not enable an orchestration launcher or native API inference.
-- One permanent UUID namespace covers existing CLI identities, including removed histories.
CREATE TABLE cli_uuid_owners (
 cli_session_id TEXT PRIMARY KEY NOT NULL,
 session TEXT NOT NULL UNIQUE,
 create_key TEXT UNIQUE,
 backend TEXT NOT NULL CHECK(backend IN ('localClaudeCli','claudeCliAgent'))
) STRICT;
INSERT INTO cli_uuid_owners SELECT r.cli_session_id,r.session,COALESCE(s.create_key,m.create_key),'localClaudeCli'
 FROM cli_binding_receipts r LEFT JOIN sessions s ON s.id=r.session LEFT JOIN session_removals m ON m.root=r.session;
CREATE TRIGGER cli_uuid_owner_immutable BEFORE UPDATE ON cli_uuid_owners
BEGIN SELECT RAISE(ABORT,'immutable CLI UUID owner'); END;
CREATE TRIGGER cli_uuid_owner_permanent BEFORE DELETE ON cli_uuid_owners
BEGIN SELECT RAISE(ABORT,'permanent CLI UUID owner'); END;
CREATE TRIGGER cli_uuid_session_reserved BEFORE INSERT ON sessions
WHEN EXISTS(SELECT 1 FROM cli_uuid_owners WHERE session=NEW.id OR create_key=NEW.create_key)
BEGIN SELECT RAISE(ABORT,'reserved CLI session identity'); END;
CREATE TRIGGER cli_legacy_global_owner AFTER INSERT ON cli_bindings
BEGIN INSERT INTO cli_uuid_owners SELECT NEW.cli_session_id,NEW.session,create_key,'localClaudeCli' FROM sessions WHERE id=NEW.session; END;

CREATE TABLE cli_agent_bindings (
 session TEXT PRIMARY KEY NOT NULL REFERENCES sessions(id),
 cli_session_id TEXT NOT NULL UNIQUE CHECK(length(cli_session_id)=36),
 project_path TEXT NOT NULL,
 executable_path TEXT NOT NULL,
 cli_version TEXT NOT NULL,
 config_ref TEXT NOT NULL,
 account_scope TEXT NOT NULL,
 launch_policy TEXT NOT NULL CHECK(launch_policy='cli-native-orchestration-v1'),
 digest BLOB NOT NULL CHECK(length(digest)=32)
) STRICT;
CREATE TRIGGER cli_agent_binding_shape BEFORE INSERT ON cli_agent_bindings
WHEN NOT EXISTS(SELECT 1 FROM sessions s WHERE s.id=NEW.session AND s.execution_backend='claudeCliAgent'
 AND s.cwd=NEW.project_path AND s.create_key IS NOT NULL AND s.model IN ('haiku','sonnet','opus')
 AND (s.parent IS NULL AND s.root IS NULL OR EXISTS(SELECT 1 FROM sessions p JOIN cli_agent_bindings b ON b.session=p.id
  WHERE p.id=s.parent AND p.execution_backend='claudeCliAgent' AND p.subagents_enabled=1 AND p.blocked IS NULL
  AND s.root=COALESCE(p.root,p.id) AND b.project_path=NEW.project_path AND b.executable_path=NEW.executable_path
  AND b.cli_version=NEW.cli_version AND b.config_ref=NEW.config_ref AND b.account_scope=NEW.account_scope
  AND b.launch_policy=NEW.launch_policy)))
 OR EXISTS(SELECT 1 FROM cli_uuid_owners WHERE session=NEW.session OR cli_session_id=NEW.cli_session_id)
BEGIN SELECT RAISE(ABORT,'invalid orchestration binding'); END;
CREATE TRIGGER cli_agent_binding_immutable BEFORE UPDATE ON cli_agent_bindings
BEGIN SELECT RAISE(ABORT,'immutable orchestration binding'); END;
CREATE TRIGGER cli_agent_global_owner AFTER INSERT ON cli_agent_bindings
BEGIN INSERT INTO cli_uuid_owners SELECT NEW.cli_session_id,NEW.session,create_key,'claudeCliAgent' FROM sessions WHERE id=NEW.session; END;
CREATE TRIGGER cli_uuid_owner_shape BEFORE INSERT ON cli_uuid_owners
WHEN NOT EXISTS(SELECT 1 FROM sessions WHERE id=NEW.session AND create_key=NEW.create_key)
 OR NOT (NEW.backend='localClaudeCli' AND EXISTS(SELECT 1 FROM cli_bindings WHERE session=NEW.session AND cli_session_id=NEW.cli_session_id)
 OR NEW.backend='claudeCliAgent' AND EXISTS(SELECT 1 FROM cli_agent_bindings WHERE session=NEW.session AND cli_session_id=NEW.cli_session_id))
BEGIN SELECT RAISE(ABORT,'unbound CLI UUID owner'); END;
CREATE TRIGGER cli_agent_session_immutable BEFORE UPDATE ON sessions
WHEN OLD.execution_backend='claudeCliAgent' AND
 (NEW.cwd IS NOT OLD.cwd OR NEW.create_key IS NOT OLD.create_key OR NEW.model IS NOT OLD.model
 OR NEW.thinking IS NOT OLD.thinking OR NEW.parent IS NOT OLD.parent OR NEW.root IS NOT OLD.root)
BEGIN SELECT RAISE(ABORT,'immutable orchestration session'); END;
