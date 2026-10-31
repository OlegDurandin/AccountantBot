use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    OpeningBalance(i64),
    Income(i64),
    Expense { category: String, amount: i64 },
    Balance,
    ConversationalBalance,
    Expenses(u32),
    History(u32),
    MonthlyExpenses { month: u8, year: Option<i32> },
    BudgetStatistics,
    ExportExcel(ExportScope),
    SeedDemo,
    DeleteDemo,
    Cancel,
    Thanks,
    Praise,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyKeyboard {
    None,
    CurrentBalance,
    OperationResult,
    Reports,
    ExpenseGroups(u32),
    ExpenseHistory(u32),
}

impl ReplyKeyboard {
    pub fn storage_key(self) -> String {
        match self {
            Self::None => "none".into(),
            Self::CurrentBalance => "current_balance".into(),
            Self::OperationResult => "operation_result".into(),
            Self::Reports => "reports".into(),
            Self::ExpenseGroups(days) => format!("expense_groups:{days}"),
            Self::ExpenseHistory(days) => format!("expense_history:{days}"),
        }
    }

    pub fn from_storage_key(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "current_balance" => Some(Self::CurrentBalance),
            "operation_result" => Some(Self::OperationResult),
            "reports" | "expense_groups" => Some(Self::Reports),
            _ => {
                if let Some(days) = value.strip_prefix("expense_groups:") {
                    return days.parse::<u32>().ok().map(Self::ExpenseGroups);
                }
                value
                    .strip_prefix("expense_history:")?
                    .parse::<u32>()
                    .ok()
                    .map(Self::ExpenseHistory)
            }
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
            Self::ConversationalBalance => "conversational_balance",
            Self::Expenses(14) => "expenses_14_days",
            Self::Expenses(30) => "expenses_30_days",
            Self::Expenses(90) => "expenses_3_months",
            Self::Expenses(_) => "expenses_custom_period",
            Self::History(14) => "history_14_days",
            Self::History(30) => "history_30_days",
            Self::History(90) => "history_3_months",
            Self::History(_) => "history_custom_period",
            Self::MonthlyExpenses { .. } => "expenses_calendar_month",
            Self::BudgetStatistics => "budget_statistics",
            Self::ExportExcel(ExportScope::Expenses) => "export_expenses_excel",
            Self::ExportExcel(ExportScope::FullHistory) => "export_full_history_excel",
            Self::SeedDemo => "seed_demo",
            Self::DeleteDemo => "delete_demo",
            Self::Cancel => "cancel",
            Self::Thanks => "thanks",
            Self::Praise => "praise",
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
    let lower = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let addressed = strip_bot_address(&lower);
    let lower = match addressed {
        Some("") => "помощь",
        Some(command) => command,
        None => &lower,
    };
    let lower = lower.trim_end_matches(['.', ',', '!', '?', ':', ';']);

    if addressed.is_some()
        && [
            "сколько у нас осталось денюжек",
            "сколько у нас осталось денежек",
            "сколько у нас осталось денег",
            "сколько осталось денюжек",
            "сколько осталось денежек",
            "сколько осталось денег",
            "сколько у нас денюжек",
            "сколько у нас денежек",
            "сколько у нас денег",
        ]
        .iter()
        .any(|phrase| matches_friendly_phrase(lower, phrase))
    {
        return Ok(Some(Command::ConversationalBalance));
    }
    if addressed.is_some()
        && ["спасибо", "спасибочки", "благодарю"]
            .iter()
            .any(|phrase| matches_friendly_phrase(lower, phrase))
    {
        return Ok(Some(Command::Thanks));
    }
    if addressed.is_some()
        && [
            "ты молодец",
            "молодец",
            "ты умница",
            "умница",
            "ты молодчинка",
        ]
        .iter()
        .any(|phrase| matches_friendly_phrase(lower, phrase))
    {
        return Ok(Some(Command::Praise));
    }

    if matches!(lower, "баланс" | "остаток" | "текущий баланс") {
        return Ok(Some(Command::Balance));
    }
    if matches!(
        lower,
        "статистика бюджета" | "доходы и расходы по месяцам" | "бюджет по месяцам"
    ) {
        return Ok(Some(Command::BudgetStatistics));
    }
    if matches!(
        lower,
        "статьи расходов"
            | "график расходов"
            | "статистика расходов"
            | "статистика расходов (по группам)"
            | "расходы по группам"
    ) {
        return Ok(Some(Command::Expenses(14)));
    }
    if lower == "расходы за" {
        return Err(ParseCommandError(
            "Укажите месяц: Зая расходы за октябрь или Зая расходы за октябрь 2026".into(),
        ));
    }
    if let Some(period) = lower.strip_prefix("расходы за ") {
        return parse_calendar_month(period).map(Some);
    }
    if matches!(lower, "расходы" | "история" | "история расходов") {
        return Ok(Some(Command::History(14)));
    }
    if matches!(
        lower,
        "история 14 дней" | "история за 14 дней" | "история: 14 дней" | "14 дней"
    ) {
        return Ok(Some(Command::History(14)));
    }
    if matches!(
        lower,
        "история 30 дней" | "история за 30 дней" | "история: 30 дней" | "30 дней"
    ) {
        return Ok(Some(Command::History(30)));
    }
    if matches!(
        lower,
        "история 3 месяца" | "история за 3 месяца" | "история: 3 месяца" | "3 месяца"
    ) {
        return Ok(Some(Command::History(90)));
    }
    if matches!(lower, "история за год" | "история год") {
        return Ok(Some(Command::History(365)));
    }
    if lower == "выгрузить всю историю в excel" {
        return Ok(Some(Command::ExportExcel(ExportScope::FullHistory)));
    }
    if matches!(lower, "выгрузить в excel" | "выгрузка в excel" | "excel") {
        return Ok(Some(Command::ExportExcel(ExportScope::Expenses)));
    }
    if matches!(lower, "тестовые данные" | "заполнить тестовыми данными")
    {
        return Ok(Some(Command::SeedDemo));
    }
    if matches!(
        lower,
        "удалить тестовые данные" | "очистить тестовые данные"
    ) {
        return Ok(Some(Command::DeleteDemo));
    }
    if lower == "отмена" {
        return Ok(Some(Command::Cancel));
    }
    if matches!(lower, "помощь" | "help" | "команды" | "начать") {
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

fn parse_calendar_month(value: &str) -> Result<Command, ParseCommandError> {
    let value = value.trim().trim_end_matches(['.', ',', '!', '?']);
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.is_empty() {
        return Err(ParseCommandError(
            "Укажите месяц: Зая расходы за октябрь".into(),
        ));
    }
    if parts.len() > 2 {
        return Err(ParseCommandError(
            "Формат команды: Зая расходы за <месяц> [год]".into(),
        ));
    }

    let month = parse_russian_month(parts[0]).ok_or_else(|| {
        ParseCommandError(format!(
            "Неизвестный месяц «{}». Например: Зая расходы за октябрь",
            parts[0]
        ))
    })?;
    let year = parts
        .get(1)
        .map(|value| {
            if value.len() != 4 || !value.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(ParseCommandError(
                    "Год укажите четырьмя цифрами, например 2026".into(),
                ));
            }
            let year = value.parse::<i32>().map_err(|_| {
                ParseCommandError("Не удалось распознать год. Например: 2026".into())
            })?;
            if !(2000..=2100).contains(&year) {
                return Err(ParseCommandError(
                    "Год должен быть в диапазоне от 2000 до 2100".into(),
                ));
            }
            Ok(year)
        })
        .transpose()?;

    Ok(Command::MonthlyExpenses { month, year })
}

fn parse_russian_month(value: &str) -> Option<u8> {
    match value {
        "январь" | "января" => Some(1),
        "февраль" | "февраля" => Some(2),
        "март" | "марта" => Some(3),
        "апрель" | "апреля" => Some(4),
        "май" | "мая" => Some(5),
        "июнь" | "июня" => Some(6),
        "июль" | "июля" => Some(7),
        "август" | "августа" => Some(8),
        "сентябрь" | "сентября" => Some(9),
        "октябрь" | "октября" => Some(10),
        "ноябрь" | "ноября" => Some(11),
        "декабрь" | "декабря" => Some(12),
        _ => None,
    }
}

fn strip_bot_address(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("зая")?;
    if rest.is_empty() {
        return Some("");
    }
    let first = rest.chars().next()?;
    if !first.is_whitespace() && !matches!(first, ',' | ':' | '!' | '?' | '.') {
        return None;
    }
    Some(rest.trim_start_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, ',' | ':' | '!' | '?' | '.')
    }))
}

