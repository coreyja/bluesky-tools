use axum::{extract::State, routing::get};
use cja::{
    app_state::AppState as AS,
    color_eyre,
    server::run_server,
    setup::{setup_sentry, setup_tracing},
};
use sqlx::{postgres::PgPoolOptions, PgPool};
use tracing::info;

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
}

impl AppState {
    pub async fn from_env() -> color_eyre::Result<Self> {
        let pool = setup_db_pool().await.unwrap();

        let cookie_key = cja::server::cookies::CookieKey::from_env_or_generate()?;

        Ok(Self {
            db: pool,
            cookie_key,
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
        .with_state(app_state)
}

async fn handler(State(state): State<AppState>) -> impl axum::response::IntoResponse {
    "Hello, World!"
}
