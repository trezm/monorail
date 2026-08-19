-- A user is a Railway account this service has seen. `railway_user_id` is the
-- `sub` claim, the only one Railway guarantees, so it is what identity is keyed
-- on rather than email -- which is absent unless the `email` scope was granted,
-- and mutable even when it is.
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    railway_user_id TEXT NOT NULL UNIQUE,
    email TEXT,
    name TEXT,
    avatar_url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A logged-in browser. `token_hash` is SHA-256 of the value in the cookie and
-- the cookie value itself is never stored, so a dump of this table yields no
-- live session.
--
-- The Railway tokens are stored as-is. They are bearer credentials for a third
-- party, so this table is as sensitive as the database is; encrypting the
-- columns is the obvious next step and needs no schema change beyond widening
-- the type.
CREATE TABLE sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash BYTEA NOT NULL UNIQUE,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    access_token TEXT NOT NULL,
    refresh_token TEXT,
    scope TEXT NOT NULL,
    access_token_expires_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Logging out everywhere, and cascading a user delete, both scan by user.
CREATE INDEX sessions_user_id_idx ON sessions (user_id);

-- Expired rows are swept by whatever runs the cleanup, and every lookup filters
-- on this column anyway.
CREATE INDEX sessions_expires_at_idx ON sessions (expires_at);
