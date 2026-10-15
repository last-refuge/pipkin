-- Source tool IDs correlate CLI observations/results only, never approval receipts.
-- Entry/block anchors keep mixed text/action streams indexed and bounded.
CREATE TABLE external_projection_actions (
 prompt INTEGER NOT NULL REFERENCES external_prompts(id),
 session TEXT NOT NULL,
 tool_id TEXT NOT NULL CHECK(length(CAST(tool_id AS BLOB)) BETWEEN 1 AND 128),
 entry INTEGER NOT NULL,
 block_index INTEGER NOT NULL CHECK(block_index BETWEEN 0 AND 255),
 PRIMARY KEY(prompt,tool_id),
 UNIQUE(session,entry,block_index),
 FOREIGN KEY(session,entry) REFERENCES external_projection_messages(session,entry)
) STRICT;