fn matches_friendly_phrase(value: &str, phrase: &str) -> bool {
    value == phrase
        || value
            .strip_prefix(phrase)
            .and_then(|suffix| suffix.chars().next())
            .is_some_and(|ch| !ch.is_alphanumeric())
}

pub fn parse_button_payload(payload: &str) -> Option<Command> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    match value.get("command")?.as_str()? {
        "expense_groups_14" => Some(Command::Expenses(14)),
        "expense_groups_30" => Some(Command::Expenses(30)),
        "expense_groups_90" => Some(Command::Expenses(90)),
        "expense_history_14" => Some(Command::History(14)),
        "expense_history_30" => Some(Command::History(30)),
        "expense_history_90" => Some(Command::History(90)),
        _ => None,
    }
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
        assert_eq!(parse_command("Заяц баланс").unwrap(), None);
        assert_eq!(
            parse_command("Сколько у нас осталось денюжек?").unwrap(),
            None
        );
        assert_eq!(parse_command("Спасибо!").unwrap(), None);
    }

    #[test]
    fn understands_friendly_addressed_phrases() {
        for phrase in [
            "Зая, сколько у нас осталось денюжек?",
            "зая сколько у нас осталось денег",
            "ЗАЯ: сколько осталось денежек!",
        ] {
            assert_eq!(
                parse_command(phrase).unwrap(),
                Some(Command::ConversationalBalance),
                "phrase: {phrase}"
            );
        }
        for phrase in [
            "Зая, спасибо!",
            "зая спасибочки",
            "ЗАЯ: благодарю",
            "Зая, спасибо большое 💛",
        ] {
            assert_eq!(
                parse_command(phrase).unwrap(),
                Some(Command::Thanks),
                "phrase: {phrase}"
            );
        }
        for phrase in [
            "Зая, ты молодец!",
            "зая умница",
            "ЗАЯ: ты молодчинка",
            "Зая, ты молодец 😅",
        ] {
            assert_eq!(
                parse_command(phrase).unwrap(),
                Some(Command::Praise),
                "phrase: {phrase}"
            );
        }
    }

    #[test]
    fn accepts_case_insensitive_bot_address() {
        assert_eq!(parse_command("Зая").unwrap(), Some(Command::Help));
        assert_eq!(parse_command("зая").unwrap(), Some(Command::Help));
        assert_eq!(parse_command("ЗАЯ баланс").unwrap(), Some(Command::Balance));
        assert_eq!(
            parse_command("зая, расход Материалы 1000").unwrap(),
            Some(Command::Expense {
                category: "материалы".into(),
                amount: 1_000,
            })
        );
        assert_eq!(
            parse_command("Зая удалить тестовые данные").unwrap(),
            Some(Command::DeleteDemo)
        );
        assert_eq!(
            parse_command("очистить тестовые данные").unwrap(),
            Some(Command::DeleteDemo)
        );
    }

    #[test]
    fn parses_expense_history_periods() {
        assert_eq!(
            parse_command("Статьи расходов").unwrap(),
            Some(Command::Expenses(14))
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
            Some(Command::Expenses(14))
        );
        assert_eq!(
            parse_command("Доходы и расходы по месяцам").unwrap(),
            Some(Command::BudgetStatistics)
        );
        assert_eq!(
            parse_command("Расходы по группам").unwrap(),
            Some(Command::Expenses(14))
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
    fn parses_all_calendar_months_and_grammatical_forms() {
        for (month, nominative, genitive) in [
            (1, "январь", "января"),
            (2, "февраль", "февраля"),
            (3, "март", "марта"),
            (4, "апрель", "апреля"),
            (5, "май", "мая"),
            (6, "июнь", "июня"),
            (7, "июль", "июля"),
            (8, "август", "августа"),
            (9, "сентябрь", "сентября"),
            (10, "октябрь", "октября"),
            (11, "ноябрь", "ноября"),
            (12, "декабрь", "декабря"),
        ] {
            for name in [nominative, genitive] {
                assert_eq!(
                    parse_command(&format!("Расходы за {name}")).unwrap(),
                    Some(Command::MonthlyExpenses { month, year: None })
                );
            }
        }
    }

    #[test]
    fn parses_calendar_month_with_address_year_and_punctuation() {
        assert_eq!(
            parse_command("Зая, расходы за Октябрь 2026!").unwrap(),
            Some(Command::MonthlyExpenses {
                month: 10,
                year: Some(2026),
            })
        );
        assert_eq!(
            parse_command("  зая   расходы за   ноября\n2025  ").unwrap(),
            Some(Command::MonthlyExpenses {
                month: 11,
                year: Some(2025),
            })
        );
    }

    #[test]
    fn rejects_invalid_calendar_month_commands() {
        for command in [
            "Расходы за",
            "Расходы за октобрь",
            "Расходы за октябрь 26",
            "Расходы за октябрь 1999",
            "Расходы за октябрь 2101",
            "Расходы за октябрь 2026 лишнее",
        ] {
            assert!(parse_command(command).is_err(), "command: {command}");
        }
        assert_eq!(parse_command("Расходы заработок").unwrap(), None);
    }

    #[test]
    fn parses_period_button_payloads_without_ambiguous_labels() {
        assert_eq!(
            parse_button_payload(r#"{"command":"expense_groups_30"}"#),
            Some(Command::Expenses(30))
        );
        assert_eq!(
            parse_button_payload(r#"{"command":"expense_history_30"}"#),
            Some(Command::History(30))
        );
        assert_eq!(parse_button_payload("not-json"), None);
    }

    #[test]
    fn categories_are_case_insensitive() {
        assert_eq!(normalize_category("  Зарплата  "), "зарплата");
    }

    #[test]
    fn reply_keyboards_round_trip_through_outbox_storage() {
        for keyboard in [
            ReplyKeyboard::None,
            ReplyKeyboard::CurrentBalance,
            ReplyKeyboard::OperationResult,
            ReplyKeyboard::Reports,
            ReplyKeyboard::ExpenseGroups(30),
            ReplyKeyboard::ExpenseHistory(90),
        ] {
            assert_eq!(
                ReplyKeyboard::from_storage_key(&keyboard.storage_key()),
                Some(keyboard)
            );
        }
    }
}
