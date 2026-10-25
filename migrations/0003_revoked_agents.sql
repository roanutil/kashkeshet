-- When the operator revoked an agent's token. A revoked token is refused
-- like an unknown one; the agent's name and messages are kept. NULL for
-- agents still in use.
ALTER TABLE agents ADD COLUMN revoked_at TEXT;
