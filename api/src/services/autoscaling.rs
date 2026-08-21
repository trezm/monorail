//! Horizontal autoscaling rules, and their Postgres store.
//!
//! A rule is local state about a Railway resource: which metric to watch on
//! one service, the band the average should stay inside, and how often to
//! look. Its identity is (service, metric) — two rules reading the same
//! signal could only agree or fight, so that pair is the primary key. The
//! loop that acts on rules is [`crate::autoscaler`]; this module is the
//! storage capability both it and the HTTP layer depend on, behind a trait
//! for the same reason [`SessionStore`](super::session::SessionStore) is.
//!
//! Rules carry the account that created them because the loop has no
//! credential of its own; the session store is what turns that owner into a
//! usable Railway token. Sessions are deliberately not this store's business.

use chrono::{DateTime, Utc};
use diesel::{
    ExpressionMethods as _, QueryDsl as _, Queryable, Selectable, SelectableHelper as _,
    deserialize::{self, FromSql, FromSqlRow},
    expression::AsExpression,
    pg::{Pg, PgValue},
    serialize::{self, Output, ToSql},
    sql_types,
};
use diesel_async::RunQueryDsl as _;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    db::{Database, DbError},
    error::ApiError,
    schema::horizontal_autoscaling,
    services::railway::Measurement,
};

pub type AutoscaleResult<T> = Result<T, AutoscaleError>;

#[derive(Debug, thiserror::Error)]
pub enum AutoscaleError {
    /// The service already has a rule for this metric — the primary key,
    /// surfaced as a conflict the caller resolves by removing the old rule.
    #[error("this service already has a rule for that metric")]
    Duplicate,

    #[error(transparent)]
    Database(#[from] DbError),
}

impl From<diesel::result::Error> for AutoscaleError {
    fn from(error: diesel::result::Error) -> Self {
        match error {
            diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            ) => Self::Duplicate,
            other => Self::Database(DbError::Query(other)),
        }
    }
}

impl From<AutoscaleError> for ApiError {
    fn from(error: AutoscaleError) -> Self {
        match error {
            AutoscaleError::Duplicate => Self::Conflict(error.to_string()),
            AutoscaleError::Database(source) => source.into(),
        }
    }
}

/// The signal a rule watches. Stored and serialized as the `SCREAMING_SNAKE`
/// names, which the migration's CHECK constraint also enumerates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, AsExpression, FromSqlRow)]
#[diesel(sql_type = sql_types::Text)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Metric {
    Cpu,
    Memory,
    NetworkRx,
    NetworkTx,
}

impl Metric {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "MEMORY",
            Self::NetworkRx => "NETWORK_RX",
            Self::NetworkTx => "NETWORK_TX",
        }
    }

    /// The Railway measurement this metric reads. Thresholds are in its unit:
    /// vCPU cores for CPU, gigabytes for the rest.
    #[must_use]
    pub fn measurement(self) -> Measurement {
        match self {
            Self::Cpu => Measurement::CpuUsage,
            Self::Memory => Measurement::MemoryUsageGb,
            Self::NetworkRx => Measurement::NetworkRxGb,
            Self::NetworkTx => Measurement::NetworkTxGb,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("`{0}` is not a known autoscaling metric")]
pub struct UnknownMetric(String);

impl std::str::FromStr for Metric {
    type Err = UnknownMetric;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "CPU" => Ok(Self::Cpu),
            "MEMORY" => Ok(Self::Memory),
            "NETWORK_RX" => Ok(Self::NetworkRx),
            "NETWORK_TX" => Ok(Self::NetworkTx),
            other => Err(UnknownMetric(other.to_owned())),
        }
    }
}

impl ToSql<sql_types::Text, Pg> for Metric {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Pg>) -> serialize::Result {
        <str as ToSql<sql_types::Text, Pg>>::to_sql(self.as_str(), out)
    }
}

impl FromSql<sql_types::Text, Pg> for Metric {
    fn from_sql(bytes: PgValue<'_>) -> deserialize::Result<Self> {
        let value = <String as FromSql<sql_types::Text, Pg>>::from_sql(bytes)?;

        value.parse().map_err(Into::into)
    }
}

