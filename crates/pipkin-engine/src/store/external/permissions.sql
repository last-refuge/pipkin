-- External authorization only; consumed does NOT attest tool execution or completion.
CREATE TABLE external_permissions (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 engine_epoch INTEGER NOT NULL CHECK(engine_epoch > 0),
 owner TEXT NOT NULL CHECK(length(owner) BETWEEN 1 AND 128),
 session TEXT NOT NULL,
 generation INTEGER NOT NULL,
 op TEXT NOT NULL,
 prompt INTEGER NOT NULL REFERENCES external_prompts(id) ON DELETE CASCADE,
 invocation_nonce TEXT NOT NULL CHECK(length(invocation_nonce) BETWEEN 1 AND 128),
 arguments_sha256 TEXT NOT NULL CHECK(length(arguments_sha256)=64 AND arguments_sha256 NOT GLOB '*[^0-9a-f]*'),
 tool_name TEXT NOT NULL CHECK(tool_name='Write'),
 policy_id TEXT NOT NULL CHECK(policy_id='restricted-write-approval-v1'),
 expires_at INTEGER NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','granted','consumed','denied','expired','revoked')),
 decision TEXT CHECK(decision IN ('allowOnce','deny')),
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL,
 CHECK(expires_at > created_at),
 CHECK((state='pending' AND decision IS NULL) OR
       (state IN ('granted','consumed') AND decision IS 'allowOnce') OR
       (state='denied' AND decision IS 'deny') OR state IN ('expired','revoked')),
 UNIQUE(engine_epoch,owner,session,generation,op,invocation_nonce),
 FOREIGN KEY(session,generation,op) REFERENCES external_runs(session,generation,op)
) STRICT;
CREATE INDEX external_permissions_run ON external_permissions(session,generation,op,id);
CREATE TRIGGER external_permission_scope_immutable BEFORE UPDATE ON external_permissions
WHEN NEW.id<>OLD.id OR NEW.engine_epoch<>OLD.engine_epoch OR NEW.owner<>OLD.owner
 OR NEW.session<>OLD.session OR NEW.generation<>OLD.generation OR NEW.op<>OLD.op
 OR NEW.prompt<>OLD.prompt OR NEW.invocation_nonce<>OLD.invocation_nonce
 OR NEW.arguments_sha256<>OLD.arguments_sha256 OR NEW.tool_name<>OLD.tool_name
 OR NEW.policy_id<>OLD.policy_id OR NEW.expires_at<>OLD.expires_at OR NEW.created_at<>OLD.created_at
 OR (OLD.decision IS NOT NULL AND NEW.decision IS NOT OLD.decision)
BEGIN SELECT RAISE(ABORT,'immutable external permission scope'); END;
CREATE TRIGGER external_permission_transition BEFORE UPDATE OF state ON external_permissions
WHEN NEW.state<>OLD.state AND NOT (
 (OLD.state='pending' AND NEW.state IN ('granted','denied','expired','revoked')) OR
 (OLD.state='granted' AND NEW.state IN ('consumed','expired','revoked')))
BEGIN SELECT RAISE(ABORT,'invalid external permission transition'); END;
