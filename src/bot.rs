use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use bytes::Bytes;
use sha2::{Digest, Sha256};
use tokio::time::MissedTickBehavior;
use tracing::{debug, error, info, warn};

use crate::{
    analytics::Analytics,
    chart::render_budget_chart,
    config::Config,
    domain::{Command, ReplyKeyboard, parse_button_payload, parse_command},
    excel::render_operations_excel,
    repository::{
        BudgetMonth, EventContext, ExecutionOutcome, ExportOperation, OutboxItem, OutboxPayload,
        Repository,
    },
    vk::{VkClient, VkEvent},
};

const MAX_RENDER_CACHE_ENTRIES: usize = 8;
const MAX_RENDER_CACHE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Default)]
struct RenderCache {
    entries: HashMap<[u8; 32], Bytes>,
    order: VecDeque<[u8; 32]>,
    total_bytes: usize,
}

impl RenderCache {
    fn get(&mut self, key: &[u8; 32]) -> Option<Bytes> {
        let rendered = self.entries.get(key).cloned()?;
        self.order.retain(|cached_key| cached_key != key);
        self.order.push_back(*key);
        Some(rendered)
    }

    fn insert(&mut self, key: [u8; 32], rendered: Bytes) {
        if rendered.len() > MAX_RENDER_CACHE_BYTES {
            return;
        }

        self.remove(&key);
        self.total_bytes += rendered.len();
        self.entries.insert(key, rendered);
        self.order.push_back(key);

        while self.entries.len() > MAX_RENDER_CACHE_ENTRIES
            || self.total_bytes > MAX_RENDER_CACHE_BYTES
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.total_bytes = self.total_bytes.saturating_sub(removed.len());
            }
        }
    }

    fn remove(&mut self, key: &[u8; 32]) {
        if let Some(removed) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(removed.len());
        }
        self.order.retain(|cached_key| cached_key != key);
    }
}

#[derive(Clone)]
pub struct Bot {
    config: Arc<Config>,
    repository: Repository,
    vk: VkClient,
    analytics: Arc<Analytics>,
    render_cache: Arc<Mutex<RenderCache>>,
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
            render_cache: Arc::new(Mutex::new(RenderCache::default())),
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

