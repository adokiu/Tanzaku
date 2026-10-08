use chrono::{Datelike, DateTime, Duration, TimeZone, Utc};

/// 当前订阅计费周期的起始时刻（与 `traffic_usage.period_start` 对齐）。
pub fn subscription_period_start(
    traffic_period: &str,
    starts_at: DateTime<Utc>,
    period_anchor: i16,
    at: DateTime<Utc>,
) -> DateTime<Utc> {
    let anchor = i32::from(period_anchor.clamp(1, 31));
    match traffic_period {
        "day" => {
            let day = at.date_naive().and_hms_opt(0, 0, 0).expect("midnight");
            Utc.from_utc_datetime(&day)
        }
        "week" => {
            let weekday = at.weekday().num_days_from_monday();
            let day = at.date_naive() - Duration::days(i64::from(weekday));
            let day = day.and_hms_opt(0, 0, 0).expect("midnight");
            Utc.from_utc_datetime(&day)
        }
        "month" => month_period_start(at, anchor),
        "quarter" => {
            let month = ((at.month0() / 3) * 3) + 1;
            let day = chrono::NaiveDate::from_ymd_opt(at.year(), month, 1)
                .expect("quarter month")
                .and_hms_opt(0, 0, 0)
                .expect("midnight");
            Utc.from_utc_datetime(&day)
        }
        "year" => {
            let day = chrono::NaiveDate::from_ymd_opt(at.year(), 1, 1)
                .expect("year start")
                .and_hms_opt(0, 0, 0)
                .expect("midnight");
            Utc.from_utc_datetime(&day)
        }
        "lifetime" | _ => {
            let day = starts_at.date_naive().and_hms_opt(0, 0, 0).expect("midnight");
            Utc.from_utc_datetime(&day)
        }
    }
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
    fn month_anchor_rolls_to_previous_month() {
        let at = Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap();
        let start = subscription_period_start("month", at, 15, at);
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 9, 15, 0, 0, 0).unwrap());
    }
}
