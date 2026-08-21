//! The horizontal autoscaling loop.
//!
//! Deliberately its own module, with its dependencies handed in as the same
//! `Arc<dyn ...>` handles [`AppState`](crate::state::AppState) holds: nothing
//! here touches the router, so detaching it into its own service is a new
//! `main` around [`Autoscaler::run`], not an untangling.
//!
//! Each sweep reads the rules whose poll frequency has elapsed, averages the
//! rule's metric over the last [`METRICS_WINDOW`](crate::constants::AUTOSCALER_METRICS_WINDOW_SECS)
//! seconds of samples, and moves the service's replica count by one — up when
//! the average exceeds the rule's maximum threshold, down when it sits under
//! the minimum — clamped into the rule's `[min_count, max_count]` band. The
//! clamp is authoritative: a count already outside the band is brought back
//! inside it the next time the metric asks for a move. A rule is stamped
//! checked whether or not its evaluation worked, so a service Railway cannot
//! answer about is retried at its poll frequency rather than every tick.
//!
//! The loop acts on Railway with the rule owner's credential, read and
//! renewed through the session store via
//! [`Credentials::access_token_for_user`] — the same machinery requests use,
//! keyed by user because the loop holds no cookie. One lookup per owner per
//! sweep; rules sharing an owner share the token.
//!
//! Two shortcuts a grown-up autoscaler would replace:
//!
//! - Every sweep reads rules and sessions straight from Postgres. A
//!   pass-through key-value store in front of those lookups (rules by due
//!   time, credentials by owner) would take the per-tick reads off the
//!   database, and becomes necessary the moment this detaches and runs wider
//!   than one instance.
//! - The aggregate is a plain mean. A single sample is noise — one GC pause
//!   or one idle minute should not resize a service — which the mean over a
//!   window already dampens, but outliers still drag it: a trimmed mean or a
//!   percentile, or requiring N consecutive breaches, is the upgrade path.

use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::{DateTime, TimeDelta, Utc};
use uuid::Uuid;

use crate::{
    constants::AUTOSCALER_METRICS_WINDOW_SECS,
    secret::Secret,
    services::{
        auth::AuthProvider,
        autoscaling::{AutoscaleResult, AutoscaleStore, Rule},
        railway::{MetricSample, RailwayApi},
        session::{Credentials, SessionStore},
    },
};

/// Which way a breached threshold moves the replica count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
}

pub struct Autoscaler {
    rules: Arc<dyn AutoscaleStore>,
    sessions: Arc<dyn SessionStore>,
    railway: Arc<dyn RailwayApi>,
    auth: Arc<dyn AuthProvider>,
    tick: Duration,
}

impl Autoscaler {
    #[must_use]
    pub fn new(
        rules: Arc<dyn AutoscaleStore>,
        sessions: Arc<dyn SessionStore>,
        railway: Arc<dyn RailwayApi>,
        auth: Arc<dyn AuthProvider>,
        tick: Duration,
    ) -> Self {
        Self {
            rules,
            sessions,
            railway,
            auth,
            tick,
        }
    }

    /// Runs until the task is dropped, sweeping once per tick. A failed sweep
    /// is logged and the next tick tries again; there is no state to unwind,
    /// because every rule records its own progress.
    pub async fn run(self) {
        let mut interval = tokio::time::interval(self.tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;

            if let Err(error) = self.sweep(Utc::now()).await {
                tracing::error!(error = ?error, "autoscaling sweep failed");
            }
        }
    }

    /// One pass over every due rule. Public, and taking `now`, so a test can
    /// drive it without a clock or a runtime timer.
    pub async fn sweep(&self, now: DateTime<Utc>) -> AutoscaleResult<()> {
        let mut tokens: HashMap<Uuid, Secret> = HashMap::new();

        for rule in self.rules.due(now).await? {
            if let Err(error) = self.evaluate(&rule, &mut tokens, now).await {
                tracing::warn!(
                    service_id = %rule.service_id,
                    metric = rule.metric.as_str(),
                    error = ?error,
                    "autoscaling rule evaluation failed",
                );
            }

            self.rules.mark_checked(&rule.service_id, now).await?;
        }

        Ok(())
    }

