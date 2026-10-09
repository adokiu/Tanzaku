use chrono::{DateTime, Months, Utc};
use uuid::Uuid;

/// 对外订单号：含 UTC 日期时间 + 随机后缀，唯一。例 `TZ20261009162844a1b2c3d4`
pub fn new_order_no(now: DateTime<Utc>) -> String {
    let stamp = now.format("%Y%m%d%H%M%S");
    let hex = Uuid::new_v4().to_string().replace('-', "");
    format!("TZ{stamp}{}", &hex[..8])
}

pub const BILLING_PERIODS: &[&str] = &[
    "month",
    "quarter",
    "half_year",
    "year",
    "two_year",
    "three_year",
    "traffic_pack",
    "reset_pack",
];

pub const ORDER_PERIODS: &[&str] = &[
    "day",
    "week",
    "month",
    "quarter",
    "half_year",
    "year",
    "two_year",
    "three_year",
    "lifetime",
    "traffic_pack",
    "reset_pack",
];

pub const ORDER_STATUSES: &[&str] = &[
    "pending",
    "paying",
    "completed",
    "cancelled",
    "failed",
    "credited",
];

#[derive(Debug, Clone, Copy)]
pub struct PlanPrices {
    pub price_month_cents: Option<i64>,
    pub price_quarter_cents: Option<i64>,
    pub price_half_year_cents: Option<i64>,
    pub price_year_cents: Option<i64>,
    pub price_two_year_cents: Option<i64>,
    pub price_three_year_cents: Option<i64>,
    pub price_traffic_pack_cents: Option<i64>,
    pub price_reset_pack_cents: Option<i64>,
}

impl PlanPrices {
    pub fn amount_for(&self, period: &str) -> Option<i64> {
        match period {
            "month" => self.price_month_cents,
            "quarter" => self.price_quarter_cents,
            "half_year" => self.price_half_year_cents,
            "year" => self.price_year_cents,
            "two_year" => self.price_two_year_cents,
            "three_year" => self.price_three_year_cents,
            "traffic_pack" => self.price_traffic_pack_cents,
            "reset_pack" => self.price_reset_pack_cents,
            _ => None,
        }
    }
}

pub fn is_addon(period: &str) -> bool {
    matches!(period, "traffic_pack" | "reset_pack")
}

pub fn extend_expiry(from: DateTime<Utc>, now: DateTime<Utc>, period: &str) -> Option<DateTime<Utc>> {
    let months = match period {
        "month" => 1,
        "quarter" => 3,
        "half_year" => 6,
        "year" => 12,
        "two_year" => 24,
        "three_year" => 36,
        _ => return None,
    };
    let start = if from > now { from } else { now };
    start.checked_add_months(Months::new(months))
}
