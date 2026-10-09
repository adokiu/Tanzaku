use crate::payment::registry::{
    ChannelFactory, ChannelMeta, ConfigField, CreatePayInput, NotifyInput, NotifyOutcome, PayError,
    PayOutcome, PaymentDriver,
};
use async_trait::async_trait;
use sqlx::{Postgres, Transaction};

pub struct EpayDriver;

inventory::submit! {
    ChannelFactory {
        build: || Box::new(EpayDriver),
    }
}

const FIELDS: &[ConfigField] = &[
    ConfigField { key: "gateway_url", label: "网关地址", secret: false, required: true },
    ConfigField { key: "pid", label: "商户 ID", secret: false, required: true },
    ConfigField { key: "key", label: "商户密钥", secret: true, required: true },
    ConfigField { key: "pay_type", label: "支付方式（alipay/wxpay）", secret: false, required: false },
];

#[async_trait]
impl PaymentDriver for EpayDriver {
    fn meta(&self) -> ChannelMeta {
        ChannelMeta {
            kind: "epay",
            display_name: "易支付",
            config_fields: FIELDS,
        }
    }

    async fn create_payment(
        &self,
        _transaction: &mut Transaction<'_, Postgres>,
        _input: &CreatePayInput<'_>,
    ) -> Result<PayOutcome, PayError> {
        Err(PayError::Unavailable("易支付尚未接入"))
    }

    async fn handle_notify(&self, _input: &NotifyInput<'_>) -> Result<NotifyOutcome, PayError> {
        Err(PayError::Unavailable("易支付尚未接入"))
    }
}