    async fn evaluate(
        &self,
        rule: &Rule,
        tokens: &mut HashMap<Uuid, Secret>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let access_token = match tokens.get(&rule.user_id) {
            Some(token) => token.clone(),
            None => {
                let token = Credentials::new(self.sessions.as_ref(), self.auth.as_ref())
                    .access_token_for_user(rule.user_id)
                    .await?;
                tokens.insert(rule.user_id, token.clone());
                token
            }
        };

        let window =
            TimeDelta::try_seconds(AUTOSCALER_METRICS_WINDOW_SECS).unwrap_or_else(TimeDelta::zero);
        let samples = self
            .railway
            .service_metrics(
                &access_token,
                &rule.service_id,
                rule.metric.measurement(),
                now - window,
            )
            .await?;

        let Some(average) = mean(&samples) else {
            return Ok(());
        };
        let Some(direction) = decide(average, rule) else {
            return Ok(());
        };

        let instance = self
            .railway
            .service_instance(&access_token, &rule.service_id, &rule.environment_id)
            .await?;
        let current = instance.num_replicas.unwrap_or(1);
        let target = next_replicas(current, direction, rule);

        if target == current {
            return Ok(());
        }

        self.railway
            .set_replicas(
                &access_token,
                &rule.service_id,
                &rule.environment_id,
                target,
            )
            .await?;

        tracing::info!(
            service_id = %rule.service_id,
            environment_id = %rule.environment_id,
            metric = rule.metric.as_str(),
            average,
            current,
            target,
            "scaled a service",
        );

        Ok(())
    }
}

/// `None` for an empty window: no data is no decision, not a zero.
fn mean(samples: &[MetricSample]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }

    #[allow(clippy::cast_precision_loss)]
    Some(samples.iter().map(|sample| sample.value).sum::<f64>() / samples.len() as f64)
}

/// Strictly outside the band, in either direction; sitting on a threshold is
/// inside it. NaN compares false both ways, so a poisoned average does nothing.
fn decide(average: f64, rule: &Rule) -> Option<Direction> {
    if average > rule.max_threshold {
        Some(Direction::Up)
    } else if average < rule.min_threshold {
        Some(Direction::Down)
    } else {
        None
    }
}

