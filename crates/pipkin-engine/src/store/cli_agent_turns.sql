-- Root receipts survive removal; no prompt, result body, executable path or secret is retained here.
CREATE TABLE cli_agent_turns (
 session TEXT NOT NULL, request_key TEXT NOT NULL,
 generation INTEGER NOT NULL CHECK(generation>0), operation TEXT NOT NULL,
 run INTEGER NOT NULL UNIQUE CHECK(run>0), owner TEXT NOT NULL,
 binding_hash BLOB NOT NULL CHECK(length(binding_hash)=32),
 fingerprint BLOB NOT NULL CHECK(length(fingerprint)=32), user_entry INTEGER NOT NULL,
 mode TEXT NOT NULL CHECK(mode IN ('fresh','resume')),
 state TEXT NOT NULL CHECK(state IN ('admitted','launchIntent','running','completed','rejected','unknown')),
 bridge TEXT, evidence TEXT CHECK(evidence IS NULL OR json_valid(evidence)),
 fenced INTEGER NOT NULL DEFAULT 0 CHECK(fenced IN (0,1)),
 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
 PRIMARY KEY(session,request_key), UNIQUE(session,generation,operation),
 CHECK((state='completed')=(evidence IS NOT NULL)),
 CHECK(state NOT IN ('launchIntent','running','completed') OR bridge IS NOT NULL)
) STRICT;
CREATE UNIQUE INDEX cli_agent_one_active_turn ON cli_agent_turns(session)
 WHERE state IN ('admitted','launchIntent','running');
CREATE TRIGGER cli_agent_turn_shape BEFORE INSERT ON cli_agent_turns
WHEN NEW.state<>'admitted' OR NEW.bridge IS NOT NULL OR NEW.fenced<>0 OR NOT EXISTS(
 SELECT 1 FROM cli_agent_bindings b JOIN sessions s ON s.id=b.session
 JOIN requests q ON q.session=s.id JOIN runs r ON r.session=s.id
 JOIN entries e ON e.session=s.id AND e.run=r.id
 WHERE b.session=NEW.session AND b.digest=NEW.binding_hash AND s.execution_backend='claudeCliAgent'
 AND q.key=NEW.request_key AND q.run=NEW.run AND q.state='running'
 AND r.id=NEW.run AND r.request=q.key AND r.state='running' AND r.stop_requested_at IS NULL
 AND e.id=NEW.user_entry AND s.blocked IS NULL)
BEGIN SELECT RAISE(ABORT,'invalid native CLI turn'); END;
CREATE TRIGGER cli_agent_turn_immutable BEFORE UPDATE ON cli_agent_turns
WHEN NEW.session<>OLD.session OR NEW.request_key<>OLD.request_key OR NEW.generation<>OLD.generation
 OR NEW.operation<>OLD.operation OR NEW.run<>OLD.run OR NEW.owner<>OLD.owner
 OR NEW.binding_hash<>OLD.binding_hash OR NEW.fingerprint<>OLD.fingerprint OR NEW.user_entry<>OLD.user_entry
 OR NEW.mode<>OLD.mode OR NEW.created_at<>OLD.created_at OR NEW.updated_at<OLD.updated_at OR NEW.fenced<OLD.fenced
 OR NOT (NEW.bridge IS OLD.bridge OR OLD.state='admitted' AND NEW.state='launchIntent' AND OLD.bridge IS NULL AND NEW.bridge IS NOT NULL)
 OR NOT (NEW.state=OLD.state AND NEW.evidence IS OLD.evidence
  OR OLD.state='admitted' AND NEW.state IN ('launchIntent','rejected')
  OR OLD.state='launchIntent' AND NEW.state IN ('running','unknown')
  OR OLD.state='running' AND NEW.state IN ('completed','unknown'))
BEGIN SELECT RAISE(ABORT,'immutable native CLI turn'); END;
CREATE TRIGGER cli_agent_turn_launch_scope BEFORE UPDATE ON cli_agent_turns
WHEN NEW.state<>OLD.state AND NEW.state IN ('launchIntent','running','completed') AND (
 NEW.fenced<>0 OR NOT EXISTS(SELECT 1 FROM runs r JOIN sessions s ON s.id=r.session
  JOIN cli_agent_bindings b ON b.session=s.id WHERE r.id=NEW.run AND r.session=NEW.session
  AND r.state='running' AND r.stop_requested_at IS NULL AND s.blocked IS NULL AND b.digest=NEW.binding_hash
  AND NOT EXISTS(SELECT 1 FROM session_removals m WHERE m.root=s.id OR m.root=s.root))
 OR NOT EXISTS(SELECT 1 FROM cli_tool_bridges b WHERE b.id=NEW.bridge AND b.session=NEW.session
  AND b.run=NEW.run AND b.owner=NEW.owner AND b.generation=NEW.generation AND b.operation=NEW.operation
  AND (NEW.state='launchIntent' AND b.state='prepared' OR NEW.state='running' AND b.state IN ('prepared','connected')
   OR NEW.state='completed' AND b.state='closed')))
BEGIN SELECT RAISE(ABORT,'invalid native CLI launch transition'); END;
CREATE TRIGGER cli_agent_turn_permanent BEFORE DELETE ON cli_agent_turns
BEGIN SELECT RAISE(ABORT,'permanent native CLI turn receipt'); END;
