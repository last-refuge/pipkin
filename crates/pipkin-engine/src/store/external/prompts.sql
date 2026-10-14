-- External protocol run IDs deliberately do not reference native runs/requests.
CREATE TABLE external_prompts (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 session TEXT NOT NULL,
 generation INTEGER NOT NULL,
 op TEXT NOT NULL,
 request_key TEXT NOT NULL CHECK(length(CAST(request_key AS BLOB)) BETWEEN 1 AND 256),
 fingerprint TEXT NOT NULL,
 user_entry INTEGER NOT NULL,
 cursor INTEGER NOT NULL DEFAULT 0 CHECK(cursor>=0),
 projected_bytes INTEGER NOT NULL DEFAULT 0 CHECK(projected_bytes BETWEEN 0 AND 1048576),
 mode TEXT NOT NULL CHECK(mode IN ('fresh','resume')),
 terminal TEXT CHECK(terminal IS NULL OR json_valid(terminal)),
 evidence TEXT CHECK(evidence IS NULL OR json_valid(evidence)),
 outcome TEXT CHECK(outcome IS NULL OR json_valid(outcome)),
 result TEXT CHECK(result IS NULL OR json_valid(result)),
 UNIQUE(session,request_key),
 UNIQUE(session,generation,op),
 FOREIGN KEY(session,generation,op) REFERENCES external_runs(session,generation,op),
 FOREIGN KEY(session,user_entry) REFERENCES entries(session,id)
) STRICT;
CREATE TABLE external_projection_messages (
 prompt INTEGER NOT NULL REFERENCES external_prompts(id),
 session TEXT NOT NULL,
 message_id TEXT NOT NULL CHECK(length(CAST(message_id AS BLOB)) BETWEEN 1 AND 128),
 entry INTEGER NOT NULL,
 final_blocks TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(final_blocks)),
 PRIMARY KEY(prompt,message_id),
 UNIQUE(session,entry),
 FOREIGN KEY(session,entry) REFERENCES entries(session,id)
) STRICT;
