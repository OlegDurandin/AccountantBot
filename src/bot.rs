use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use tokio::time::MissedTickBehavior;
use tracing::{debug, error, info, warn};

use crate::{
    analytics::Analytics,
    chart::render_budget_chart,
    config::Config,
    domain::{Command, ReplyKeyboard, parse_button_payload, parse_command},
    excel::render_operations_excel,
    repository::{EventContext, ExecutionOutcome, OutboxItem, OutboxPayload, Repository},
    vk::{VkClient, VkEvent},
};

#[derive(Clone)]
pub struct Bot {
    config: Arc<Config>,
    repository: Repository,
    vk: VkClient,
    analytics: Arc<Analytics>,
}

impl Bot {
    pub fn new(config: Arc<Config>, repository: Repository, vk: VkClient) -> Self {
        let analytics = Arc::new(Analytics::new(Duration::from_secs(
            config.analytics_interval_seconds,
        )));
        Self {
            config,
            repository,
            vk,
            analytics,
        }
    }

    pub async fn run(&self) -> Result<()> {
        self.analytics.start_reporter();
        self.start_outbox_worker();
        loop {
            let server = match self.vk.get_long_poll_server().await {
                Ok(server) => server,
                Err(error) => {
                    self.analytics.error();
                    self.analytics.reconnect();
                    error!(error = %error, "failed to obtain VK Long Poll server");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            let mut ts = server.ts.clone();
            info!(
                group_id = self.config.vk_group_id,
                "connected to VK Long Poll"
            );

            loop {
                let batch = match self.vk.poll(&server, &ts).await {
                    Ok(batch) => batch,
                    Err(error) => {
                        self.analytics.error();
                        self.analytics.reconnect();
                        warn!(error = %error, "VK Long Poll connection interrupted");
                        break;
                    }
                };

                match batch.failed {
                    Some(1) => {
                        if let Some(next_ts) = batch.ts {
                            ts = next_ts;
                        }
                        continue;
                    }
                    Some(code @ (2 | 3)) => {
                        self.analytics.reconnect();
                        warn!(code, "VK Long Poll server requested reconnection");
                        break;
                    }
                    Some(code) => {
                        self.analytics.error();
                        self.analytics.reconnect();
                        warn!(code, "unknown VK Long Poll failure code");
                        break;
                    }
                    None => {}
                }

                if let Some(next_ts) = batch.ts {
                    ts = next_ts;
                }
                debug!(updates = batch.updates.len(), "received VK Long Poll batch");
                for event in batch.updates {
                    self.analytics.event_received();
                    let event_id = event.event_id.clone();
                    let event_type = event.event_type.clone();
                    if let Err(error) = self.handle_event(event).await {
                        self.analytics.error();
                        error!(
                            event_id,
                            event_type,
                            error = %error,
                            "failed to handle VK event"
                        );
                    }
                }
            }
        }
    }

    async fn handle_event(&self, event: VkEvent) -> Result<()> {
        if event.event_type != "message_new" {
            self.analytics.event_ignored();
            debug!(event_type = event.event_type, "ignored unrelated VK event");
            return Ok(());
        }
        let message = VkClient::parse_message_event(&event)
            .context("failed to parse message_new event")?
            .message;
        let payload_command = message.payload.as_deref().and_then(parse_button_payload);
        let parsed_command = payload_command
            .map(|command| Ok(Some(command)))
            .unwrap_or_else(|| parse_command(&message.text));
        let command = match parsed_command {
            Ok(Some(command)) => command,
            Ok(None) => {
                self.analytics.event_ignored();
                debug!(
                    event_id = event.event_id,
                    peer_id = message.peer_id,
                    author_id = message.from_id,
                    "ignored message without a bot command"
                );
                return Ok(());
            }
            Err(error) => {
                self.analytics.invalid_command();
                warn!(
                    event_id = event.event_id,
                    peer_id = message.peer_id,
                    author_id = message.from_id,
                    reason = %error,
                    "rejected invalid bot command"
                );
                self.vk
                    .send_message(
                        message.peer_id,
                        &event.event_id,
                        &error.to_string(),
                        ReplyKeyboard::None,
                    )
                    .await
                    .context("failed to send command validation error")?;
                return Ok(());
            }
        };
        self.analytics.command_recognized();
        let command_name = command.name();
        let started_at = Instant::now();

        if matches!(command, Command::SeedDemo) && !self.config.enable_demo_data {
            self.vk
                .send_message(
                    message.peer_id,
                    &event.event_id,
                    "Тестовые данные отключены. Установите ENABLE_DEMO_DATA=true только в тестовом окружении.",
                    ReplyKeyboard::None,
                )
                .await
                .context("failed to report disabled demo data")?;
            warn!(
                event_id = event.event_id,
                peer_id = message.peer_id,
                author_id = message.from_id,
                "rejected demo data command because it is disabled"
            );
            return Ok(());
        }

        if let Command::Expense { category, .. } = &command
            && !self.config.category_is_allowed(category)
        {
            self.analytics.invalid_command();
            let allowed = self
                .config
                .allowed_expense_categories
                .as_ref()
                .map(|categories| {
                    let mut categories = categories.iter().cloned().collect::<Vec<_>>();
                    categories.sort();
                    categories.join(", ")
                })
                .unwrap_or_default();
            self.vk
                .send_message(
                    message.peer_id,
                    &event.event_id,
                    &format!("Неизвестная категория. Разрешены: {allowed}"),
                    ReplyKeyboard::None,
                )
                .await
                .context("failed to send category validation error")?;
            warn!(
                event_id = event.event_id,
                peer_id = message.peer_id,
                author_id = message.from_id,
                command = command_name,
                "rejected expense with an unknown category"
            );
            return Ok(());
        }

        let fallback_event_id = format!(
            "message:{}:{}",
            message.peer_id, message.conversation_message_id
        );
        let event_id = if event.event_id.is_empty() {
            fallback_event_id
        } else {
            event.event_id
        };
        let context = EventContext {
            event_id: event_id.clone(),
            peer_id: message.peer_id,
            author_id: message.from_id,
            message_id: message.conversation_message_id,
            reply_to_message_id: message
                .reply_message
                .map(|reply| reply.conversation_message_id),
        };

        match self.repository.execute(&context, &command).await? {
            ExecutionOutcome::Duplicate => {
                self.analytics.duplicate_event();
                info!(
                    event_id,
                    peer_id = context.peer_id,
                    author_id = context.author_id,
                    command = command_name,
                    "ignored duplicate VK event"
                );
            }
            ExecutionOutcome::Reply { message, keyboard } => {
                info!(
                    event_id,
                    peer_id = context.peer_id,
                    author_id = context.author_id,
                    command = command_name,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    response_chars = message.chars().count(),
                    keyboard = keyboard.storage_key(),
                    "command committed and queued for delivery"
                );
            }
            ExecutionOutcome::BudgetChart { months, keyboard } => {
                info!(
                    event_id,
                    peer_id = context.peer_id,
                    author_id = context.author_id,
                    command = command_name,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    months = months.len(),
                    keyboard = keyboard.storage_key(),
                    "budget chart committed and queued for delivery"
                );
            }
            ExecutionOutcome::Excel { rows, keyboard } => {
                info!(
                    event_id,
                    peer_id = context.peer_id,
                    author_id = context.author_id,
                    command = command_name,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    operations = rows.len(),
                    keyboard = keyboard.storage_key(),
                    "Excel export committed and queued for delivery"
                );
            }
        }
        Ok(())
    }

    fn start_outbox_worker(&self) {
        let bot = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                if let Err(error) = bot.retry_pending_outbox().await {
                    bot.analytics.error();
                    error!(error = %error, "outbox retry cycle failed");
                }
            }
        });
    }

    async fn retry_pending_outbox(&self) -> Result<()> {
        let pending = self.repository.pending_outbox(20).await?;
        for item in pending {
            let delay = retry_delay_seconds(item.attempt_count);
            match self.deliver_outbox_item(&item, delay).await {
                Ok(response_bytes) => {
                    self.analytics.command_processed();
                    info!(
                        event_id = item.event_id,
                        peer_id = item.peer_id,
                        attempt = item.attempt_count + 1,
                        response_bytes,
                        "outbox message delivered"
                    );
                }
                Err(error) => {
                    self.analytics.error();
                    warn!(
                        event_id = item.event_id,
                        peer_id = item.peer_id,
                        attempt = item.attempt_count + 1,
                        retry_delay_seconds = delay,
                        error = %error,
                        "outbox delivery failed; retry scheduled"
                    );
                }
            }
        }
        Ok(())
    }

    async fn deliver_outbox_item(&self, item: &OutboxItem, retry_delay: u64) -> Result<usize> {
        self.deliver_and_mark(
            &item.event_id,
            item.peer_id,
            &item.payload,
            item.keyboard,
            retry_delay,
        )
        .await
    }

    async fn deliver_and_mark(
        &self,
        event_id: &str,
        peer_id: i64,
        payload: &OutboxPayload,
        keyboard: ReplyKeyboard,
        retry_delay: u64,
    ) -> Result<usize> {
        let delivery = match payload {
            OutboxPayload::Text(message) => self
                .vk
                .send_message(peer_id, event_id, message, keyboard)
                .await
                .map(|()| message.len()),
            OutboxPayload::BudgetChart(months) => match render_budget_chart(months) {
                Ok(png) => {
                    let response_bytes = png.len();
                    self.vk
                        .send_budget_chart(peer_id, event_id, png, keyboard)
                        .await
                        .map(|()| response_bytes)
                }
                Err(error) => Err(error).context("failed to render budget chart"),
            },
            OutboxPayload::Excel(rows) => match render_operations_excel(rows) {
                Ok(xlsx) => {
                    let response_bytes = xlsx.len();
                    self.vk
                        .send_excel_export(peer_id, event_id, xlsx, keyboard)
                        .await
                        .map(|()| response_bytes)
                }
                Err(error) => Err(error).context("failed to render Excel export"),
            },
        };

        match delivery {
            Ok(response_bytes) => {
                self.repository.mark_outbox_sent(event_id).await?;
                Ok(response_bytes)
            }
            Err(error) => {
                let error_message = error.to_string().chars().take(1_000).collect::<String>();
                self.repository
                    .mark_outbox_failed(event_id, &error_message, retry_delay)
                    .await
                    .context("failed to schedule outbox retry")?;
                Err(error)
            }
        }
    }
}

fn retry_delay_seconds(attempt_count: u32) -> u64 {
    5_u64
        .saturating_mul(2_u64.saturating_pow(attempt_count.min(6)))
        .min(300)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbox_retry_delay_is_bounded() {
        assert_eq!(retry_delay_seconds(0), 5);
        assert_eq!(retry_delay_seconds(3), 40);
        assert_eq!(retry_delay_seconds(20), 300);
    }
}
