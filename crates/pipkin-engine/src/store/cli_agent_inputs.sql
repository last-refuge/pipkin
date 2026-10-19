-- Data only: no persisted input can reconstruct its connected ticket.
CREATE TABLE cli_agent_inputs (
 session TEXT NOT NULL, request_key TEXT NOT NULL, ticket TEXT NOT NULL,
 generation INTEGER NOT NULL CHECK(generation>0), operation TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('followUp','steer','compact')),
 PRIMARY KEY(session,request_key),
 FOREIGN KEY(session,request_key) REFERENCES requests(session,key) ON DELETE CASCADE
) STRICT;
CREATE TABLE cli_agent_context (
 run INTEGER PRIMARY KEY, tokens INTEGER CHECK(tokens>=0),
 compact_count INTEGER NOT NULL DEFAULT 0 CHECK(compact_count>=0),
 pre_tokens INTEGER CHECK(pre_tokens>=0)
) STRICT;
CREATE TRIGGER cli_agent_input_immutable BEFORE UPDATE ON cli_agent_inputs
BEGIN SELECT RAISE(ABORT,'immutable CLI input intent'); END;
