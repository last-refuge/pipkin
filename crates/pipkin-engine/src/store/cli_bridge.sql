-- Inactive orchestration rollout: no public creator or native execution grant.
-- Identity receipts deliberately survive session/history removal, like CLI UUID receipts.
CREATE TABLE cli_tool_bridges (
 id TEXT PRIMARY KEY CHECK(length(id) BETWEEN 1 AND 128),
 session TEXT NOT NULL,
 generation INTEGER NOT NULL CHECK(generation > 0),
 operation TEXT NOT NULL,
 run INTEGER NOT NULL CHECK(run > 0),
 owner TEXT NOT NULL,
 binding_hash BLOB NOT NULL CHECK(length(binding_hash)=32),
 capability_hash BLOB NOT NULL CHECK(length(capability_hash)=32),
 policy TEXT NOT NULL CHECK(policy='cli-native-orchestration-v1'),
 state TEXT NOT NULL CHECK(state IN ('prepared','connected','closed','fenced')),
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL,
 UNIQUE(session,generation,operation),
 UNIQUE(run)
) STRICT;
CREATE INDEX cli_tool_bridges_active ON cli_tool_bridges(state)
 WHERE state IN ('prepared','connected');
CREATE TRIGGER cli_tool_bridge_shape BEFORE INSERT ON cli_tool_bridges
WHEN NEW.state <> 'prepared' OR NOT EXISTS (
 SELECT 1 FROM sessions s JOIN runs r ON r.session=s.id
 WHERE s.id=NEW.session AND s.execution_backend='claudeCliAgent' AND s.blocked IS NULL
 AND r.id=NEW.run AND r.state='running' AND r.stop_requested_at IS NULL
 AND NOT EXISTS(SELECT 1 FROM session_removals m WHERE m.root=s.id OR m.root=s.root)
 AND NOT EXISTS(SELECT 1 FROM sessions a WHERE a.id IN (s.parent,s.root) AND a.execution_backend<>'claudeCliAgent'))
BEGIN SELECT RAISE(ABORT,'invalid CLI orchestration scope'); END;
CREATE TRIGGER cli_tool_bridge_immutable BEFORE UPDATE ON cli_tool_bridges
WHEN NEW.id<>OLD.id OR NEW.session<>OLD.session OR NEW.generation<>OLD.generation
 OR NEW.operation<>OLD.operation OR NEW.run<>OLD.run OR NEW.owner<>OLD.owner
 OR NEW.binding_hash<>OLD.binding_hash OR NEW.capability_hash<>OLD.capability_hash
 OR NEW.policy<>OLD.policy OR NEW.created_at<>OLD.created_at OR NEW.updated_at<OLD.updated_at
 OR NOT(NEW.state=OLD.state OR (OLD.state='prepared' AND NEW.state IN ('connected','fenced'))
 OR (OLD.state='connected' AND NEW.state IN ('closed','fenced')))
BEGIN SELECT RAISE(ABORT,'immutable CLI orchestration identity'); END;
CREATE TRIGGER cli_tool_bridge_permanent BEFORE DELETE ON cli_tool_bridges
BEGIN SELECT RAISE(ABORT,'permanent CLI orchestration receipt'); END;

CREATE TABLE cli_bridge_calls (
 bridge TEXT NOT NULL REFERENCES cli_tool_bridges(id),
 session TEXT NOT NULL,
 rpc_key TEXT NOT NULL CHECK(json_valid(rpc_key)),
 call_id TEXT NOT NULL,
 fingerprint BLOB NOT NULL CHECK(length(fingerprint)=32),
 name TEXT NOT NULL,
 arguments TEXT NOT NULL CHECK(json_valid(arguments)),
 state TEXT NOT NULL CHECK(state IN ('intent','completed','unknown')),
 result TEXT CHECK(result IS NULL OR json_valid(result)),
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL,
 PRIMARY KEY(bridge,rpc_key),
 UNIQUE(session,call_id),
 CHECK((state='completed')=(result IS NOT NULL))
) STRICT;
CREATE TRIGGER cli_bridge_call_shape BEFORE INSERT ON cli_bridge_calls
WHEN NEW.state<>'intent' OR NOT EXISTS(
 SELECT 1 FROM cli_tool_bridges b JOIN runs r ON r.id=b.run JOIN sessions s ON s.id=b.session
 WHERE b.id=NEW.bridge AND b.session=NEW.session AND b.state='connected'
 AND s.execution_backend='claudeCliAgent' AND s.blocked IS NULL
 AND r.session=s.id AND r.state='running' AND r.stop_requested_at IS NULL
 AND NOT EXISTS(SELECT 1 FROM session_removals m WHERE m.root=s.id OR m.root=s.root))
BEGIN SELECT RAISE(ABORT,'invalid CLI bridge call'); END;
CREATE TRIGGER cli_bridge_call_immutable BEFORE UPDATE ON cli_bridge_calls
WHEN NEW.bridge<>OLD.bridge OR NEW.session<>OLD.session OR NEW.rpc_key<>OLD.rpc_key
 OR NEW.call_id<>OLD.call_id OR NEW.fingerprint<>OLD.fingerprint OR NEW.name<>OLD.name
 OR NEW.arguments<>OLD.arguments OR NEW.created_at<>OLD.created_at OR NEW.updated_at<OLD.updated_at
 OR NOT((OLD.state='intent' AND NEW.state IN ('completed','unknown'))
 OR (NEW.state=OLD.state AND NEW.result IS OLD.result))
 OR (NEW.state='completed' AND OLD.state='intent' AND NOT EXISTS(
 SELECT 1 FROM cli_tool_bridges b JOIN runs r ON r.id=b.run
 WHERE b.id=NEW.bridge AND b.state='connected' AND r.state='running' AND r.stop_requested_at IS NULL))
BEGIN SELECT RAISE(ABORT,'immutable CLI bridge call'); END;
