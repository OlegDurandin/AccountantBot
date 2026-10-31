use std::{str::FromStr, time::Duration};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool, sqlite::SqliteConnectOptions, sqlite::SqlitePoolOptions};
use tracing::info;

use crate::domain::{Command, ReplyKeyboard};

#[derive(Debug, Clone)]
pub struct EventContext {
    pub event_id: String,
    pub peer_id: i64,
    pub author_id: i64,
    pub message_id: i64,
    pub reply_to_message_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionOutcome {
    Duplicate,
    Reply {
        message: String,
        keyboard: ReplyKeyboard,
    },
    BudgetChart {
        months: Vec<BudgetMonth>,
        keyboard: ReplyKeyboard,
    },
    Excel {
        rows: Vec<ExportOperation>,
        keyboard: ReplyKeyboard,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetMonth {
    pub label: String,
    pub income: i64,
    pub expenses: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportOperation {
    pub author_id: i64,
    pub created_at: String,
    pub operation_type: String,
    pub income: Option<i64>,
    pub expense: Option<i64>,
    pub balance: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboxPayload {
    Text(String),
    BudgetChart(Vec<BudgetMonth>),
    Excel(Vec<ExportOperation>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxItem {
    pub event_id: String,
    pub peer_id: i64,
    pub payload: OutboxPayload,
    pub keyboard: ReplyKeyboard,
    pub attempt_count: u32,
}

#[derive(Clone)]
pub struct Repository {
    pool: SqlitePool,
}

impl Repository {
    pub async fn connect(database_url: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(database_url)
            .with_context(|| format!("invalid DATABASE_URL: {database_url}"))?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let max_connections = if database_url.contains(":memory:") {
            1
        } else {
            5
        };
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(options)
            .await
            .context("failed to connect to SQLite")?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::raw_sql(include_str!("../migrations/0001_init.sql"))
            .execute(&self.pool)
            .await
            .context("failed to apply database migration")?;
        sqlx::raw_sql(include_str!("../migrations/0002_outbox.sql"))
            .execute(&self.pool)
            .await
            .context("failed to apply outbox migration")?;
        sqlx::raw_sql(include_str!("../migrations/0003_history_date_index.sql"))
            .execute(&self.pool)
            .await
            .context("failed to apply history index migration")?;
        let outbox_schema: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'outbox'",
        )
        .fetch_one(&self.pool)
        .await
        .context("failed to inspect outbox schema")?;
        if !outbox_schema.contains("'excel'") {
            sqlx::raw_sql(include_str!("../migrations/0004_outbox_excel.sql"))
                .execute(&self.pool)
                .await
                .context("failed to apply Excel outbox migration")?;
        }
        info!("database migration completed");
        Ok(())
    }

    pub async fn execute(
        &self,
        event: &EventContext,
        command: &Command,
    ) -> Result<ExecutionOutcome> {
        let mut tx = self.pool.begin().await?;
        let inserted =
            sqlx::query("INSERT OR IGNORE INTO processed_events (event_id, peer_id) VALUES (?, ?)")
                .bind(&event.event_id)
                .bind(event.peer_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();

        if inserted == 0 {
            tx.rollback().await?;
            return Ok(ExecutionOutcome::Duplicate);
        }

        if matches!(command, Command::BudgetStatistics) {
            let months = budget_months_in_tx(&mut tx, event.peer_id).await?;
            let keyboard = ReplyKeyboard::Reports;
            enqueue_outbox(
                &mut tx,
                &event.event_id,
                event.peer_id,
                &OutboxPayload::BudgetChart(months.clone()),
                keyboard,
            )
            .await?;
            tx.commit().await?;
            return Ok(ExecutionOutcome::BudgetChart { months, keyboard });
        }

        if let Command::ExportExcel(_) = command {
            let rows = export_operations_in_tx(&mut tx, event.peer_id).await?;
            let keyboard = ReplyKeyboard::CurrentBalance;
            enqueue_outbox(
                &mut tx,
                &event.event_id,
                event.peer_id,
                &OutboxPayload::Excel(rows.clone()),
                keyboard,
            )
            .await?;
            tx.commit().await?;
            return Ok(ExecutionOutcome::Excel { rows, keyboard });
        }

        let (message, keyboard) = match command {
            Command::OpeningBalance(amount) => {
                let exists: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM operations WHERE peer_id = ? AND kind = 'opening' AND cancelled_by_event_id IS NULL",
                )
                .bind(event.peer_id)
                .fetch_one(&mut *tx)
                .await?;
                if exists > 0 {
                    (
                        "Начальный остаток уже задан. Если он неверный, отмените ту операцию ответом на её сообщение.".into(),
                        ReplyKeyboard::OperationResult,
                    )
                } else {
                    insert_operation(&mut tx, event, "opening", *amount, None, None).await?;
                    (
                        format!("Остаток: {} ₽.", format_money(*amount)),
                        ReplyKeyboard::OperationResult,
                    )
                }
            }
            Command::Income(amount) => {
                insert_operation(&mut tx, event, "income", *amount, None, None).await?;
                let balance = balance_in_tx(&mut tx, event.peer_id).await?;
                (
                    format!("Остаток: {} ₽.", format_money(balance)),
                    ReplyKeyboard::OperationResult,
                )
            }
            Command::Expense { category, amount } => {
                insert_operation(&mut tx, event, "expense", *amount, Some(category), None).await?;
                let balance = balance_in_tx(&mut tx, event.peer_id).await?;
                (
                    format!("Остаток: {} ₽.", format_money(balance)),
                    ReplyKeyboard::OperationResult,
                )
            }
            Command::Balance => {
                let balance = balance_in_tx(&mut tx, event.peer_id).await?;
                (
                    format!("Остаток: {} ₽.", format_money(balance)),
                    ReplyKeyboard::Reports,
                )
            }
            Command::Expenses(days) => (
                expenses_in_tx(&mut tx, event.peer_id, *days).await?,
                ReplyKeyboard::ExpenseGroups(*days),
            ),
            Command::History(days) => (
                history_in_tx(&mut tx, event.peer_id, *days).await?,
                ReplyKeyboard::ExpenseHistory(*days),
            ),
            Command::MonthlyExpenses { month, year } => (
                monthly_expenses_in_tx(&mut tx, event.peer_id, *month, *year).await?,
                ReplyKeyboard::Reports,
            ),
            Command::BudgetStatistics => unreachable!("handled before the command match"),
            Command::ExportExcel(_) => unreachable!("handled before the command match"),
            Command::SeedDemo => (
                seed_demo_in_tx(&mut tx, event).await?,
                ReplyKeyboard::Reports,
            ),
            Command::Cancel => (
                cancel_in_tx(&mut tx, event).await?,
                ReplyKeyboard::OperationResult,
            ),
            Command::Help => (help_text(), ReplyKeyboard::OperationResult),
        };

        enqueue_outbox(
            &mut tx,
            &event.event_id,
            event.peer_id,
            &OutboxPayload::Text(message.clone()),
            keyboard,
        )
        .await?;
        tx.commit().await?;
        Ok(ExecutionOutcome::Reply { message, keyboard })
    }

    pub async fn pending_outbox(&self, limit: u32) -> Result<Vec<OutboxItem>> {
        let rows = sqlx::query(
            "SELECT event_id, peer_id, response_type, payload, keyboard, attempt_count
             FROM outbox
             WHERE sent_at IS NULL AND available_at <= CURRENT_TIMESTAMP
             ORDER BY id
             LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let response_type: String = row.get("response_type");
                let raw_payload: String = row.get("payload");
                let keyboard_key: String = row.get("keyboard");
                let keyboard = ReplyKeyboard::from_storage_key(&keyboard_key)
                    .ok_or_else(|| anyhow!("unknown outbox keyboard: {keyboard_key}"))?;
                let payload = match response_type.as_str() {
                    "text" => OutboxPayload::Text(raw_payload),
                    "budget_chart" => OutboxPayload::BudgetChart(
                        serde_json::from_str(&raw_payload)
                            .context("invalid budget chart payload in outbox")?,
                    ),
                    "excel" => OutboxPayload::Excel(
                        serde_json::from_str(&raw_payload)
                            .context("invalid Excel payload in outbox")?,
                    ),
                    _ => return Err(anyhow!("unknown outbox response type: {response_type}")),
                };
                Ok(OutboxItem {
                    event_id: row.get("event_id"),
                    peer_id: row.get("peer_id"),
                    payload,
                    keyboard,
                    attempt_count: row.get::<i64, _>("attempt_count") as u32,
                })
            })
            .collect()
    }

    pub async fn mark_outbox_sent(&self, event_id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE outbox
             SET sent_at = CURRENT_TIMESTAMP, last_error = NULL
             WHERE event_id = ? AND sent_at IS NULL",
        )
        .bind(event_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_outbox_failed(
        &self,
        event_id: &str,
        error: &str,
        retry_delay_seconds: u64,
    ) -> Result<()> {
        let modifier = format!("+{} seconds", retry_delay_seconds.max(1));
        sqlx::query(
            "UPDATE outbox
             SET attempt_count = attempt_count + 1,
                 available_at = datetime('now', ?),
                 last_error = ?
             WHERE event_id = ? AND sent_at IS NULL",
        )
        .bind(modifier)
        .bind(error)
        .bind(event_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

async fn enqueue_outbox(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    event_id: &str,
    peer_id: i64,
    payload: &OutboxPayload,
    keyboard: ReplyKeyboard,
) -> Result<()> {
    let (response_type, serialized_payload) = match payload {
        OutboxPayload::Text(message) => ("text", message.clone()),
        OutboxPayload::BudgetChart(months) => (
            "budget_chart",
            serde_json::to_string(months).context("failed to serialize budget chart outbox")?,
        ),
        OutboxPayload::Excel(rows) => (
            "excel",
            serde_json::to_string(rows).context("failed to serialize Excel outbox")?,
        ),
    };
    sqlx::query(
        "INSERT INTO outbox
         (event_id, peer_id, response_type, payload, keyboard)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(event_id)
    .bind(peer_id)
    .bind(response_type)
    .bind(serialized_payload)
    .bind(keyboard.storage_key())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    event: &EventContext,
    kind: &str,
    amount: i64,
    category: Option<&str>,
    target_operation_id: Option<i64>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO operations
         (event_id, peer_id, author_id, message_id, kind, amount, category, target_operation_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&event.event_id)
    .bind(event.peer_id)
    .bind(event.author_id)
    .bind(event.message_id)
    .bind(kind)
    .bind(amount)
    .bind(category)
    .bind(target_operation_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn balance_in_tx(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, peer_id: i64) -> Result<i64> {
    let balance = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(CASE
            WHEN kind IN ('opening', 'income') THEN amount
            WHEN kind = 'expense' THEN -amount
            ELSE 0 END), 0)
         FROM operations
         WHERE peer_id = ? AND cancelled_by_event_id IS NULL",
    )
    .bind(peer_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(balance)
}

async fn export_operations_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    peer_id: i64,
) -> Result<Vec<ExportOperation>> {
    let rows = sqlx::query(
        "SELECT author_id,
                kind,
                amount,
                category,
                datetime(created_at, '+3 hours') AS created_at
         FROM operations
         WHERE peer_id = ?
           AND kind IN ('opening', 'income', 'expense')
           AND cancelled_by_event_id IS NULL
         ORDER BY created_at ASC, id ASC",
    )
    .bind(peer_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut balance = 0_i64;
    let mut operations = Vec::with_capacity(rows.len());
    for row in rows {
        let kind: String = row.get("kind");
        let amount: i64 = row.get("amount");
        let category: Option<String> = row.get("category");
        let (operation_type, income, expense) = match kind.as_str() {
            "opening" => {
                balance += amount;
                ("Начальный остаток".to_string(), Some(amount), None)
            }
            "income" => {
                balance += amount;
                ("Приход".to_string(), Some(amount), None)
            }
            "expense" => {
                balance -= amount;
                let category = category
                    .as_deref()
                    .map(capitalize_category)
                    .unwrap_or_else(|| "Без категории".into());
                (format!("Расход — {category}"), None, Some(amount))
            }
            _ => continue,
        };
        operations.push(ExportOperation {
            author_id: row.get("author_id"),
            created_at: row.get("created_at"),
            operation_type,
            income,
            expense,
            balance,
        });
    }
    Ok(operations)
}

async fn budget_months_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    peer_id: i64,
) -> Result<Vec<BudgetMonth>> {
    let rows = sqlx::query(
        "WITH RECURSIVE months(month_start) AS (
             SELECT date('now', 'start of month', '-3 months')
             UNION ALL
             SELECT date(month_start, '+1 month')
             FROM months
             WHERE month_start < date('now', 'start of month')
         )
         SELECT strftime('%Y-%m', months.month_start) AS month,
                COALESCE(SUM(CASE WHEN operations.kind = 'income' THEN operations.amount ELSE 0 END), 0) AS income,
                COALESCE(SUM(CASE WHEN operations.kind = 'expense' THEN operations.amount ELSE 0 END), 0) AS expenses
         FROM months
         LEFT JOIN operations
           ON operations.peer_id = ?
          AND operations.cancelled_by_event_id IS NULL
          AND operations.kind IN ('income', 'expense')
          AND operations.created_at >= months.month_start
          AND operations.created_at < date(months.month_start, '+1 month')
         GROUP BY months.month_start
         ORDER BY months.month_start",
    )
    .bind(peer_id)
    .fetch_all(&mut **tx)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| BudgetMonth {
            label: row.get("month"),
            income: row.get("income"),
            expenses: row.get("expenses"),
        })
        .collect())
}

async fn seed_demo_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    event: &EventContext,
) -> Result<String> {
    let demo_rows = [
        ("income-current", "income", 120_000, None, "-5 days"),
        (
            "materials-current",
            "expense",
            30_000,
            Some("материалы"),
            "-4 days",
        ),
        (
            "salary-current",
            "expense",
            20_000,
            Some("зарплата аниматорам"),
            "-12 days",
        ),
        ("income-30-days", "income", 80_000, None, "-20 days"),
        (
            "rent-30-days",
            "expense",
            25_000,
            Some("аренда"),
            "-22 days",
        ),
        ("income-month-1", "income", 100_000, None, "-1 month"),
        (
            "expense-month-1",
            "expense",
            70_000,
            Some("программы"),
            "-1 month",
        ),
        ("income-month-2", "income", 95_000, None, "-2 months"),
        (
            "expense-month-2",
            "expense",
            60_000,
            Some("реклама"),
            "-2 months",
        ),
        ("income-month-3", "income", 110_000, None, "-3 months"),
        (
            "expense-month-3",
            "expense",
            80_000,
            Some("оборудование"),
            "-3 months",
        ),
    ];

    let mut inserted = 0_u64;
    for (index, (key, kind, amount, category, date_modifier)) in demo_rows.into_iter().enumerate() {
        let event_id = format!("demo:{}:{key}", event.peer_id);
        inserted += sqlx::query(
            "INSERT OR IGNORE INTO operations
             (event_id, peer_id, author_id, message_id, kind, amount, category, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now', ?))",
        )
        .bind(event_id)
        .bind(event.peer_id)
        .bind(event.author_id)
        .bind(-((index as i64) + 1))
        .bind(kind)
        .bind(amount)
        .bind(category)
        .bind(date_modifier)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    }

    Ok(format!(
        "Тестовые данные готовы: добавлено {inserted} операций за текущий и три предыдущих месяца. Повторный запуск не создаёт дубли."
    ))
}

async fn expenses_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    peer_id: i64,
    days: u32,
) -> Result<String> {
    expense_groups_in_tx(tx, peer_id, days).await
}

async fn expense_groups_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    peer_id: i64,
    days: u32,
) -> Result<String> {
    let rows = sqlx::query(
        "SELECT COALESCE(category, 'без категории') AS category,
                SUM(amount) AS total
         FROM operations
         WHERE peer_id = ?
           AND kind = 'expense'
           AND cancelled_by_event_id IS NULL
           AND created_at >= datetime('now', ?)
         GROUP BY category
         ORDER BY total DESC, category ASC",
    )
    .bind(peer_id)
    .bind(format!("-{days} days"))
    .fetch_all(&mut **tx)
    .await?;

    let period = if days == 90 {
        "3 месяца".to_string()
    } else {
        format!("{days} дней")
    };
    if rows.is_empty() {
        return Ok(format!("За последние {period} расходов не было."));
    }

    let mut lines = vec![format!("Расходы по группам за {period}:")];
    for row in rows {
        let category: String = row.get("category");
        let total: i64 = row.get("total");
        lines.push(format!(
            "{}: {} ₽",
            capitalize_category(&category),
            format_money(total)
        ));
    }
    Ok(lines.join("\n"))
}

fn capitalize_category(category: &str) -> String {
    let mut chars = category.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_uppercase().chain(chars).collect()
}

async fn history_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    peer_id: i64,
    days: u32,
) -> Result<String> {
    let rows = sqlx::query(
        "SELECT amount,
                COALESCE(category, 'без категории') AS category,
                author_id,
                datetime(created_at, '+3 hours') AS created_at
         FROM operations
         WHERE peer_id = ?
           AND kind = 'expense'
           AND cancelled_by_event_id IS NULL
           AND created_at >= datetime('now', ?)
         ORDER BY created_at DESC, id DESC",
    )
    .bind(peer_id)
    .bind(format!("-{days} days"))
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Ok(format!("За последние {days} дней расходов не было."));
    }

    let total: i64 = rows.iter().map(|row| row.get::<i64, _>("amount")).sum();
    let period = match days {
        14 => "14 дней".to_string(),
        30 => "30 дней".to_string(),
        90 => "3 месяца".to_string(),
        _ => format!("{days} дней"),
    };
    let mut lines = vec![format!(
        "История расходов за последние {period}: {} ₽",
        format_money(total)
    )];
    for row in rows {
        let amount: i64 = row.get("amount");
        let category: String = row.get("category");
        let author_id: i64 = row.get("author_id");
        let created_at: String = row.get("created_at");
        lines.push(format!(
            "• {} · {} · {} ₽ · vk.com/id{}",
            &created_at[..created_at.len().min(10)],
            category,
            format_money(amount),
            author_id
        ));
    }
    Ok(lines.join("\n"))
}

async fn monthly_expenses_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    peer_id: i64,
    month: u8,
    requested_year: Option<i32>,
) -> Result<String> {
    if !(1..=12).contains(&month) {
        return Err(anyhow!("invalid calendar month: {month}"));
    }

    let current = sqlx::query(
        "SELECT CAST(strftime('%Y', 'now', '+3 hours') AS INTEGER) AS year,
                CAST(strftime('%m', 'now', '+3 hours') AS INTEGER) AS month",
    )
    .fetch_one(&mut **tx)
    .await?;
    let current_year = current.get::<i64, _>("year") as i32;
    let current_month = current.get::<i64, _>("month") as u8;
    let year = resolve_calendar_month_year(month, requested_year, current_year, current_month);
    let (start, end) = calendar_month_bounds(year, month);

    let rows = sqlx::query(
        "SELECT amount,
                COALESCE(category, 'без категории') AS category,
                author_id,
                datetime(created_at, '+3 hours') AS created_at
         FROM operations
         WHERE peer_id = ?
           AND kind = 'expense'
           AND cancelled_by_event_id IS NULL
           AND datetime(created_at, '+3 hours') >= ?
           AND datetime(created_at, '+3 hours') < ?
         ORDER BY created_at DESC, id DESC",
    )
    .bind(peer_id)
    .bind(&start)
    .bind(&end)
    .fetch_all(&mut **tx)
    .await?;

    let month_name = russian_month_name(month);
    if rows.is_empty() {
        return Ok(format!("За {month_name} {year} года расходов не было."));
    }

    let total: i64 = rows.iter().map(|row| row.get::<i64, _>("amount")).sum();
    let title = format!(
        "Расходы за {month_name} {year} года: {} ₽",
        format_money(total)
    );
    let lines = rows
        .iter()
        .map(|row| {
            let created_at: String = row.get("created_at");
            let category: String = row.get("category");
            let amount: i64 = row.get("amount");
            let author_id: i64 = row.get("author_id");
            format!(
                "• {} · {} · {} ₽ · vk.com/id{}",
                &created_at[..created_at.len().min(10)],
                capitalize_category(&category),
                format_money(amount),
                author_id
            )
        })
        .collect::<Vec<_>>();
    Ok(render_limited_expense_history(title, &lines))
}

fn resolve_calendar_month_year(
    month: u8,
    requested_year: Option<i32>,
    current_year: i32,
    current_month: u8,
) -> i32 {
    requested_year.unwrap_or_else(|| {
        if month > current_month {
            current_year - 1
        } else {
            current_year
        }
    })
}

fn calendar_month_bounds(year: i32, month: u8) -> (String, String) {
    let start = format!("{year:04}-{month:02}-01 00:00:00");
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let end = format!("{next_year:04}-{next_month:02}-01 00:00:00");
    (start, end)
}

fn russian_month_name(month: u8) -> &'static str {
    match month {
        1 => "январь",
        2 => "февраль",
        3 => "март",
        4 => "апрель",
        5 => "май",
        6 => "июнь",
        7 => "июль",
        8 => "август",
        9 => "сентябрь",
        10 => "октябрь",
        11 => "ноябрь",
        12 => "декабрь",
        _ => "неизвестный месяц",
    }
}

fn render_limited_expense_history(title: String, lines: &[String]) -> String {
    const BODY_LIMIT_CHARS: usize = 3_600;

    let mut message = title;
    let mut shown = 0;
    for line in lines {
        let next_length = message.chars().count() + 1 + line.chars().count();
        if next_length > BODY_LIMIT_CHARS {
            break;
        }
        message.push('\n');
        message.push_str(line);
        shown += 1;
    }

    let omitted = lines.len() - shown;
    if omitted > 0 {
        message.push_str(&format!(
            "\n… Не показано операций: {omitted}. Полная история доступна через выгрузку в Excel."
        ));
    }
    message
}

async fn cancel_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    event: &EventContext,
) -> Result<String> {
    let target = if let Some(reply_id) = event.reply_to_message_id {
        sqlx::query(
            "SELECT id, amount, category, kind FROM operations
             WHERE peer_id = ? AND author_id = ? AND message_id = ?
               AND kind != 'cancel' AND cancelled_by_event_id IS NULL
             ORDER BY id DESC LIMIT 1",
        )
        .bind(event.peer_id)
        .bind(event.author_id)
        .bind(reply_id)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        sqlx::query(
            "SELECT id, amount, category, kind FROM operations
             WHERE peer_id = ? AND author_id = ?
               AND kind != 'cancel' AND cancelled_by_event_id IS NULL
             ORDER BY id DESC LIMIT 1",
        )
        .bind(event.peer_id)
        .bind(event.author_id)
        .fetch_optional(&mut **tx)
        .await?
    };

    let Some(target) = target else {
        return Ok("Не нашла действующую собственную операцию для отмены.".into());
    };
    let target_id: i64 = target.get("id");
    let amount: i64 = target.get("amount");
    let category: Option<String> = target.get("category");
    let kind: String = target.get("kind");

    sqlx::query("UPDATE operations SET cancelled_by_event_id = ? WHERE id = ?")
        .bind(&event.event_id)
        .bind(target_id)
        .execute(&mut **tx)
        .await?;
    insert_operation(
        tx,
        event,
        "cancel",
        amount,
        category.as_deref(),
        Some(target_id),
    )
    .await?;

    let balance = balance_in_tx(tx, event.peer_id).await?;
    let operation_name = match kind.as_str() {
        "opening" => "начальный остаток",
        "income" => "доход",
        "expense" => "расход",
        _ => "операция",
    };
    Ok(format!(
        "Отменено: {operation_name} {} ₽. Остаток: {} ₽.",
        format_money(amount),
        format_money(balance)
    ))
}