/// One autoscaling rule, as stored and as the API returns it. The owner stays
/// off the wire: rules are only ever read through the session that owns them.
#[derive(Debug, Clone, PartialEq, Queryable, Selectable, Serialize)]
#[diesel(table_name = horizontal_autoscaling, check_for_backend(Pg))]
pub struct Rule {
    pub service_id: String,
    pub metric: Metric,
    #[serde(skip)]
    pub user_id: Uuid,
    pub environment_id: String,
    pub min_threshold: f64,
    pub max_threshold: f64,
    pub poll_frequency_secs: i32,
    pub last_checked: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What a caller supplies; the store fills in the rest.
#[derive(Debug, Clone, Deserialize)]
pub struct NewRule {
    pub environment_id: String,
    pub metric: Metric,
    pub min_threshold: f64,
    pub max_threshold: f64,
    pub poll_frequency_secs: i32,
}

/// Creates, reads and removes autoscaling rules.
///
/// One trait for both halves — the HTTP layer's CRUD and the loop's sweep —
/// so detaching the loop into its own service takes the store with it intact.
#[async_trait::async_trait]
pub trait AutoscaleStore: Send + Sync + 'static {
    /// [`AutoscaleError::Duplicate`] when the service already has a rule for
    /// the metric.
    async fn create(&self, owner: Uuid, service_id: &str, rule: NewRule) -> AutoscaleResult<Rule>;

    /// The owner's rules for one service, oldest first.
    async fn list(&self, owner: Uuid, service_id: &str) -> AutoscaleResult<Vec<Rule>>;

    /// `false` when nothing was removed — no such rule, or a rule that is not
    /// the owner's to remove.
    async fn remove(&self, owner: Uuid, service_id: &str, metric: Metric) -> AutoscaleResult<bool>;

    /// Every rule due at `now`: never checked, or checked longer ago than its
    /// poll frequency.
    async fn due(&self, now: DateTime<Utc>) -> AutoscaleResult<Vec<Rule>>;

    /// Stamps a rule as checked, due again one poll frequency from `now`.
    async fn mark_checked(
        &self,
        service_id: &str,
        metric: Metric,
        now: DateTime<Utc>,
    ) -> AutoscaleResult<()>;
}

/// [`AutoscaleStore`] over the application's Postgres pool.
#[derive(Debug, Clone)]
pub struct PgAutoscaleStore {
    database: Database,
}

impl PgAutoscaleStore {
    #[must_use]
    pub fn new(database: Database) -> Self {
        Self { database }
    }
}

#[async_trait::async_trait]
impl AutoscaleStore for PgAutoscaleStore {
    async fn create(&self, owner: Uuid, service_id: &str, rule: NewRule) -> AutoscaleResult<Rule> {
        let mut conn = self.database.conn().await?;

        Ok(diesel::insert_into(horizontal_autoscaling::table)
            .values((
                horizontal_autoscaling::user_id.eq(owner),
                horizontal_autoscaling::service_id.eq(service_id),
                horizontal_autoscaling::environment_id.eq(&rule.environment_id),
                horizontal_autoscaling::metric.eq(rule.metric),
                horizontal_autoscaling::min_threshold.eq(rule.min_threshold),
                horizontal_autoscaling::max_threshold.eq(rule.max_threshold),
                horizontal_autoscaling::poll_frequency_secs.eq(rule.poll_frequency_secs),
            ))
            .returning(Rule::as_returning())
            .get_result(&mut conn)
            .await?)
    }

    async fn list(&self, owner: Uuid, service_id: &str) -> AutoscaleResult<Vec<Rule>> {
        let mut conn = self.database.conn().await?;

        Ok(horizontal_autoscaling::table
            .filter(horizontal_autoscaling::user_id.eq(owner))
            .filter(horizontal_autoscaling::service_id.eq(service_id))
            .order(horizontal_autoscaling::created_at.asc())
            .select(Rule::as_select())
            .load(&mut conn)
            .await?)
    }

    async fn remove(&self, owner: Uuid, service_id: &str, metric: Metric) -> AutoscaleResult<bool> {
        let mut conn = self.database.conn().await?;

        let removed = diesel::delete(
            horizontal_autoscaling::table
                .filter(horizontal_autoscaling::service_id.eq(service_id))
                .filter(horizontal_autoscaling::metric.eq(metric))
                .filter(horizontal_autoscaling::user_id.eq(owner)),
        )
        .execute(&mut conn)
        .await?;

        Ok(removed > 0)
    }

    async fn due(&self, now: DateTime<Utc>) -> AutoscaleResult<Vec<Rule>> {
        let mut conn = self.database.conn().await?;

        // The due predicate lives in SQL because the interval is per-row;
        // diesel has no expression for `column + make_interval(column)`.
        //
        // A single poller is assumed. Running more than one would double-scale
        // every due rule: they need to claim rules rather than read them — a
        // checked-out-at column, or `FOR UPDATE SKIP LOCKED` around the sweep.
        Ok(horizontal_autoscaling::table
            .filter(
                diesel::dsl::sql::<sql_types::Bool>(
                    "last_checked IS NULL OR last_checked + make_interval(secs => poll_frequency_secs) < ",
                )
                .bind::<sql_types::Timestamptz, _>(now),
            )
            .order(horizontal_autoscaling::created_at.asc())
            .select(Rule::as_select())
            .load(&mut conn)
            .await?)
    }