        if matches!(command, Command::SeedDemo | Command::DeleteDemo)
            && !self.config.enable_demo_data
        {
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
            ExecutionOutcome::BudgetChart {
                month_count,
                keyboard,
            } => {
                info!(
                    event_id,
                    peer_id = context.peer_id,
                    author_id = context.author_id,
                    command = command_name,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    months = month_count,
                    keyboard = keyboard.storage_key(),
                    "budget chart committed and queued for delivery"
                );
            }
            ExecutionOutcome::Excel {
                operation_count,
                keyboard,
            } => {
                info!(
                    event_id,
                    peer_id = context.peer_id,
                    author_id = context.author_id,
                    command = command_name,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    operations = operation_count,
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
            let delivery_started_at = Instant::now();
            let delay = retry_delay_seconds(item.attempt_count);
            match self.deliver_outbox_item(&item, delay).await {
                Ok(response_bytes) => {
                    self.analytics.command_processed();
                    info!(
                        event_id = item.event_id,
                        peer_id = item.peer_id,
                        attempt = item.attempt_count + 1,
                        duration_ms = delivery_started_at.elapsed().as_millis() as u64,
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
                        duration_ms = delivery_started_at.elapsed().as_millis() as u64,
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
            OutboxPayload::BudgetChart(months) => {
                let render_started_at = Instant::now();
                match self.render_budget_chart(months).await {
                    Ok((png, cache_hit)) => {
                        info!(
                            event_id,
                            peer_id,
                            render_kind = "budget_chart",
                            source_items = months.len(),
                            render_duration_ms = render_started_at.elapsed().as_millis() as u64,
                            response_bytes = png.len(),
                            cache_hit,
                            "rendered attachment ready for upload"
                        );
                        let response_bytes = png.len();
                        self.vk
                            .send_budget_chart(peer_id, event_id, png, keyboard)
                            .await
                            .map(|()| response_bytes)
                    }
                    Err(error) => Err(error).context("failed to render budget chart"),
                }
            }
            OutboxPayload::Excel(rows) => {
                let render_started_at = Instant::now();
                match self.render_operations_excel(rows).await {
                    Ok((xlsx, cache_hit)) => {
                        info!(
                            event_id,
                            peer_id,
                            render_kind = "excel",
                            source_items = rows.len(),
                            render_duration_ms = render_started_at.elapsed().as_millis() as u64,
                            response_bytes = xlsx.len(),
                            cache_hit,
                            "rendered attachment ready for upload"
                        );
                        let response_bytes = xlsx.len();
                        self.vk
                            .send_excel_export(peer_id, event_id, xlsx, keyboard)
                            .await
                            .map(|()| response_bytes)
                    }
                    Err(error) => Err(error).context("failed to render Excel export"),
                }
            }
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

    async fn render_budget_chart(&self, months: &[BudgetMonth]) -> Result<(Bytes, bool)> {
        let cache_key = budget_chart_cache_key(months);
        if let Some(rendered) = self.get_render_cache(&cache_key) {
            return Ok((rendered, true));
        }

        let months = months.to_vec();
        let rendered = tokio::task::spawn_blocking(move || render_budget_chart(&months))
            .await
            .context("budget chart render task panicked")??;
        let rendered = Bytes::from(rendered);
        self.insert_render_cache(cache_key, rendered.clone());
        Ok((rendered, false))
    }

    async fn render_operations_excel(&self, rows: &[ExportOperation]) -> Result<(Bytes, bool)> {
        let cache_key = excel_cache_key(rows);
        if let Some(rendered) = self.get_render_cache(&cache_key) {
            return Ok((rendered, true));
        }

        let rows = rows.to_vec();
        let rendered = tokio::task::spawn_blocking(move || render_operations_excel(&rows))
            .await
            .context("Excel render task panicked")??;
        let rendered = Bytes::from(rendered);
        self.insert_render_cache(cache_key, rendered.clone());
        Ok((rendered, false))
    }

    fn get_render_cache(&self, key: &[u8; 32]) -> Option<Bytes> {
        self.render_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
    }

    fn insert_render_cache(&self, key: [u8; 32], rendered: Bytes) {
        self.render_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key, rendered);
    }
}

fn budget_chart_cache_key(months: &[BudgetMonth]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"budget-chart-v1");
    for month in months {
        hash_string(&mut hasher, &month.label);
        hasher.update(month.income.to_le_bytes());
        hasher.update(month.expenses.to_le_bytes());
    }
    hasher.finalize().into()
}

fn excel_cache_key(rows: &[ExportOperation]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"excel-export-v1");
    for row in rows {
        hasher.update(row.author_id.to_le_bytes());
        hash_string(&mut hasher, &row.created_at);
        hash_string(&mut hasher, &row.operation_type);
        hash_optional_i64(&mut hasher, row.income);
        hash_optional_i64(&mut hasher, row.expense);
        hasher.update(row.balance.to_le_bytes());
    }
    hasher.finalize().into()
}

fn hash_string(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn hash_optional_i64(hasher: &mut Sha256, value: Option<i64>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.to_le_bytes());
        }
        None => hasher.update([0]),
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

    #[test]
    fn rendered_attachment_cache_is_bounded_and_removable() {
        let mut cache = RenderCache::default();
        for index in 0..=MAX_RENDER_CACHE_ENTRIES {
            cache.insert([index as u8; 32], Bytes::from_static(b"data"));
        }

        assert_eq!(cache.entries.len(), MAX_RENDER_CACHE_ENTRIES);
        assert!(cache.get(&[0; 32]).is_none());
        assert!(cache.get(&[8; 32]).is_some());

        cache.remove(&[8; 32]);
        assert!(cache.get(&[8; 32]).is_none());
        assert_eq!(cache.total_bytes, (MAX_RENDER_CACHE_ENTRIES - 1) * 4);
    }

    #[test]
    fn rendered_attachment_cache_keys_change_with_source_data() {
        let first = [BudgetMonth {
            label: "2026-10".into(),
            income: 10_000,
            expenses: 4_000,
        }];
        let mut second = first.clone();
        second[0].expenses += 1;

        assert_ne!(
            budget_chart_cache_key(&first),
            budget_chart_cache_key(&second)
        );
    }
}
