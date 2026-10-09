use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ConfigField {
    pub key: &'static str,
    pub label: &'static str,
    pub secret: bool,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ChannelMeta {
    pub kind: &'static str,
    pub display_name: &'static str,
    pub config_fields: &'static [ConfigField],
}

pub struct CreatePayInput<'a> {
    pub order_id: Uuid,
    pub user_id: Uuid,
    pub amount_cents: i64,
    pub subject: &'a str,
    pub channel_config: &'a Value,
    pub notify_url: &'a str,
    pub return_url: &'a str,
}

pub struct NotifyInput<'a> {
    pub channel_config: &'a Value,
    pub query: &'a [(String, String)],
    pub body: &'a str,
}

#[derive(Debug)]
pub enum PayOutcome {
    Completed,
    Pending { pay_url: Option<String> },
}

#[derive(Debug)]
pub enum NotifyOutcome {
    Paid { order_id: Uuid },
    Ignored,
}

#[derive(Debug)]
pub enum PayError {
    Unavailable(&'static str),
    Message(&'static str),
}

#[async_trait]
pub trait PaymentDriver: Send + Sync {
    fn meta(&self) -> ChannelMeta;
    async fn create_payment(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        input: &CreatePayInput<'_>,
    ) -> Result<PayOutcome, PayError>;
    async fn handle_notify(&self, input: &NotifyInput<'_>) -> Result<NotifyOutcome, PayError>;
}

pub struct ChannelFactory {
    pub build: fn() -> Box<dyn PaymentDriver>,
}

inventory::collect!(ChannelFactory);

pub fn find(kind: &str) -> Option<Box<dyn PaymentDriver>> {
    registered_channels()
        .into_iter()
        .find(|driver| driver.meta().kind == kind)
}

pub fn registered_channels() -> Vec<Box<dyn PaymentDriver>> {
    let mut items: Vec<_> = inventory::iter::<ChannelFactory>()
        .map(|factory| (factory.build)())
        .collect();
    items.sort_by_key(|item| item.meta().kind);
    items
}

pub fn validate_config(kind: &str, config: &Value) -> Result<(), PayError> {
    let driver = find(kind).ok_or(PayError::Unavailable("未知支付渠道类型"))?;
    let object = config.as_object();
    for field in driver.meta().config_fields {
        if !field.required {
            continue;
        }
        let Some(object) = object else {
            return Err(PayError::Message("支付渠道配置无效"));
        };
        let value = object.get(field.key).and_then(Value::as_str).unwrap_or("").trim();
        if value.is_empty() {
            return Err(PayError::Message("支付渠道配置不完整"));
        }
    }
    Ok(())
}