fn help_text() -> String {
    [
        "🐰 Зая на связи!",
        "Могу записать приход или расход и показать, сколько денег в кассе.",
        "",
        "Мои команды:",
        "• Зая начальный остаток 50000",
        "• Зая доход 15000 (или Зая приход 15000)",
        "• Зая расход Материалы 4000",
        "• Зая баланс",
        "• Зая история расходов",
        "• Зая расходы за октябрь (или Зая расходы за октябрь 2026)",
        "• Зая расходы по группам",
        "• Зая бюджет по месяцам",
        "• Зая отмена — ответом на сообщение с операцией или для последней своей операции",
    ]
    .join("\n")
}

pub fn format_money(value: i64) -> String {
    let negative = value.is_negative();
    let digits = value.unsigned_abs().to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(' ');
        }
        result.push(ch);
    }
    if negative {
        result.insert(0, '−');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, message_id: i64) -> EventContext {
        EventContext {
            event_id: id.into(),
            peer_id: 42,
            author_id: 7,
            message_id,
            reply_to_message_id: None,
        }
    }

    #[test]
    fn formats_money() {
        assert_eq!(format_money(61_000), "61 000");
        assert_eq!(format_money(-4_000), "−4 000");
    }

    #[test]
    fn help_introduces_zaya_and_prefixed_commands() {
        let help = help_text();
        assert!(help.starts_with("🐰 Зая на связи!"));
        assert!(help.contains("Зая баланс"));
        assert!(help.contains("Зая расход Материалы 4000"));
        assert!(help.contains("Зая расходы за октябрь"));
    }

    #[test]
    fn resolves_omitted_calendar_year_to_latest_started_month() {
        assert_eq!(resolve_calendar_month_year(10, None, 2026, 10), 2026);
        assert_eq!(resolve_calendar_month_year(11, None, 2026, 10), 2025);
        assert_eq!(resolve_calendar_month_year(12, Some(2024), 2026, 10), 2024);
    }

    #[test]
    fn calendar_month_bounds_handle_year_rollover() {
        assert_eq!(
            calendar_month_bounds(2026, 10),
            ("2026-10-01 00:00:00".into(), "2026-11-01 00:00:00".into())
        );
        assert_eq!(
            calendar_month_bounds(2026, 12),
            ("2026-12-01 00:00:00".into(), "2027-01-01 00:00:00".into())
        );
    }

    #[test]
    fn truncates_oversized_monthly_history_safely() {
        let lines = (0..500)
            .map(|index| {
                format!(
                    "• 2026-10-31 · Очень длинная категория номер {index} · 100 000 ₽ · vk.com/id7"
                )
            })
            .collect::<Vec<_>>();
        let rendered = render_limited_expense_history("Расходы за октябрь".into(), &lines);
        assert!(rendered.chars().count() < 4_096);
        assert!(rendered.contains("Не показано операций"));
    }

    #[tokio::test]
    async fn monthly_expenses_respect_moscow_boundaries_and_conversation() {
        let repository = Repository::connect("sqlite::memory:").await.unwrap();
        repository.migrate().await.unwrap();

        for (event_id, peer_id, category, amount, created_at, cancelled) in [
            (
                "before-october",
                42,
                "до месяца",
                500,
                "2026-09-30 20:59:59",
                None,
            ),
            (
                "october-start",
                42,
                "начало месяца",
                1_000,
                "2026-09-30 21:00:00",
                None,
            ),
            (
                "october-end",
                42,
                "конец месяца",
                2_000,
                "2026-10-31 20:59:59",
                None,
            ),
            (
                "after-october",
                42,
                "после месяца",
                4_000,
                "2026-10-31 21:00:00",
                None,
            ),
            (
                "cancelled-october",
                42,
                "отменённая",
                8_000,
                "2026-10-15 10:00:00",
                Some("cancel-event"),
            ),
            (
                "other-chat-october",
                99,
                "другая беседа",
                16_000,
                "2026-10-15 10:00:00",
                None,
            ),
        ] {
            sqlx::query(
                "INSERT INTO operations
                 (event_id, peer_id, author_id, message_id, kind, amount, category,
                  cancelled_by_event_id, created_at)
                 VALUES (?, ?, 7, 1, 'expense', ?, ?, ?, ?)",
            )
            .bind(event_id)
            .bind(peer_id)
            .bind(amount)
            .bind(category)
            .bind(cancelled)
            .bind(created_at)
            .execute(&repository.pool)
            .await
            .unwrap();
        }

        let outcome = repository
            .execute(
                &event("october-report", 10),
                &Command::MonthlyExpenses {
                    month: 10,
                    year: Some(2026),
                },
            )
            .await
            .unwrap();
        let ExecutionOutcome::Reply { message, keyboard } = outcome else {
            panic!("expected monthly expense reply");
        };
        assert_eq!(keyboard, ReplyKeyboard::Reports);
        assert!(message.contains("Расходы за октябрь 2026 года: 3 000 ₽"));
        assert!(message.contains("Начало месяца"));
        assert!(message.contains("Конец месяца"));
        assert!(!message.contains("До месяца"));
        assert!(!message.contains("После месяца"));
        assert!(!message.contains("Отменённая"));
        assert!(!message.contains("Другая беседа"));
        assert!(message.find("Конец месяца") < message.find("Начало месяца"));

        let empty = repository
            .execute(
                &event("august-report", 11),
                &Command::MonthlyExpenses {
                    month: 8,
                    year: Some(2026),
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            empty,
            ExecutionOutcome::Reply { message, .. }
                if message == "За август 2026 года расходов не было."
        ));
    }

    #[tokio::test]
    async fn balances_are_isolated_by_conversation() {
        let repository = Repository::connect("sqlite::memory:").await.unwrap();
        repository.migrate().await.unwrap();

        let mut first_chat = event("chat-1-opening", 1);
        first_chat.peer_id = 101;
        repository
            .execute(&first_chat, &Command::OpeningBalance(10_000))
            .await
            .unwrap();

        let mut second_chat = event("chat-2-opening", 1);
        second_chat.peer_id = 202;
        repository
            .execute(&second_chat, &Command::OpeningBalance(25_000))
            .await
            .unwrap();

        let mut first_balance = event("chat-1-balance", 2);
        first_balance.peer_id = 101;
        let first = repository
            .execute(&first_balance, &Command::Balance)
            .await
            .unwrap();
        assert!(matches!(
            first,
            ExecutionOutcome::Reply { message, .. } if message.contains("10 000")
        ));

        let mut second_balance = event("chat-2-balance", 2);
        second_balance.peer_id = 202;
        let second = repository
            .execute(&second_balance, &Command::Balance)
            .await
            .unwrap();
        assert!(matches!(
            second,
            ExecutionOutcome::Reply { message, .. } if message.contains("25 000")
        ));
    }

    #[tokio::test]
    async fn spec_example_and_duplicate_event() {
        let repository = Repository::connect("sqlite::memory:").await.unwrap();
        repository.migrate().await.unwrap();
        repository
            .execute(&event("1", 1), &Command::OpeningBalance(50_000))
            .await
            .unwrap();
        repository
            .execute(&event("2", 2), &Command::Income(15_000))
            .await
            .unwrap();
        repository
            .execute(
                &event("3", 3),
                &Command::Expense {
                    category: "материалы".into(),
                    amount: 4_000,
                },
            )
            .await
            .unwrap();

        let outcome = repository
            .execute(&event("4", 4), &Command::Balance)
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            ExecutionOutcome::Reply { message, .. } if message.contains("61 000")
        ));
        let history = repository
            .execute(&event("5", 5), &Command::History(14))
            .await
            .unwrap();
        assert!(matches!(
            history,
            ExecutionOutcome::Reply { message, .. }
                if message.contains("материалы") && message.contains("4 000")
        ));
        let expenses = repository
            .execute(&event("6", 6), &Command::Expenses(14))
            .await
            .unwrap();
        assert!(matches!(
            expenses,
            ExecutionOutcome::Reply {
                message,
                keyboard: ReplyKeyboard::ExpenseGroups(14),
            } if message.contains("за 14 дней")
        ));
        let duplicate = repository
            .execute(&event("3", 3), &Command::Income(999_999))
            .await
            .unwrap();
        assert_eq!(duplicate, ExecutionOutcome::Duplicate);
    }

    #[tokio::test]
    async fn groups_expenses_by_category_for_selected_period() {
        let repository = Repository::connect("sqlite::memory:").await.unwrap();
        repository.migrate().await.unwrap();

        for (event_id, message_id, category, amount) in [
            ("expense-1", 1, "материалы", 6_000),
            ("expense-2", 2, "материалы", 4_000),
            ("expense-3", 3, "зарплата аниматорам", 10_000),
        ] {
            repository
                .execute(
                    &event(event_id, message_id),
                    &Command::Expense {
                        category: category.into(),
                        amount,
                    },
                )
                .await
                .unwrap();
        }

        sqlx::query(
            "UPDATE operations
             SET created_at = datetime('now', '-20 days')
             WHERE event_id = 'expense-3'",
        )
        .execute(&repository.pool)
        .await
        .unwrap();

        let recent = repository
            .execute(&event("statistics-14", 4), &Command::Expenses(14))
            .await
            .unwrap();
        assert!(matches!(
            recent,
            ExecutionOutcome::Reply {
                message,
                keyboard: ReplyKeyboard::ExpenseGroups(14),
            } if message.contains("Материалы: 10 000 ₽")
                && !message.contains("Зарплата аниматорам")
        ));

        let outcome = repository
            .execute(&event("statistics-30", 5), &Command::Expenses(30))
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            ExecutionOutcome::Reply { message, .. }
                if message.contains("Расходы по группам")
                    && message.contains("за 30 дней")
                    && message.contains("Материалы: 10 000 ₽")
                    && message.contains("Зарплата аниматорам: 10 000 ₽")
        ));
    }

    #[tokio::test]
    async fn history_is_sorted_by_operation_date_from_newest_to_oldest() {
        let repository = Repository::connect("sqlite::memory:").await.unwrap();
        repository.migrate().await.unwrap();

        repository
            .execute(
                &event("newer-expense", 1),
                &Command::Expense {
                    category: "новая запись".into(),
                    amount: 1_000,
                },
            )
            .await
            .unwrap();
        repository
            .execute(
                &event("older-expense", 2),
                &Command::Expense {
                    category: "старая запись".into(),
                    amount: 2_000,
                },
            )
            .await
            .unwrap();

        sqlx::query(
            "UPDATE operations
             SET created_at = CASE event_id
                 WHEN 'newer-expense' THEN datetime('now', '-1 day')
                 WHEN 'older-expense' THEN datetime('now', '-10 days')
             END
             WHERE event_id IN ('newer-expense', 'older-expense')",
        )
        .execute(&repository.pool)
        .await
        .unwrap();

        let outcome = repository
            .execute(&event("ordered-history", 3), &Command::History(14))
            .await
            .unwrap();
        let ExecutionOutcome::Reply { message, .. } = outcome else {
            panic!("expected a text history response");
        };
        let newer_position = message.find("новая запись").unwrap();
        let older_position = message.find("старая запись").unwrap();
        assert!(newer_position < older_position);
    }

    #[tokio::test]
    async fn seeds_demo_months_idempotently_and_builds_budget_data() {
        let repository = Repository::connect("sqlite::memory:").await.unwrap();
        repository.migrate().await.unwrap();

        let first = repository
            .execute(&event("seed-command-1", 1), &Command::SeedDemo)
            .await
            .unwrap();
        assert!(matches!(
            first,
            ExecutionOutcome::Reply { message, .. } if message.contains("добавлено 11 операций")
        ));

        let chart = repository
            .execute(&event("chart", 2), &Command::BudgetStatistics)
            .await
            .unwrap();
        assert!(matches!(
            chart,
            ExecutionOutcome::BudgetChart { months, .. }
                if months.len() == 4
                    && months.iter().any(|month| month.income > 0)
                    && months.iter().any(|month| month.expenses > 0)
        ));

        let second = repository
            .execute(&event("seed-command-2", 3), &Command::SeedDemo)
            .await
            .unwrap();
        assert!(matches!(
            second,
            ExecutionOutcome::Reply { message, .. } if message.contains("добавлено 0 операций")
        ));
    }

    #[tokio::test]
    async fn exports_full_financial_history_with_running_balance() {
        let repository = Repository::connect("sqlite::memory:").await.unwrap();
        repository.migrate().await.unwrap();
        repository.migrate().await.unwrap();

        repository
            .execute(
                &event("opening-export", 1),
                &Command::OpeningBalance(50_000),
            )
            .await
            .unwrap();
        repository
            .execute(&event("income-export", 2), &Command::Income(15_000))
            .await
            .unwrap();
        repository
            .execute(
                &event("expense-export", 3),
                &Command::Expense {
                    category: "материалы".into(),
                    amount: 4_000,
                },
            )
            .await
            .unwrap();

        let outcome = repository
            .execute(
                &event("excel-export", 4),
                &Command::ExportExcel(crate::domain::ExportScope::FullHistory),
            )
            .await
            .unwrap();
        let ExecutionOutcome::Excel { rows, keyboard } = outcome else {
            panic!("expected an Excel export");
        };
        assert_eq!(keyboard, ReplyKeyboard::CurrentBalance);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].operation_type, "Начальный остаток");
        assert_eq!(rows[0].income, Some(50_000));
        assert_eq!(rows[0].balance, 50_000);
        assert_eq!(rows[1].operation_type, "Приход");
        assert_eq!(rows[1].balance, 65_000);
        assert_eq!(rows[2].operation_type, "Расход — Материалы");
        assert_eq!(rows[2].expense, Some(4_000));
        assert_eq!(rows[2].balance, 61_000);

        let pending = repository.pending_outbox(10).await.unwrap();
        assert!(pending.iter().any(|item| {
            item.event_id == "excel-export"
                && matches!(&item.payload, OutboxPayload::Excel(rows) if rows.len() == 3)
        }));
    }

    #[tokio::test]
    async fn upgrades_existing_outbox_schema_for_excel_payloads() {
        let repository = Repository::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(include_str!("../migrations/0001_init.sql"))
            .execute(&repository.pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE outbox (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                peer_id INTEGER NOT NULL,
                response_type TEXT NOT NULL CHECK (response_type IN ('text', 'budget_chart')),
                payload TEXT NOT NULL,
                keyboard TEXT NOT NULL,
                attempt_count INTEGER NOT NULL DEFAULT 0,
                available_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                last_error TEXT,
                sent_at TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );
             INSERT INTO outbox (event_id, peer_id, response_type, payload, keyboard)
             VALUES ('saved-reply', 42, 'text', 'Сохранённый ответ', 'none');",
        )
        .execute(&repository.pool)
        .await
        .unwrap();

        repository.migrate().await.unwrap();

        let schema: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'outbox'",
        )
        .fetch_one(&repository.pool)
        .await
        .unwrap();
        assert!(schema.contains("'excel'"));
        let saved_payload: String =
            sqlx::query_scalar("SELECT payload FROM outbox WHERE event_id = 'saved-reply'")
                .fetch_one(&repository.pool)
                .await
                .unwrap();
        assert_eq!(saved_payload, "Сохранённый ответ");
    }

    #[tokio::test]
    async fn outbox_retries_and_marks_delivery() {
        let repository = Repository::connect("sqlite::memory:").await.unwrap();
        repository.migrate().await.unwrap();

        repository
            .execute(&event("outbox-event", 1), &Command::Income(10_000))
            .await
            .unwrap();

        let pending = repository.pending_outbox(10).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].event_id, "outbox-event");
        assert_eq!(pending[0].keyboard, ReplyKeyboard::OperationResult);
        assert!(matches!(
            &pending[0].payload,
            OutboxPayload::Text(message) if message.contains("10 000")
        ));

        repository
            .mark_outbox_failed("outbox-event", "temporary VK error", 30)
            .await
            .unwrap();
        assert!(repository.pending_outbox(10).await.unwrap().is_empty());

        repository.mark_outbox_sent("outbox-event").await.unwrap();
        let (attempt_count, sent): (i64, i64) = sqlx::query_as(
            "SELECT attempt_count, sent_at IS NOT NULL FROM outbox WHERE event_id = ?",
        )
        .bind("outbox-event")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
        assert_eq!(attempt_count, 1);
        assert_eq!(sent, 1);
    }
}
