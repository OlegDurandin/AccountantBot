use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    OpeningBalance(i64),
    Income(i64),
    Expense { category: String, amount: i64 },
    Balance,
    Expenses(u32),
    History(u32),
    BudgetStatistics,
    ExportExcel(ExportScope),
    SeedDemo,
    Cancel,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyKeyboard {
    None,
    OperationResult,
    ExpenseGroups,
    ExpenseHistory(u32),
}

impl ReplyKeyboard {
    pub fn storage_key(self) -> String {
        match self {
            Self::None => "none".into(),
            Self::OperationResult => "operation_result".into(),
            Self::ExpenseGroups => "expense_groups".into(),
            Self::ExpenseHistory(days) => format!("expense_history:{days}"),
        }
    }

    pub fn from_storage_key(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "operation_result" => Some(Self::OperationResult),
            "expense_groups" => Some(Self::ExpenseGroups),
            _ => value
                .strip_prefix("expense_history:")?
                .parse::<u32>()
                .ok()
                .map(Self::ExpenseHistory),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    Expenses,
    FullHistory,
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::OpeningBalance(_) => "opening_balance",
            Self::Income(_) => "income",
            Self::Expense { .. } => "expense",
            Self::Balance => "balance",
            Self::Expenses(7) => "expenses_7_days",
            Self::Expenses(30) => "expenses_30_days",
            Self::Expenses(_) => "expenses_custom_period",
            Self::History(14) => "history_14_days",
            Self::History(30) => "history_30_days",
            Self::History(90) => "history_3_months",
            Self::History(_) => "history_custom_period",
            Self::BudgetStatistics => "budget_statistics",
            Self::ExportExcel(ExportScope::Expenses) => "export_expenses_excel",
            Self::ExportExcel(ExportScope::FullHistory) => "export_full_history_excel",
            Self::SeedDemo => "seed_demo",
            Self::Cancel => "cancel",
            Self::Help => "help",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseCommandError(pub String);

impl fmt::Display for ParseCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn parse_command(text: &str) -> Result<Option<Command>, ParseCommandError> {
    let text = text.trim();
    let lower = text.to_lowercase();

    if matches!(lower.as_str(), "баланс" | "остаток" | "текущий баланс") {
        return Ok(Some(Command::Balance));
    }
    if matches!(
        lower.as_str(),
        "статистика бюджета" | "доходы и расходы по месяцам" | "бюджет по месяцам"
    ) {
        return Ok(Some(Command::BudgetStatistics));
    }
    if matches!(
        lower.as_str(),
        "статьи расходов"
            | "график расходов"
            | "статистика расходов"
            | "статистика расходов (по группам)"
            | "расходы по группам"
    ) {
        return Ok(Some(Command::Expenses(7)));
    }
    if matches!(lower.as_str(), "расходы" | "история" | "история расходов")
    {
        return Ok(Some(Command::History(14)));
    }
    if matches!(
        lower.as_str(),
        "история 14 дней" | "история за 14 дней" | "история: 14 дней" | "14 дней"
    ) {
        return Ok(Some(Command::History(14)));
    }
    if matches!(
        lower.as_str(),
        "история 30 дней" | "история за 30 дней" | "история: 30 дней" | "30 дней"
    ) {
        return Ok(Some(Command::History(30)));
    }
    if matches!(
        lower.as_str(),
        "история 3 месяца" | "история за 3 месяца" | "история: 3 месяца" | "3 месяца"
    ) {
        return Ok(Some(Command::History(90)));
    }
    if matches!(lower.as_str(), "история за год" | "история год") {
        return Ok(Some(Command::History(365)));
    }
    if lower == "выгрузить всю историю в excel" {
        return Ok(Some(Command::ExportExcel(ExportScope::FullHistory)));
    }
    if matches!(
        lower.as_str(),
        "выгрузить в excel" | "выгрузка в excel" | "excel"
    ) {
        return Ok(Some(Command::ExportExcel(ExportScope::Expenses)));
    }
    if matches!(
        lower.as_str(),
        "тестовые данные" | "заполнить тестовыми данными"
    ) {
        return Ok(Some(Command::SeedDemo));
    }
    if lower == "отмена" {
        return Ok(Some(Command::Cancel));
    }
    if matches!(lower.as_str(), "помощь" | "help" | "команды" | "начать") {
        return Ok(Some(Command::Help));
    }

    if let Some(value) = lower.strip_prefix("начальный остаток ") {
        return parse_amount(value).map(Command::OpeningBalance).map(Some);
    }
    for prefix in ["доход ", "приход "] {
        if let Some(value) = lower.strip_prefix(prefix) {
            return parse_amount(value).map(Command::Income).map(Some);
        }
    }
    if let Some(rest) = lower.strip_prefix("расход ") {
        let Some((category, amount)) = rest.rsplit_once(char::is_whitespace) else {
            return Err(ParseCommandError(
                "Формат расхода: Расход <категория> <сумма>".into(),
            ));
        };
        let category = normalize_category(category);
        if category.is_empty() {
            return Err(ParseCommandError("Укажите категорию расхода".into()));
        }
        return Ok(Some(Command::Expense {
            category,
            amount: parse_amount(amount)?,
        }));
    }

    Ok(None)
}

pub fn normalize_category(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn parse_amount(value: &str) -> Result<i64, ParseCommandError> {
    let normalized = value.trim().replace('_', "");
    let amount = normalized.parse::<i64>().map_err(|_| {
        ParseCommandError("Сумма должна быть положительным целым числом рублей".into())
    })?;
    if amount <= 0 {
        return Err(ParseCommandError("Сумма должна быть больше нуля".into()));
    }
    Ok(amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spec_examples() {
        assert_eq!(
            parse_command("Начальный остаток 50000").unwrap(),
            Some(Command::OpeningBalance(50_000))
        );
        assert_eq!(
            parse_command("Приход 15000").unwrap(),
            Some(Command::Income(15_000))
        );
        assert_eq!(
            parse_command("Расход Расходные материалы 4000").unwrap(),
            Some(Command::Expense {
                category: "расходные материалы".into(),
                amount: 4_000,
            })
        );
    }

    #[test]
    fn ignores_regular_conversation_messages() {
        assert_eq!(parse_command("Кто сегодня работает?").unwrap(), None);
    }

    #[test]
    fn parses_expense_history_periods() {
        assert_eq!(
            parse_command("Статьи расходов").unwrap(),
            Some(Command::Expenses(7))
        );
        assert_eq!(
            parse_command("История расходов").unwrap(),
            Some(Command::History(14))
        );
        assert_eq!(
            parse_command("История: 3 месяца").unwrap(),
            Some(Command::History(90))
        );
        assert_eq!(
            parse_command("История: 30 дней").unwrap(),
            Some(Command::History(30))
        );
        assert_eq!(
            parse_command("Выгрузить всю историю в Excel").unwrap(),
            Some(Command::ExportExcel(ExportScope::FullHistory))
        );
        assert_eq!(
            parse_command("Текущий баланс").unwrap(),
            Some(Command::Balance)
        );
        assert_eq!(
            parse_command("Статистика расходов (по группам)").unwrap(),
            Some(Command::Expenses(7))
        );
        assert_eq!(
            parse_command("Доходы и расходы по месяцам").unwrap(),
            Some(Command::BudgetStatistics)
        );
        assert_eq!(
            parse_command("Расходы по группам").unwrap(),
            Some(Command::Expenses(7))
        );
        assert_eq!(
            parse_command("Бюджет по месяцам").unwrap(),
            Some(Command::BudgetStatistics)
        );
        assert_eq!(
            parse_command("14 дней").unwrap(),
            Some(Command::History(14))
        );
        assert_eq!(
            parse_command("30 дней").unwrap(),
            Some(Command::History(30))
        );
        assert_eq!(
            parse_command("3 месяца").unwrap(),
            Some(Command::History(90))
        );
    }

    #[test]
    fn categories_are_case_insensitive() {
        assert_eq!(normalize_category("  Зарплата  "), "зарплата");
    }

    #[test]
    fn reply_keyboards_round_trip_through_outbox_storage() {
        for keyboard in [
            ReplyKeyboard::None,
            ReplyKeyboard::OperationResult,
            ReplyKeyboard::ExpenseGroups,
            ReplyKeyboard::ExpenseHistory(90),
        ] {
            assert_eq!(
                ReplyKeyboard::from_storage_key(&keyboard.storage_key()),
                Some(keyboard)
            );
        }
    }
}
