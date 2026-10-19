-- Immutable delivery membership; never a live ticket or a replay instruction.
CREATE TABLE cli_agent_input_deliveries (
 session TEXT NOT NULL, request_key TEXT NOT NULL, run INTEGER NOT NULL,
 entry INTEGER NOT NULL, channel TEXT NOT NULL CHECK(channel IN ('prompt','tool')),
 call_id TEXT, text TEXT NOT NULL,
 PRIMARY KEY(session,request_key),
 FOREIGN KEY(session,request_key) REFERENCES cli_agent_inputs(session,request_key) ON DELETE CASCADE,
 FOREIGN KEY(run) REFERENCES runs(id) ON DELETE CASCADE,
 CHECK((channel='prompt' AND call_id IS NULL) OR (channel='tool' AND call_id IS NOT NULL))
) STRICT;
CREATE TRIGGER cli_agent_delivery_immutable BEFORE UPDATE ON cli_agent_input_deliveries
BEGIN SELECT RAISE(ABORT,'immutable CLI input delivery'); END;
