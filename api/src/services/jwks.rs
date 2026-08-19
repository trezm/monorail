//! The provider's ID token signing keys.
//!
//! Fetched from `{issuer}/oauth/jwks` on first use and cached by `kid`. An
//! unknown `kid` refetches once, which is what makes a key rotation survive
//! without a restart; a `kid` still unknown after that is a failure rather than
//! a retry loop.

use std::{collections::HashMap, sync::RwLock};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use url::Url;

use crate::services::auth::{AuthError, AuthResult};

/// The one algorithm Railway advertises in
/// `id_token_signing_alg_values_supported`.
///
/// Pinned rather than read from the token's own header: taking the algorithm
/// from the thing being verified is how `alg` confusion works.
const SIGNING_ALGORITHM: Algorithm = Algorithm::ES256;

/// The claims this service reads. `sub` is the only one Railway guarantees;
/// `iss`, `aud` and `exp` are checked by [`Validation`] rather than here.
#[derive(Debug, Deserialize)]
pub struct IdTokenClaims {
    pub sub: String,
}

pub struct Jwks {
    uri: Url,
    http: reqwest::Client,
    keys: RwLock<HashMap<String, DecodingKey>>,
}

impl Jwks {
    pub fn new(uri: Url, http: reqwest::Client) -> Self {
        Self {
            uri,
            http,
            keys: RwLock::new(HashMap::new()),
        }
    }

    /// Verifies an ID token's signature, issuer, audience and expiry.
    pub async fn verify(
        &self,
        token: &str,
        issuer: &str,
        audience: &str,
    ) -> AuthResult<IdTokenClaims> {
        let header = decode_header(token).map_err(|error| {
            AuthError::InvalidIdToken(anyhow::Error::new(error).context("unreadable header"))
        })?;

        let kid = header.kid.ok_or_else(|| {
            AuthError::InvalidIdToken(anyhow::anyhow!("the header carries no `kid`"))
        })?;

        let key = self.key(&kid).await?;

        let mut validation = Validation::new(SIGNING_ALGORITHM);
        validation.set_issuer(&[issuer]);
        validation.set_audience(&[audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

        decode::<IdTokenClaims>(token, &key, &validation)
            .map(|data| data.claims)
            .map_err(|error| AuthError::InvalidIdToken(anyhow::Error::new(error)))
    }

    /// The key for `kid`, fetching the set if it is not already cached.
    async fn key(&self, kid: &str) -> AuthResult<DecodingKey> {
        if let Some(key) = self.cached(kid) {
            return Ok(key);
        }

        self.fetch().await?;

        self.cached(kid).ok_or_else(|| {
            AuthError::InvalidIdToken(anyhow::anyhow!(
                "the provider published no key for `kid` {kid}"
            ))
        })
    }

    fn cached(&self, kid: &str) -> Option<DecodingKey> {
        self.keys
            .read()
            .expect("the jwks cache lock was poisoned")
            .get(kid)
            .cloned()
    }

    async fn fetch(&self) -> AuthResult<()> {
        let response = self
            .http
            .get(self.uri.clone())
            .send()
            .await
            .map_err(|error| {
                AuthError::Provider(anyhow::Error::new(error).context("jwks request failed"))
            })?;

        let status = response.status();

        if !status.is_success() {
            return Err(AuthError::Provider(anyhow::anyhow!(
                "jwks endpoint answered {status}"
            )));
        }

        let document: JwkSet = response.json().await.map_err(|error| {
            AuthError::Provider(
                anyhow::Error::new(error).context("jwks response was not the expected shape"),
            )
        })?;

        let mut cache = self.keys.write().expect("the jwks cache lock was poisoned");
        *cache = document
            .keys
            .into_iter()
            .filter_map(|jwk| jwk.into_key())
            .collect();

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

/// One EC key. Railway signs with P-256 only, so an entry that is anything else
/// is dropped rather than guessed at.
#[derive(Debug, Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    crv: Option<String>,
    x: String,
    y: Option<String>,
}

impl Jwk {
    fn into_key(self) -> Option<(String, DecodingKey)> {
        if self.kty != "EC" || self.crv.as_deref() != Some("P-256") {
            return None;
        }

        let key = DecodingKey::from_ec_components(&self.x, self.y.as_deref()?).ok()?;

        Some((self.kid, key))
    }
}
