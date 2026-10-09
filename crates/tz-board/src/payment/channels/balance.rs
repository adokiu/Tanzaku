use crate::payment::registry::{
    ChannelFactory, ChannelMeta, CreatePayInput, NotifyInput, NotifyOutcome, PayError, PayOutcome,
    PaymentDriver,
};
use async_trait::async_trait;
use sqlx::{Postgres, Transaction};

pub struct BalanceDriver;

inventory::submit! {
    ChannelFactory {
        build: || Box::new(BalanceDriver),
    }
}

#[async_trait]
impl PaymentDriver for BalanceDriver {
    fn meta(&self) -> ChannelMeta {
        ChannelMeta {
            kind: "balance",
            display_name: "余额支付",
            config_fields: &[],
        }
    }

    async fn create_payment(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        input: &CreatePayInput<'_>,
    ) -> Result<PayOutcome, PayError> {
        let balance: i64 = sqlx::query_scalar("SELECT balance_cents FROM users WHERE id = $1 FOR UPDATE")
            .bind(input.user_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(|_| PayError::Unavailable("数据库暂不可用"))?
            .ok_or(PayError::Unavailable("用户不存在"))?;
        if balance < input.amount_cents {
            return Err(PayError::Message("账户余额不足"));
        }
        let updated = sqlx::query(
            "UPDATE users SET balance_cents = balance_cents - $2, updated_at = now() WHERE id = $1 AND balance_cents >= $2",
        )
        .bind(input.user_id)
        .bind(input.amount_cents)
        .execute(&mut **transaction)
        .await
        .map_err(|_| PayError::Unavailable("数据库暂不可用"))?;
        if updated.rows_affected() != 1 {
            return Err(PayError::Message("账户余额不足"));
        }
        Ok(PayOutcome::Completed)
    }

    async fn handle_notify(&self, _input: &NotifyInput<'_>) -> Result<NotifyOutcome, PayError> {
        Ok(NotifyOutcome::Ignored)
    }
}
