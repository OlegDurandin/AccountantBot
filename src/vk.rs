use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, multipart};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::domain::ReplyKeyboard;

#[derive(Debug, Clone, Deserialize)]
pub struct LongPollServer {
    pub key: String,
    pub server: String,
    pub ts: String,
}

#[derive(Debug, Deserialize)]
pub struct LongPollBatch {
    pub ts: Option<String>,
    #[serde(default)]
    pub updates: Vec<VkEvent>,
    pub failed: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub struct VkEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub event_id: String,
    pub object: Value,
}

#[derive(Debug, Deserialize)]
pub struct MessageNewObject {
    pub message: VkMessage,
}

#[derive(Debug, Deserialize)]
pub struct VkMessage {
    pub from_id: i64,
    pub peer_id: i64,
    #[serde(default)]
    pub text: String,
    pub conversation_message_id: i64,
    pub reply_message: Option<Box<ReplyMessage>>,
}

#[derive(Debug, Deserialize)]
pub struct ReplyMessage {
    pub conversation_message_id: i64,
}

#[derive(Clone)]
pub struct VkClient {
    http: Client,
    token: String,
    group_id: u64,
    api_version: String,
}

impl VkClient {
    pub fn new(token: String, group_id: u64, api_version: String) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(40))
            .user_agent("accountant-bot/0.1")
            .build()?;
        Ok(Self {
            http,
            token,
            group_id,
            api_version,
        })
    }

    pub async fn get_long_poll_server(&self) -> Result<LongPollServer> {
        self.api_call(
            "groups.getLongPollServer",
            vec![("group_id".into(), self.group_id.to_string())],
        )
        .await
    }

    pub async fn poll(&self, server: &LongPollServer, ts: &str) -> Result<LongPollBatch> {
        self.http
            .get(&server.server)
            .query(&[
                ("act", "a_check"),
                ("key", server.key.as_str()),
                ("ts", ts),
                ("wait", "25"),
            ])
            .send()
            .await
            .context("VK Long Poll request failed")?
            .error_for_status()
            .context("VK Long Poll returned an HTTP error")?
            .json::<LongPollBatch>()
            .await
            .context("invalid VK Long Poll response")
    }

    pub async fn send_message(
        &self,
        peer_id: i64,
        event_id: &str,
        message: &str,
        keyboard_kind: ReplyKeyboard,
    ) -> Result<()> {
        self.send_message_with_attachment(peer_id, event_id, message, keyboard_kind, None)
            .await
    }

    pub async fn send_budget_chart(
        &self,
        peer_id: i64,
        event_id: &str,
        png: Vec<u8>,
        keyboard_kind: ReplyKeyboard,
    ) -> Result<()> {
        let upload_server: PhotoUploadServer = self
            .api_call(
                "photos.getMessagesUploadServer",
                vec![("peer_id".into(), peer_id.to_string())],
            )
            .await?;
        let form = multipart::Form::new().part(
            "photo",
            multipart::Part::bytes(png)
                .file_name("budget-statistics.png")
                .mime_str("image/png")?,
        );
        let uploaded = self
            .http
            .post(upload_server.upload_url)
            .multipart(form)
            .send()
            .await
            .context("failed to upload budget chart to VK")?
            .error_for_status()
            .context("VK chart upload returned an HTTP error")?
            .json::<PhotoUploadResult>()
            .await
            .context("invalid VK chart upload response")?;

        let saved: Vec<SavedPhoto> = self
            .api_call(
                "photos.saveMessagesPhoto",
                vec![
                    ("server".into(), uploaded.server.to_string()),
                    ("photo".into(), uploaded.photo),
                    ("hash".into(), uploaded.hash),
                ],
            )
            .await?;
        let photo = saved
            .first()
            .context("VK returned no saved photo for the budget chart")?;
        let mut attachment = format!("photo{}_{}", photo.owner_id, photo.id);
        if let Some(access_key) = &photo.access_key {
            attachment.push('_');
            attachment.push_str(access_key);
        }
        self.send_message_with_attachment(
            peer_id,
            event_id,
            "Бюджет по месяцам",
            keyboard_kind,
            Some(&attachment),
        )
        .await
    }

    pub async fn send_excel_export(
        &self,
        peer_id: i64,
        event_id: &str,
        xlsx: Vec<u8>,
        keyboard_kind: ReplyKeyboard,
    ) -> Result<()> {
        let upload_server: DocumentUploadServer = self
            .api_call(
                "docs.getMessagesUploadServer",
                vec![
                    ("peer_id".into(), peer_id.to_string()),
                    ("type".into(), "doc".into()),
                ],
            )
            .await?;
        let form = multipart::Form::new().part(
            "file",
            multipart::Part::bytes(xlsx)
                .file_name("finance-history.xlsx")
                .mime_str("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")?,
        );
        let uploaded = self
            .http
            .post(upload_server.upload_url)
            .multipart(form)
            .send()
            .await
            .context("failed to upload Excel export to VK")?
            .error_for_status()
            .context("VK document upload returned an HTTP error")?
            .json::<DocumentUploadResult>()
            .await
            .context("invalid VK document upload response")?;

        let saved: SavedDocumentResponse = self
            .api_call(
                "docs.save",
                vec![
                    ("file".into(), uploaded.file),
                    ("title".into(), "История финансовых операций.xlsx".into()),
                ],
            )
            .await?;
        let document = saved.doc.context("VK returned no saved Excel document")?;
        let mut attachment = format!("doc{}_{}", document.owner_id, document.id);
        if let Some(access_key) = &document.access_key {
            attachment.push('_');
            attachment.push_str(access_key);
        }
        self.send_message_with_attachment(
            peer_id,
            event_id,
            "История финансовых операций в Excel",
            keyboard_kind,
            Some(&attachment),
        )
        .await
    }

    async fn send_message_with_attachment(
        &self,
        peer_id: i64,
        event_id: &str,
        message: &str,
        keyboard_kind: ReplyKeyboard,
        attachment: Option<&str>,
    ) -> Result<()> {
        let mut params = vec![
            ("peer_id".into(), peer_id.to_string()),
            ("random_id".into(), stable_random_id(event_id).to_string()),
            ("message".into(), message.into()),
        ];
        if keyboard_kind != ReplyKeyboard::None {
            params.push(("keyboard".into(), keyboard(keyboard_kind).to_string()));
        }
        if let Some(attachment) = attachment {
            params.push(("attachment".into(), attachment.into()));
        }
        let _: i64 = self.api_call("messages.send", params).await?;
        Ok(())
    }

    pub fn parse_message_event(event: &VkEvent) -> Result<MessageNewObject> {
        serde_json::from_value(event.object.clone()).context("invalid message_new event payload")
    }

    async fn api_call<T: DeserializeOwned>(
        &self,
        method: &str,
        mut params: Vec<(String, String)>,
    ) -> Result<T> {
        params.push(("access_token".into(), self.token.clone()));
        params.push(("v".into(), self.api_version.clone()));
        let url = format!("https://api.vk.com/method/{method}");
        let envelope = self
            .http
            .post(url)
            .form(&params)
            .send()
            .await
            .with_context(|| format!("VK API request {method} failed"))?
            .error_for_status()
            .with_context(|| format!("VK API method {method} returned an HTTP error"))?
            .json::<VkEnvelope<T>>()
            .await
            .with_context(|| format!("invalid response from VK API method {method}"))?;

        if let Some(error) = envelope.error {
            bail!("VK API error {}: {}", error.error_code, error.error_msg);
        }
        envelope
            .response
            .ok_or_else(|| anyhow!("VK API method {method} returned no response"))
    }
}

