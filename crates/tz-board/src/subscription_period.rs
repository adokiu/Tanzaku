use chrono::{Datelike, DateTime, TimeZone, Utc};

pub const PLAN_RESET_MODES: &[&str] = &[
    "system",
    "month_first",
    "month_purchase",
    "never",
    "year_first",
    "year_purchase",
];

pub const SYSTEM_RESET_MODES: &[&str] = &[
    "month_first",
    "month_purchase",
    "never",
    "year_first",
    "year_purchase",
];

pub fn is_plan_reset_mode(value: &str) -> bool {
    PLAN_RESET_MODES.contains(&value)
}

pub fn is_system_reset_mode(value: &str) -> bool {
    SYSTEM_RESET_MODES.contains(&value)
}

pub fn resolve_reset_mode<'a>(stored: &'a str, system: &'a str) -> &'a str {
    if stored == "system" {
        return if is_system_reset_mode(system) {
            system
        } else {
            "month_purchase"
        };
    }
    match stored {
        "month_first" | "month_purchase" | "never" | "year_first" | "year_purchase" => stored,
        "month" | "day" | "week" | "quarter" => "month_purchase",
        "year" => "year_first",
        "lifetime" => "never",
        _ => "month_purchase",
    }
}

/// 当前订阅流量周期的起始时刻（与 `traffic_usage.period_start` 对齐）。
pub fn subscription_period_start(
    traffic_period: &str,
    starts_at: DateTime<Utc>,
    period_anchor: i16,
    at: DateTime<Utc>,
    system_reset_mode: &str,
) -> DateTime<Utc> {
    let mode = resolve_reset_mode(traffic_period, system_reset_mode);
    let anchor = i32::from(period_anchor.clamp(1, 31));
    match mode {
        "month_first" => month_period_start(at, 1),
        "month_purchase" => month_period_start(at, anchor),
        "year_first" => year_first_start(at),
        "year_purchase" => year_purchase_start(at, starts_at),
        "never" | _ => {
            let day = starts_at.date_naive().and_hms_opt(0, 0, 0).expect("midnight");
            Utc.from_utc_datetime(&day)
        }
    }
}

fn year_first_start(at: DateTime<Utc>) -> DateTime<Utc> {
    let day = chrono::NaiveDate::from_ymd_opt(at.year(), 1, 1)
        .expect("year start")
        .and_hms_opt(0, 0, 0)
        .expect("midnight");
    Utc.from_utc_datetime(&day)
}

fn year_purchase_start(at: DateTime<Utc>, starts_at: DateTime<Utc>) -> DateTime<Utc> {
    let month = starts_at.month();
    let day = starts_at.day();
    let this_year = clamp_ymd(at.year(), month, day);
    if at.date_naive() >= this_year {
        return utc_midnight(this_year);
    }
    utc_midnight(clamp_ymd(at.year() - 1, month, day))
}

fn clamp_ymd(year: i32, month: u32, day: u32) -> chrono::NaiveDate {
    let last = chrono::NaiveDate::from_ymd_opt(year, month, 1)
        .and_then(|date| date.with_month(month + 1))
        .or_else(|| chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1))
        .and_then(|next| next.pred_opt())
        .map(|date| date.day())
        .unwrap_or(28);
    chrono::NaiveDate::from_ymd_opt(year, month, day.min(last)).expect("ymd")
}

fn utc_midnight(date: chrono::NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight"))
}

fn month_period_start(at: DateTime<Utc>, anchor: i32) -> DateTime<Utc> {
    let clamp_day = |year: i32, month: u32| {
        let days = chrono::NaiveDate::from_ymd_opt(year, month, 1)
            .expect("month")
            .with_day(1)
            .and_then(|date| date.with_month(month + 1))
            .or_else(|| chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1))
            .map(|next| next.pred_opt().expect("last day").day() as i32)
            .unwrap_or(28);
        anchor.min(days).max(1)
    };
    let (year, month) = (at.year(), at.month());
    let anchor_this = clamp_day(year, month);
    if i64::from(at.day()) >= i64::from(anchor_this) {
        let day = chrono::NaiveDate::from_ymd_opt(year, month, anchor_this as u32)
            .expect("anchor day")
            .and_hms_opt(0, 0, 0)
            .expect("midnight");
        return Utc.from_utc_datetime(&day);
    }
    let (prev_year, prev_month) = if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    };
    let anchor_prev = clamp_day(prev_year, prev_month);
    let day = chrono::NaiveDate::from_ymd_opt(prev_year, prev_month, anchor_prev as u32)
        .expect("prev anchor")
        .and_hms_opt(0, 0, 0)
        .expect("midnight");
    Utc.from_utc_datetime(&day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_purchase_rolls_to_previous_month() {
        let at = Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap();
        let start = subscription_period_start("month_purchase", at, 15, at, "never");
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 9, 15, 0, 0, 0).unwrap());
    }
}