    async fn mark_checked(
        &self,
        service_id: &str,
        metric: Metric,
        now: DateTime<Utc>,
    ) -> AutoscaleResult<()> {
        let mut conn = self.database.conn().await?;

        diesel::update(
            horizontal_autoscaling::table
                .filter(horizontal_autoscaling::service_id.eq(service_id))
                .filter(horizontal_autoscaling::metric.eq(metric)),
        )
        .set((
            horizontal_autoscaling::last_checked.eq(now),
            horizontal_autoscaling::updated_at.eq(now),
        ))
        .execute(&mut conn)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, Utc};

    use super::{AutoscaleError, AutoscaleStore, Metric, NewRule, PgAutoscaleStore};
    use crate::{
        config::Config,
        db::Database,
        secret::Secret,
        services::{
            auth::{RailwayIdentity, TokenSet},
            railway::Measurement,
            session::{PgSessionStore, SessionStore as _},
        },
    };

    #[test]
    fn metrics_round_trip_their_wire_names() {
        for metric in [
            Metric::Cpu,
            Metric::Memory,
            Metric::NetworkRx,
            Metric::NetworkTx,
        ] {
            assert_eq!(metric.as_str().parse::<Metric>().ok(), Some(metric));
            assert_eq!(
                serde_json::to_value(metric).expect("should serialize"),
                serde_json::Value::String(metric.as_str().to_owned())
            );
        }

        assert!("cpu".parse::<Metric>().is_err());
    }

    #[test]
    fn each_metric_reads_its_measurement() {
        assert_eq!(Metric::Cpu.measurement(), Measurement::CpuUsage);
        assert_eq!(Metric::Memory.measurement(), Measurement::MemoryUsageGb);
        assert_eq!(Metric::NetworkRx.measurement(), Measurement::NetworkRxGb);
        assert_eq!(Metric::NetworkTx.measurement(), Measurement::NetworkTxGb);
    }

    /// Exercises the queries against a real Postgres — the only thing that can
    /// catch `schema.rs` drifting from the migration. Ignored for the same
    /// reason the session round-trip is:
    ///
    /// ```text
    /// tools/stack.sh db && bazel run //api:migrate
    /// cargo test -p monorail-api -- --ignored
    /// ```
    #[tokio::test]
    #[ignore = "needs the local Postgres from tools/stack.sh"]
    async fn a_rule_round_trips_through_postgres() {
        let config = Config::from_env().expect("development defaults should parse");
        let database = Database::new(&config);
        let sessions = PgSessionStore::new(database.clone(), TimeDelta::hours(1));
        let store = PgAutoscaleStore::new(database);

        let identity = RailwayIdentity {
            subject: format!("user_{}", crate::secret::random_token()),
            email: None,
            name: None,
            avatar_url: None,
        };
        let tokens = TokenSet {
            access_token: Secret::new("access"),
            refresh_token: Some(Secret::new("refresh")),
            id_token: None,
            scope: "openid".to_owned(),
            expires_at: Utc::now() + TimeDelta::hours(1),
        };

        let (session_token, session) = sessions
            .begin(&identity, tokens)
            .await
            .expect("a session should open");
        let owner = session.user.id;
        let service_id = format!("svc_{}", crate::secret::random_token());

        let new_rule = || NewRule {
            environment_id: "env-1".to_owned(),
            metric: Metric::Cpu,
            min_threshold: 0.1,
            max_threshold: 0.8,
            poll_frequency_secs: 3600,
        };

        let rule = store
            .create(owner, &service_id, new_rule())
            .await
            .expect("a rule should be created");
        assert_eq!(rule.metric, Metric::Cpu);
        assert_eq!(rule.last_checked, None);

        let duplicate = store.create(owner, &service_id, new_rule()).await;
        assert!(matches!(duplicate, Err(AutoscaleError::Duplicate)));

        let listed = store
            .list(owner, &service_id)
            .await
            .expect("listing should succeed");
        assert_eq!(listed, vec![rule.clone()]);

        let now = Utc::now();
        let due = store.due(now).await.expect("the sweep should read");
        assert!(
            due.iter()
                .any(|due| due.service_id == service_id && due.metric == Metric::Cpu),
            "a never-checked rule should be due"
        );

        store
            .mark_checked(&service_id, Metric::Cpu, now)
            .await
            .expect("the stamp should write");
        let due = store.due(now).await.expect("the sweep should read");
        assert!(
            !due.iter().any(|due| due.service_id == service_id),
            "a freshly checked rule should wait out its poll frequency"
        );

        assert!(
            store
                .remove(owner, &service_id, Metric::Cpu)
                .await
                .expect("removal should succeed")
        );
        assert!(
            !store
                .remove(owner, &service_id, Metric::Cpu)
                .await
                .expect("a second removal should succeed"),
            "removing an absent rule should report nothing removed"
        );

        sessions
            .end(&session_token)
            .await
            .expect("cleanup should succeed");
    }
}
