-- The migration diesel-cli generates for a fresh project, checked in verbatim
-- so `diesel migration generate` and this repo agree on migration `00000000000000`.
--
-- It installs a trigger function for `updated_at` columns. A table opts in with:
--
--     SELECT diesel_manage_updated_at('some_table');
--
-- after which any UPDATE that does not itself touch `updated_at` gets the
-- current transaction timestamp written into it.

CREATE OR REPLACE FUNCTION diesel_manage_updated_at(_tbl regclass) RETURNS VOID AS $$
BEGIN
    EXECUTE format('CREATE TRIGGER set_updated_at BEFORE UPDATE ON %s
                    FOR EACH ROW EXECUTE PROCEDURE diesel_set_updated_at()', _tbl);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION diesel_set_updated_at() RETURNS trigger AS $$
BEGIN
    IF (
        NEW IS DISTINCT FROM OLD AND
        NEW.updated_at IS NOT DISTINCT FROM OLD.updated_at
    ) THEN
        NEW.updated_at := current_timestamp;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
