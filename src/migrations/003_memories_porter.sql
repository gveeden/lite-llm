-- Recreate the FTS5 table with porter stemming so that e.g. "lights" matches
-- memories containing "light", and vice versa.

DROP TRIGGER IF EXISTS memories_ad;
DROP TRIGGER IF EXISTS memories_ai;
DROP TABLE IF EXISTS memories_fts;

CREATE VIRTUAL TABLE memories_fts USING fts5(
    content,
    content=memories,
    content_rowid=id,
    tokenize='porter ascii'
);

-- Repopulate from existing rows.
INSERT INTO memories_fts(rowid, content) SELECT id, content FROM memories;

CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content) VALUES ('delete', old.id, old.content);
END;
