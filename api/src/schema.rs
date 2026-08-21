//! The database schema, as diesel sees it.
//!
//! Hand-written rather than generated: `diesel print-schema` needs diesel-cli,
//! which links libpq, and a reachable database — both of which this repo
//! deliberately avoids. Keep it in step with `migrations/` by hand; a mismatch
//! is a compile error at the query, not a runtime surprise.

diesel::table! {
    users (id) {
        id -> Uuid,
        railway_user_id -> Text,
        email -> Nullable<Text>,
        name -> Nullable<Text>,
        avatar_url -> Nullable<Text>,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    sessions (id) {
        id -> Uuid,
        token_hash -> Bytea,
        user_id -> Uuid,
        access_token -> Text,
        refresh_token -> Nullable<Text>,
        scope -> Text,
        access_token_expires_at -> Timestamptz,
        expires_at -> Timestamptz,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    horizontal_autoscaling (service_id) {
        service_id -> Text,
        metric -> Text,
        user_id -> Uuid,
        environment_id -> Text,
        min_threshold -> Float8,
        max_threshold -> Float8,
        min_count -> Int4,
        max_count -> Int4,
        poll_frequency_secs -> Int4,
        last_checked -> Nullable<Timestamptz>,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::joinable!(sessions -> users (user_id));
diesel::joinable!(horizontal_autoscaling -> users (user_id));
diesel::allow_tables_to_appear_in_same_query!(horizontal_autoscaling, sessions, users);
