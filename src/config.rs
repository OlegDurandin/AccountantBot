use std::{collections::HashSet, env};

use anyhow::{Context, Result, bail};

use crate::domain::normalize_category;

#[derive(Debug, Clone)]
pub struct Config {
    pub vk_token: String,
    pub vk_group_id: u64,
    pub vk_api_version: String,
    pub allowed_expense_categories: Option<HashSet<String>>,
    pub database_url: String,
    pub analytics_interval_seconds: u64,
    pub enable_demo_data: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let vk_token = required("VK_TOKEN")?;
        if vk_token.contains("PASTE_") {
            bail!("VK_TOKEN still contains the placeholder value");
        }

        let vk_group_id = required("VK_GROUP_ID")?
            .parse()
            .context("VK_GROUP_ID must be a positive integer")?;

        let allowed_expense_categories = env::var("ALLOWED_EXPENSE_CATEGORIES")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                value
                    .split(',')
                    .map(normalize_category)
                    .filter(|value| !value.is_empty())
                    .collect::<HashSet<_>>()
            })
            .filter(|categories| !categories.is_empty());

        Ok(Self {
            vk_token,
            vk_group_id,
            vk_api_version: env::var("VK_API_VERSION").unwrap_or_else(|_| "5.199".into()),
            allowed_expense_categories,
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://data/accountant.db?mode=rwc".into()),
            analytics_interval_seconds: env::var("ANALYTICS_INTERVAL_SECONDS")
                .unwrap_or_else(|_| "300".into())
                .parse::<u64>()
                .context("ANALYTICS_INTERVAL_SECONDS must be a positive integer")?
                .max(1),
            enable_demo_data: env::var("ENABLE_DEMO_DATA")
                .is_ok_and(|value| value.eq_ignore_ascii_case("true")),
        })
    }

    pub fn category_is_allowed(&self, category: &str) -> bool {
        self.allowed_expense_categories
            .as_ref()
            .is_none_or(|categories| categories.contains(&normalize_category(category)))
    }
}

fn required(name: &str) -> Result<String> {
    env::var(name)
        .with_context(|| format!("required environment variable {name} is not configured"))
        .and_then(|value| {
            if value.trim().is_empty() {
                bail!("required environment variable {name} is empty")
            }
            Ok(value)
        })
}
