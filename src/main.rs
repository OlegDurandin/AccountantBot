use std::sync::Arc;

use accountant_bot::{bot::Bot, config::Config, repository::Repository, vk::VkClient};
use anyhow::Result;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_logging();

    let config = Arc::new(Config::from_env()?);
    ensure_database_directory(&config.database_url)?;

    let repository = Repository::connect(&config.database_url).await?;
    repository.migrate().await?;

    let vk = VkClient::new(
        config.vk_token.clone(),
        config.vk_group_id,
        config.vk_api_version.clone(),
    )?;

    info!(
        group_id = config.vk_group_id,
        api_version = %config.vk_api_version,
        analytics_interval_seconds = config.analytics_interval_seconds,
        fixed_categories = config.allowed_expense_categories.is_some(),
        demo_data_enabled = config.enable_demo_data,
        "accountant bot started"
    );
    Bot::new(config, repository, vk).run().await
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,reqwest=warn"));
    if std::env::var("LOG_FORMAT").is_ok_and(|value| value.eq_ignore_ascii_case("json")) {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_file(true)
            .with_line_number(true)
            .init();
    } else {
        tracing_subscriber::fmt()
            .compact()
            .with_env_filter(filter)
            .with_file(true)
            .with_line_number(true)
            .init();
    }
}

fn ensure_database_directory(database_url: &str) -> Result<()> {
    let Some(path) = database_url.strip_prefix("sqlite://") else {
        return Ok(());
    };
    let path = path.split('?').next().unwrap_or(path);
    if path == ":memory:" {
        return Ok(());
    }
    if let Some(parent) = std::path::Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}