/// One step at a time, clamped into the rule's replica band. The clamp wins
/// over the step: a count outside `[min_count, max_count]` — the bounds were
/// tightened, or someone scaled by hand — comes back inside it. The migration
/// keeps `min_count >= 1`, so zero replicas — a stopped service — stays
/// unreachable from here.
fn next_replicas(current: i64, direction: Direction, rule: &Rule) -> i64 {
    let step = match direction {
        Direction::Up => current.saturating_add(1),
        Direction::Down => current.saturating_sub(1),
    };

    step.clamp(i64::from(rule.min_count), i64::from(rule.max_count))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::TimeDelta;

    use super::*;
    use crate::services::{
        auth::{AuthResult, CsrfState, Pkce, RailwayIdentity, TokenSet},
        autoscaling::{AutoscaleError, Metric, NewRule},
        railway::{Measurement, Project, RailwayResult, Service, ServiceInstance, ServiceSource},
        session::{Session, SessionCredentials, SessionResult, SessionToken},
    };

    fn sample(value: f64) -> MetricSample {
        MetricSample { ts: 0, value }
    }

    fn rule(min: f64, max: f64) -> Rule {
        Rule {
            service_id: "svc-1".to_owned(),
            metric: Metric::Cpu,
            user_id: uuid::Uuid::nil(),
            environment_id: "env-1".to_owned(),
            min_threshold: min,
            max_threshold: max,
            min_count: 1,
            max_count: 4,
            poll_frequency_secs: 60,
            last_checked: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn an_empty_window_is_no_decision() {
        assert_eq!(mean(&[]), None);
        assert_eq!(mean(&[sample(0.25), sample(0.75)]), Some(0.5));
    }

    #[test]
    fn the_band_is_exclusive_of_its_thresholds() {
        let rule = rule(0.2, 0.8);

        assert_eq!(decide(0.9, &rule), Some(Direction::Up));
        assert_eq!(decide(0.1, &rule), Some(Direction::Down));
        assert_eq!(decide(0.5, &rule), None);
        assert_eq!(decide(0.8, &rule), None, "sitting on a threshold is inside");
        assert_eq!(decide(0.2, &rule), None);
        assert_eq!(decide(f64::NAN, &rule), None);
    }

    #[test]
    fn a_step_stays_inside_the_replica_band() {
        let rule = rule(0.2, 0.8);

        assert_eq!(next_replicas(3, Direction::Up, &rule), 4);
        assert_eq!(next_replicas(3, Direction::Down, &rule), 2);
        assert_eq!(
            next_replicas(4, Direction::Up, &rule),
            4,
            "capped at max_count"
        );
        assert_eq!(
            next_replicas(1, Direction::Down, &rule),
            1,
            "floored at min_count"
        );
    }

    /// The clamp is authoritative: a count someone moved outside the band by
    /// hand is brought back inside it, not stepped further out.
    #[test]
    fn a_count_outside_the_band_is_pulled_back_in() {
        let rule = rule(0.2, 0.8);

        assert_eq!(next_replicas(9, Direction::Up, &rule), 4);
        assert_eq!(next_replicas(0, Direction::Down, &rule), 1);
    }

    /// An [`AutoscaleStore`] over a vec, recording what the loop did to it.
    #[derive(Default)]
    struct MemoryRules {
        due: Mutex<Vec<Rule>>,
        checked: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl AutoscaleStore for MemoryRules {
        async fn create(
            &self,
            _owner: uuid::Uuid,
            _service_id: &str,
            _rule: NewRule,
        ) -> Result<Rule, AutoscaleError> {
            unreachable!("the loop never creates rules")
        }

        async fn list(
            &self,
            _owner: uuid::Uuid,
            _service_id: &str,
        ) -> Result<Vec<Rule>, AutoscaleError> {
            unreachable!("the loop never lists rules")
        }

        async fn remove(
            &self,
            _owner: uuid::Uuid,
            _service_id: &str,
        ) -> Result<bool, AutoscaleError> {
            unreachable!("the loop never removes rules")
        }

        async fn due(&self, _now: DateTime<Utc>) -> Result<Vec<Rule>, AutoscaleError> {
            Ok(self.due.lock().expect("lock").clone())
        }

        async fn mark_checked(
            &self,
            service_id: &str,
            _now: DateTime<Utc>,
        ) -> Result<(), AutoscaleError> {
            self.checked
                .lock()
                .expect("lock")
                .push(service_id.to_owned());
            Ok(())
        }
    }

    /// A [`SessionStore`] holding one user's credentials, counting reads and
    /// recording write-backs. Only the cookie-less half is real; the loop
    /// never touches the rest.
    struct OneUserSessions {
        credentials: Option<SessionCredentials>,
        reads: Mutex<usize>,
        renewed: Mutex<Vec<(uuid::Uuid, String)>>,
    }

    impl OneUserSessions {
        fn new(credentials: Option<SessionCredentials>) -> Self {
            Self {
                credentials,
                reads: Mutex::new(0),
                renewed: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for OneUserSessions {
        async fn begin(
            &self,
            _identity: &RailwayIdentity,
            _tokens: TokenSet,
        ) -> SessionResult<(SessionToken, Session)> {
            unreachable!("the loop never opens sessions")
        }

        async fn lookup(&self, _token: &SessionToken) -> SessionResult<Option<Session>> {
            unreachable!("the loop never reads cookies")
        }

        async fn renew(&self, _token: &SessionToken, _tokens: &TokenSet) -> SessionResult<()> {
            unreachable!("the loop never renews by cookie")
        }

        async fn end(&self, _token: &SessionToken) -> SessionResult<()> {
            unreachable!("the loop never ends sessions")
        }

        async fn freshest_for_user(
            &self,
            _user_id: uuid::Uuid,
        ) -> SessionResult<Option<SessionCredentials>> {
            *self.reads.lock().expect("lock") += 1;
            Ok(self.credentials.clone())
        }

        async fn renew_by_id(
            &self,
            session_id: uuid::Uuid,
            tokens: &TokenSet,
        ) -> SessionResult<()> {
            self.renewed
                .lock()
                .expect("lock")
                .push((session_id, tokens.access_token.expose().to_owned()));
            Ok(())
        }
    }

    /// A [`RailwayApi`] that answers with fixed metrics and replicas, and
    /// records replica updates.
    struct FixedRailway {
        samples: Vec<MetricSample>,
        replicas: i64,
        scaled: Mutex<Vec<(String, String, i64)>>,
        tokens_seen: Mutex<Vec<String>>,
    }

    impl FixedRailway {
        fn new(samples: Vec<MetricSample>, replicas: i64) -> Self {
            Self {
                samples,
                replicas,
                scaled: Mutex::new(Vec::new()),
                tokens_seen: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl RailwayApi for FixedRailway {
        async fn projects(&self, _access_token: &Secret) -> RailwayResult<Vec<Project>> {
            unreachable!("the loop never lists projects")
        }

        async fn create_service(
            &self,
            _access_token: &Secret,
            _project_id: &str,
            _source: &ServiceSource,
        ) -> RailwayResult<Service> {
            unreachable!("the loop never creates services")
        }

        async fn environments(
            &self,
            _access_token: &Secret,
            _project_id: &str,
        ) -> RailwayResult<Vec<crate::services::railway::Environment>> {
            unreachable!("the loop never lists environments")
        }

        async fn service_instance(
            &self,
            _access_token: &Secret,
            service_id: &str,
            environment_id: &str,
        ) -> RailwayResult<ServiceInstance> {
            Ok(ServiceInstance {
                id: format!("{service_id}:{environment_id}"),
                start_command: None,
                build_command: None,
                root_directory: None,
                healthcheck_path: None,
                region: None,
                num_replicas: Some(self.replicas),
                restart_policy_type: None,
                restart_policy_max_retries: None,
                latest_deployment: None,
            })
        }

        async fn service_metrics(
            &self,
            access_token: &Secret,
            _service_id: &str,
            _measurement: Measurement,
            _since: DateTime<Utc>,
        ) -> RailwayResult<Vec<MetricSample>> {
            self.tokens_seen
                .lock()
                .expect("lock")
                .push(access_token.expose().to_owned());
            Ok(self.samples.clone())
        }

        async fn set_replicas(
            &self,
            _access_token: &Secret,
            service_id: &str,
            environment_id: &str,
            replicas: i64,
        ) -> RailwayResult<()> {
            self.scaled.lock().expect("lock").push((
                service_id.to_owned(),
                environment_id.to_owned(),
                replicas,
            ));
            Ok(())
        }

        async fn spin_down(
            &self,
            _access_token: &Secret,
            _service_id: &str,
            _environment_id: &str,
        ) -> RailwayResult<()> {
            unreachable!("the loop never spins services down")
        }

        async fn spin_up(
            &self,
            _access_token: &Secret,
            _service_id: &str,
            _environment_id: &str,
        ) -> RailwayResult<crate::services::railway::Deployment> {
            unreachable!("the loop never spins services up")
        }
    }

    /// Honours only `refresh-ok`, like the stub in `tests/api.rs`.
    struct RefreshOnly;

    #[async_trait::async_trait]
    impl AuthProvider for RefreshOnly {
        fn authorize_url(&self, _state: &CsrfState, _pkce: &Pkce) -> String {
            unreachable!("the loop never starts logins")
        }

        async fn exchange_code(&self, _code: &str, _pkce: &Pkce) -> AuthResult<TokenSet> {
            unreachable!("the loop never exchanges codes")
        }

        async fn refresh(&self, refresh_token: &Secret) -> AuthResult<TokenSet> {
            assert_eq!(refresh_token.expose(), "refresh-ok");

            Ok(TokenSet {
                access_token: Secret::new("access-renewed"),
                refresh_token: None,
                id_token: None,
                scope: "openid".to_owned(),
                expires_at: Utc::now() + TimeDelta::hours(1),
            })
        }

        async fn identity(&self, _access_token: &Secret) -> AuthResult<RailwayIdentity> {
            unreachable!("the loop never reads identities")
        }
    }

    fn credentials(expires_at: DateTime<Utc>) -> SessionCredentials {
        SessionCredentials {
            session_id: uuid::Uuid::nil(),
            tokens: TokenSet {
                access_token: Secret::new("access-live"),
                refresh_token: Some(Secret::new("refresh-ok")),
                id_token: None,
                scope: "openid".to_owned(),
                expires_at,
            },
        }
    }

    fn autoscaler(
        rules: Arc<MemoryRules>,
        sessions: Arc<OneUserSessions>,
        railway: Arc<FixedRailway>,
    ) -> Autoscaler {
        Autoscaler::new(
            rules,
            sessions,
            railway,
            Arc::new(RefreshOnly),
            Duration::from_secs(1),
        )
    }

    #[tokio::test]
    async fn a_breached_maximum_scales_up_by_one() {
        let now = Utc::now();
        let rules = Arc::new(MemoryRules::default());
        rules.due.lock().expect("lock").push(rule(0.2, 0.8));
        let sessions = Arc::new(OneUserSessions::new(Some(credentials(
            now + TimeDelta::hours(1),
        ))));
        let railway = Arc::new(FixedRailway::new(vec![sample(0.9), sample(0.95)], 2));

        autoscaler(rules.clone(), sessions, railway.clone())
            .sweep(now)
            .await
            .expect("the sweep should succeed");

        assert_eq!(
            railway.scaled.lock().expect("lock").clone(),
            [("svc-1".to_owned(), "env-1".to_owned(), 3)]
        );
        assert_eq!(
            rules.checked.lock().expect("lock").clone(),
            ["svc-1".to_owned()]
        );
    }

    #[tokio::test]
    async fn an_average_inside_the_band_changes_nothing() {
        let now = Utc::now();
        let rules = Arc::new(MemoryRules::default());
        rules.due.lock().expect("lock").push(rule(0.2, 0.8));
        let sessions = Arc::new(OneUserSessions::new(Some(credentials(
            now + TimeDelta::hours(1),
        ))));
        let railway = Arc::new(FixedRailway::new(vec![sample(0.5)], 2));

        autoscaler(rules.clone(), sessions, railway.clone())
            .sweep(now)
            .await
            .expect("the sweep should succeed");

        assert!(railway.scaled.lock().expect("lock").is_empty());
        assert_eq!(
            rules.checked.lock().expect("lock").len(),
            1,
            "a quiet rule still records its check"
        );
    }

    #[tokio::test]
    async fn one_replica_under_the_minimum_stays_one() {
        let now = Utc::now();
        let rules = Arc::new(MemoryRules::default());
        rules.due.lock().expect("lock").push(rule(0.2, 0.8));
        let sessions = Arc::new(OneUserSessions::new(Some(credentials(
            now + TimeDelta::hours(1),
        ))));
        let railway = Arc::new(FixedRailway::new(vec![sample(0.05)], 1));

        autoscaler(rules.clone(), sessions, railway.clone())
            .sweep(now)
            .await
            .expect("the sweep should succeed");

        assert!(railway.scaled.lock().expect("lock").is_empty());
    }

    /// A breached maximum at `max_count` replicas asks Railway for nothing:
    /// the clamp makes the target equal the current count.
    #[tokio::test]
    async fn a_service_at_max_count_is_not_scaled_further() {
        let now = Utc::now();
        let rules = Arc::new(MemoryRules::default());
        rules.due.lock().expect("lock").push(rule(0.2, 0.8));
        let sessions = Arc::new(OneUserSessions::new(Some(credentials(
            now + TimeDelta::hours(1),
        ))));
        let railway = Arc::new(FixedRailway::new(vec![sample(0.95)], 4));

        autoscaler(rules.clone(), sessions, railway.clone())
            .sweep(now)
            .await
            .expect("the sweep should succeed");

        assert!(railway.scaled.lock().expect("lock").is_empty());
        assert_eq!(rules.checked.lock().expect("lock").len(), 1);
    }

    /// A spent access token is renewed through the session store before
    /// Railway is read — the same `Credentials` path a request takes.
    #[tokio::test]
    async fn a_spent_token_is_renewed_before_the_metrics_are_read() {
        let now = Utc::now();
        let rules = Arc::new(MemoryRules::default());
        rules.due.lock().expect("lock").push(rule(0.2, 0.8));
        let sessions = Arc::new(OneUserSessions::new(Some(credentials(
            now - TimeDelta::seconds(1),
        ))));
        let railway = Arc::new(FixedRailway::new(vec![sample(0.5)], 2));

        autoscaler(rules.clone(), sessions.clone(), railway.clone())
            .sweep(now)
            .await
            .expect("the sweep should succeed");

        assert_eq!(
            railway.tokens_seen.lock().expect("lock").clone(),
            ["access-renewed"],
            "the spent token should never reach Railway"
        );
        assert_eq!(
            sessions.renewed.lock().expect("lock").clone(),
            [(uuid::Uuid::nil(), "access-renewed".to_owned())]
        );
    }

    /// Rules sharing an owner share one credential lookup per sweep.
    #[tokio::test]
    async fn one_owner_is_looked_up_once_per_sweep() {
        let now = Utc::now();
        let rules = Arc::new(MemoryRules::default());
        let mut second = rule(0.2, 0.8);
        second.service_id = "svc-2".to_owned();
        rules.due.lock().expect("lock").push(rule(0.2, 0.8));
        rules.due.lock().expect("lock").push(second);
        let sessions = Arc::new(OneUserSessions::new(Some(credentials(
            now + TimeDelta::hours(1),
        ))));
        let railway = Arc::new(FixedRailway::new(vec![sample(0.5)], 2));

        autoscaler(rules, sessions.clone(), railway)
            .sweep(now)
            .await
            .expect("the sweep should succeed");

        assert_eq!(*sessions.reads.lock().expect("lock"), 1);
    }

    /// No live session is a skipped rule, not a failed sweep — and the check
    /// is still recorded, so the retry honours the poll frequency.
    #[tokio::test]
    async fn a_rule_without_a_session_waits_for_a_login() {
        let now = Utc::now();
        let rules = Arc::new(MemoryRules::default());
        rules.due.lock().expect("lock").push(rule(0.2, 0.8));
        let sessions = Arc::new(OneUserSessions::new(None));
        let railway = Arc::new(FixedRailway::new(vec![sample(0.9)], 2));

        autoscaler(rules.clone(), sessions, railway.clone())
            .sweep(now)
            .await
            .expect("the sweep should succeed");

        assert!(railway.tokens_seen.lock().expect("lock").is_empty());
        assert!(railway.scaled.lock().expect("lock").is_empty());
        assert_eq!(rules.checked.lock().expect("lock").len(), 1);
    }
}
