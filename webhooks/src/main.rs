use std::sync::Arc;

use atrium_api::{
    agent::{store::MemorySessionStore, AtpAgent},
    types::string::Did,
};
use atrium_xrpc_client::reqwest::ReqwestClient;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse as _, Response},
    routing::{get, post},
    Form,
};
use cja::{
    app_state::AppState as AS,
    color_eyre,
    server::run_server,
    setup::{setup_sentry, setup_tracing},
};
use maud::html;
use serde::Deserialize;
use sms::TwilioConfig;
use sqlx::{postgres::PgPoolOptions, PgPool};
use tracing::info;

mod sms;

fn main() -> color_eyre::Result<()> {
    let _sentry_guard = setup_sentry();

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()?
        .block_on(async { _main().await })
}

async fn _main() -> cja::Result<()> {
    setup_tracing("bsky-webhooks")?;

    let app_state = AppState::from_env().await?;

    cja::sqlx::migrate!().run(app_state.db()).await?;

    info!("Spawning Tasks");
    let mut futures = vec![
        tokio::spawn(run_server(routes(app_state.clone()))),
        // tokio::spawn(cja::jobs::worker::job_worker(app_state.clone(), jobs::Jobs)),
    ];
    // if std::env::var("CRON_DISABLED").unwrap_or_else(|_| "false".to_string()) != "true" {
    //     info!("Cron Enabled");
    //     futures.push(tokio::spawn(cron::run_cron(app_state.clone())));
    // }
    info!("Tasks Spawned");

    futures::future::try_join_all(futures).await?;

    Ok(())
}

#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::PgPool,
    pub cookie_key: cja::server::cookies::CookieKey,
    pub atproto_agent: Arc<AtpAgent<MemorySessionStore, ReqwestClient>>,
    pub twilio_config: TwilioConfig,
}

impl AppState {
    pub async fn from_env() -> color_eyre::Result<Self> {
        let pool = setup_db_pool().await.unwrap();

        let cookie_key = cja::server::cookies::CookieKey::from_env_or_generate()?;

        let client = ReqwestClient::new("https://bsky.social");
        let agent = AtpAgent::new(client, MemorySessionStore::default());

        Ok(Self {
            db: pool,
            cookie_key,
            atproto_agent: Arc::new(agent),
            twilio_config: TwilioConfig::from_env()?,
        })
    }
}

impl AS for AppState {
    fn version(&self) -> &str {
        "0.0.1"
    }

    fn db(&self) -> &sqlx::PgPool {
        &self.db
    }

    fn cookie_key(&self) -> &cja::server::cookies::CookieKey {
        &self.cookie_key
    }
}

#[tracing::instrument(err)]
pub async fn setup_db_pool() -> cja::Result<PgPool> {
    const MIGRATION_LOCK_ID: i64 = 0xDB_DB_DB_DB_DB_DB_DB;

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    sqlx::query!("SELECT pg_advisory_lock($1)", MIGRATION_LOCK_ID)
        .execute(&pool)
        .await?;

    sqlx::migrate!().run(&pool).await?;

    let unlock_result = sqlx::query!("SELECT pg_advisory_unlock($1)", MIGRATION_LOCK_ID)
        .fetch_one(&pool)
        .await?
        .pg_advisory_unlock;

    match unlock_result {
        Some(b) => {
            if b {
                tracing::info!("Migration lock unlocked");
            } else {
                tracing::info!("Failed to unlock migration lock");
            }
        }
        None => panic!("Failed to unlock migration lock"),
    }

    Ok(pool)
}

fn routes(app_state: AppState) -> axum::Router {
    axum::Router::new()
        .route("/", get(handler))
        .route("/sms_subscription", post(sms_subscription))
        .with_state(app_state)
}

async fn handler() -> impl axum::response::IntoResponse {
    html! {
        form action="/sms_subscription" method="post" {
            input type="text" name="phone_number" placeholder="Phone Number" {}
            input type="text" name="handle" placeholder="Handle" {}
            input type="submit" value="Subscribe" {}
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
struct SmsSubscriptionForm {
    phone_number: String,
    handle: String,
}

async fn sms_subscription(
    State(state): State<AppState>,
    Form(form): Form<SmsSubscriptionForm>,
) -> Result<Response, Response> {
    let did = resolve_handle(&state, &form.handle)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    let verified_phone_number =
        sms::find_verified_phone_numbers(&state.twilio_config, &form.phone_number)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    sqlx::query!(
        "INSERT INTO SmsHandleSubscriptions (phone_number, handle, did) VALUES ($1, $2, $3)",
        verified_phone_number.phone_number,
        &form.handle,
        did.as_str(),
    )
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    Ok("Success".into_response())
}

async fn resolve_handle(state: &AppState, handle: &str) -> cja::Result<Did> {
    let resp = state
        .atproto_agent
        .api
        .com
        .atproto
        .identity
        .resolve_handle(
            atrium_api::com::atproto::identity::resolve_handle::ParametersData {
                handle: handle
                    .parse()
                    .map_err(|_| cja::color_eyre::eyre::eyre!("Invalid handle"))?,
            }
            .into(),
        )
        .await?;
    let did = resp.data.did;
    Ok(did)
}
