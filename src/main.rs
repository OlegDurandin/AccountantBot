use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use accountant_bot::{
    bot::Bot, config::Config, domain::ReplyKeyboard, repository::Repository, vk::VkClient,
};
use anyhow::{Result, bail};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const RELEASE_NOTES: &str = include_str!("../deploy/release-notes.txt");

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_logging();

    let config = Arc::new(Config::from_env()?);
    ensure_database_directory(&config.database_url)?;

    let repository = Repository::connect(&config.database_url).await?;
    let application_command = std::env::args().nth(1);
    if application_command.is_none() {
        repository.migrate().await?;
    }

    let vk = VkClient::new(
        config.vk_token.clone(),
        config.vk_group_id,
        config.vk_api_version.clone(),
    )?;

    if let Some(command) = application_command {
        let (phase, message) = match command.as_str() {
            "notify-updating" => (
                "updating",
                "🐰 Зая обновляется. Вернусь на связь через пару минут!".to_string(),
            ),
            "notify-updated" => (
                "updated",
                format!(
                    "🐰 Я обновилась! Теперь я умею:\n{}\n\nЯ снова на связи — можно продолжать считать денюжки 💛",
                    RELEASE_NOTES.trim()
                ),
            ),
            _ => bail!("unknown application command: {command}"),
        };
        notify_known_conversations(&repository, &vk, phase, &message).await?;
        return Ok(());
    }

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

async fn notify_known_conversations(
    repository: &Repository,
    vk: &VkClient,
    phase: &str,
    message: &str,
) -> Result<()> {
    let peer_ids = repository.known_peer_ids().await?;
    if peer_ids.is_empty() {
        info!(phase, "no known conversations to notify about deployment");
        return Ok(());
    }

    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let mut failed = 0_usize;
    for peer_id in &peer_ids {
        let event_id = format!("deployment:{phase}:{timestamp}:{peer_id}");
        if let Err(error) = vk
            .send_message(*peer_id, &event_id, message, ReplyKeyboard::None)
            .await
        {
            failed += 1;
            warn!(peer_id, phase, error = %error, "failed to send deployment notification");
        }
    }

    info!(
        phase,
        conversations = peer_ids.len(),
        failed,
        "deployment notifications processed"
    );
    if failed == peer_ids.len() {
        bail!("failed to notify every known conversation");
    }
    Ok(())
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