#[derive(Debug, Deserialize)]
struct VkEnvelope<T> {
    response: Option<T>,
    error: Option<VkApiError>,
}

#[derive(Debug, Deserialize)]
struct VkApiError {
    error_code: i64,
    error_msg: String,
}

#[derive(Debug, Deserialize)]
struct PhotoUploadServer {
    upload_url: String,
}

#[derive(Debug, Deserialize)]
struct PhotoUploadResult {
    server: i64,
    photo: String,
    hash: String,
}

#[derive(Debug, Deserialize)]
struct SavedPhoto {
    id: i64,
    owner_id: i64,
    access_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DocumentUploadServer {
    upload_url: String,
}

#[derive(Debug, Deserialize)]
struct DocumentUploadResult {
    file: String,
}

#[derive(Debug, Deserialize)]
struct SavedDocumentResponse {
    doc: Option<SavedDocument>,
}

#[derive(Debug, Deserialize)]
struct SavedDocument {
    id: i64,
    owner_id: i64,
    access_key: Option<String>,
}

fn stable_random_id(event_id: &str) -> i32 {
    let mut hasher = DefaultHasher::new();
    event_id.hash(&mut hasher);
    ((hasher.finish() % i32::MAX as u64) as i32).max(1)
}

fn keyboard(kind: ReplyKeyboard) -> Value {
    let buttons = match kind {
        ReplyKeyboard::None => Vec::new(),
        ReplyKeyboard::OperationResult => vec![
            vec![
                text_button("История расходов", "primary"),
                text_button("Расходы по группам", "secondary"),
            ],
            vec![text_button("Бюджет по месяцам", "secondary")],
        ],
        ReplyKeyboard::ExpenseGroups => vec![
            vec![
                text_button("Баланс", "primary"),
                text_button("Расходы", "secondary"),
            ],
            vec![text_button("Расходы по группам", "secondary")],
            vec![text_button("Бюджет по месяцам", "secondary")],
            vec![text_button("Выгрузить в Excel", "positive")],
        ],
        ReplyKeyboard::ExpenseHistory(active_days) => vec![
            vec![
                period_button("14 дней", active_days == 14),
                period_button("30 дней", active_days == 30),
            ],
            vec![period_button("3 месяца", active_days == 90)],
            vec![text_button("Текущий баланс", "secondary")],
            vec![text_button("Выгрузить всю историю в Excel", "positive")],
        ],
    };
    json!({"inline": true, "buttons": buttons})
}

fn text_button(label: &str, color: &str) -> Value {
    json!({"action": {"type": "text", "label": label}, "color": color})
}

fn period_button(label: &str, active: bool) -> Value {
    text_button(label, if active { "primary" } else { "secondary" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_keyboard_has_requested_layout() {
        let keyboard = keyboard(ReplyKeyboard::OperationResult);
        let rows = keyboard["buttons"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0]["action"]["label"], "История расходов");
        assert_eq!(rows[0][1]["action"]["label"], "Расходы по группам");
        assert_eq!(rows[1][0]["action"]["label"], "Бюджет по месяцам");
    }

    #[test]
    fn expense_groups_keyboard_has_requested_layout() {
        let keyboard = keyboard(ReplyKeyboard::ExpenseGroups);
        assert_eq!(keyboard["buttons"][0][0]["action"]["label"], "Баланс");
        assert_eq!(keyboard["buttons"][0][1]["action"]["label"], "Расходы");
        assert_eq!(
            keyboard["buttons"][1][0]["action"]["label"],
            "Расходы по группам"
        );
        assert_eq!(
            keyboard["buttons"][2][0]["action"]["label"],
            "Бюджет по месяцам"
        );
        assert_eq!(
            keyboard["buttons"][3][0]["action"]["label"],
            "Выгрузить в Excel"
        );
    }

    #[test]
    fn expense_history_keyboard_has_requested_layout() {
        for (active_days, active_row, active_column) in [(14, 0, 0), (30, 0, 1), (90, 1, 0)] {
            let keyboard = keyboard(ReplyKeyboard::ExpenseHistory(active_days));
            let rows = keyboard["buttons"].as_array().unwrap();
            assert_eq!(rows.len(), 4);
            assert_eq!(rows[active_row][active_column]["color"], "primary");
            assert_eq!(rows[0][0]["action"]["label"], "14 дней");
            assert_eq!(rows[0][1]["action"]["label"], "30 дней");
            assert_eq!(rows[1][0]["action"]["label"], "3 месяца");
            assert_eq!(rows[2][0]["action"]["label"], "Текущий баланс");
            assert_eq!(
                rows[3][0]["action"]["label"],
                "Выгрузить всю историю в Excel"
            );
        }
    }
}
