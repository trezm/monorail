-- The service and environment are Railway's resources, so their ids are plain
-- text with no local table to reference. `user_id` is whose Railway credential
-- the autoscaling loop acts with, and removing the account removes the rules.
--
-- The key is (service_id, metric): one rule per service and metric, because
-- two rules reading the same signal could only agree or fight — and that
-- identity is the primary key rather than a surrogate id alongside it.
CREATE TABLE horizontal_autoscaling (
    service_id TEXT NOT NULL,
    metric TEXT NOT NULL CHECK (metric IN ('CPU', 'MEMORY', 'NETWORK_RX', 'NETWORK_TX')),
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    environment_id TEXT NOT NULL,
    min_threshold DOUBLE PRECISION NOT NULL CHECK (min_threshold >= 0),
    max_threshold DOUBLE PRECISION NOT NULL,
    poll_frequency_secs INTEGER NOT NULL CHECK (poll_frequency_secs > 0),
    last_checked TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (min_threshold < max_threshold),
    PRIMARY KEY (service_id, metric)
);

CREATE INDEX horizontal_autoscaling_user_id_idx ON horizontal_autoscaling (user_id);
-- The sweep selects by due time, which is a function of these two columns.
CREATE INDEX horizontal_autoscaling_last_checked_idx ON horizontal_autoscaling (last_checked);
