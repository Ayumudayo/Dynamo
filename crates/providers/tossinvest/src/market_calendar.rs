use std::fmt;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, NaiveDate, TimeDelta, TimeZone, Utc};
use reqwest::Method;

use crate::{
    TossInvestClient, TossInvestResponse, TossRateLimitGroup,
    models::{ApiEnvelope, TossErrorEnvelope, TossMarketCalendarRaw, TossMarketDayRaw},
};

const MARKET_CALENDAR_PATH: &str = "/api/v1/market-calendar/US";
const MARKET_CALENDAR_GROUP: TossRateLimitGroup = TossRateLimitGroup::MarketInfo;
const KST_OFFSET_SECONDS: i32 = 9 * 60 * 60;

#[derive(Clone, Debug)]
pub struct TossInvestMarketCalendarService {
    client: TossInvestClient,
}

impl TossInvestMarketCalendarService {
    pub fn new(client: TossInvestClient) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &TossInvestClient {
        &self.client
    }

    pub async fn fetch_today(&self) -> Result<TossMarketCalendarRaw> {
        fetch_market_calendar(&self.client, None).await
    }

    pub async fn fetch_for_date(&self, date: NaiveDate) -> Result<TossMarketCalendarRaw> {
        fetch_market_calendar(&self.client, Some(date)).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TossMarketSessionPhase {
    DayMarket,
    PreMarket,
    RegularMarket,
    AfterMarket,
    Closed,
    Unknown,
}

impl TossMarketSessionPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DayMarket => "Day Market",
            Self::PreMarket => "Pre Market",
            Self::RegularMarket => "Regular Market",
            Self::AfterMarket => "After Market",
            Self::Closed => "Closed",
            Self::Unknown => "Unknown",
        }
    }
}

impl fmt::Display for TossMarketSessionPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TossMarketCalendarRaw {
    pub fn classify_at(&self, at: DateTime<Utc>) -> TossMarketSessionPhase {
        for day in self.days() {
            if let Some(phase) = classify_day_session(day, at) {
                return phase;
            }
        }

        if self.contains_known_window(at) {
            TossMarketSessionPhase::Closed
        } else {
            TossMarketSessionPhase::Unknown
        }
    }

    fn days(&self) -> [&TossMarketDayRaw; 3] {
        [
            &self.previous_business_day,
            &self.today,
            &self.next_business_day,
        ]
    }

    fn contains_known_window(&self, at: DateTime<Utc>) -> bool {
        let starts = self.days().into_iter().filter_map(known_window_start);
        let ends = self.days().into_iter().filter_map(known_window_end);

        match (starts.min(), ends.max()) {
            (Some(start), Some(end)) => start <= at && at < end,
            _ => false,
        }
    }
}

async fn fetch_market_calendar(
    client: &TossInvestClient,
    date: Option<NaiveDate>,
) -> Result<TossMarketCalendarRaw> {
    let path = market_calendar_path(date);
    let response = client
        .send_authenticated(MARKET_CALENDAR_GROUP, Method::GET, &path)
        .await?;
    build_market_calendar(response)
}

fn market_calendar_path(date: Option<NaiveDate>) -> String {
    match date {
        Some(date) => format!("{MARKET_CALENDAR_PATH}?date={date}"),
        None => MARKET_CALENDAR_PATH.to_string(),
    }
}

fn build_market_calendar(response: TossInvestResponse) -> Result<TossMarketCalendarRaw> {
    if !response.status().is_success() {
        return Err(build_market_calendar_request_error(&response));
    }

    response
        .json::<ApiEnvelope<TossMarketCalendarRaw>>()
        .map(|payload| payload.result)
        .context("failed to deserialize Toss Invest market-calendar response")
}

fn build_market_calendar_request_error(response: &TossInvestResponse) -> anyhow::Error {
    if let Ok(error) = response.json::<TossErrorEnvelope>() {
        return anyhow!(
            "Toss Invest market-calendar request failed with status {} (request_id: {}, code: {}, message: {})",
            response.status(),
            error.error.request_id.as_deref().unwrap_or("unknown"),
            error.error.code,
            error.error.message,
        );
    }

    anyhow!(
        "Toss Invest market-calendar request failed with status {}",
        response.status()
    )
}

fn classify_day_session(
    day: &TossMarketDayRaw,
    at: DateTime<Utc>,
) -> Option<TossMarketSessionPhase> {
    let sessions = [
        (TossMarketSessionPhase::DayMarket, day.day_market.as_ref()),
        (TossMarketSessionPhase::PreMarket, day.pre_market.as_ref()),
        (
            TossMarketSessionPhase::RegularMarket,
            day.regular_market.as_ref(),
        ),
        (
            TossMarketSessionPhase::AfterMarket,
            day.after_market.as_ref(),
        ),
    ];

    sessions
        .into_iter()
        .find(|(_, session)| {
            session
                .map(|session| {
                    let start = session.start_time.to_utc();
                    let end = session.end_time.to_utc();
                    start < end && start <= at && at < end
                })
                .unwrap_or(false)
        })
        .map(|(phase, _)| phase)
}

fn known_window_start(day: &TossMarketDayRaw) -> Option<DateTime<Utc>> {
    let date_start = kst_midnight(day.date)?;
    day_sessions(day)
        .filter_map(|session| session.map(|session| session.start_time.to_utc()))
        .min()
        .map(|session_start| date_start.min(session_start))
        .or(Some(date_start))
}

fn known_window_end(day: &TossMarketDayRaw) -> Option<DateTime<Utc>> {
    let date_end = kst_midnight(day.date)?.checked_add_signed(TimeDelta::days(2))?;
    day_sessions(day)
        .filter_map(|session| session.map(|session| session.end_time.to_utc()))
        .max()
        .map(|session_end| date_end.max(session_end))
        .or(Some(date_end))
}

fn day_sessions(
    day: &TossMarketDayRaw,
) -> impl Iterator<Item = Option<&crate::models::TossMarketSessionRaw>> {
    [
        day.day_market.as_ref(),
        day.pre_market.as_ref(),
        day.regular_market.as_ref(),
        day.after_market.as_ref(),
    ]
    .into_iter()
}

fn kst_midnight(date: NaiveDate) -> Option<DateTime<Utc>> {
    let offset = chrono::FixedOffset::east_opt(KST_OFFSET_SECONDS)?;
    let midnight = date.and_hms_opt(0, 0, 0)?;
    offset
        .from_local_datetime(&midnight)
        .single()
        .map(|value| value.to_utc())
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, NaiveDate, TimeZone, Utc};

    use crate::{
        TossRateLimitGroup,
        models::{TossMarketCalendarRaw, TossMarketDayRaw, TossMarketSessionRaw},
    };

    use super::{
        MARKET_CALENDAR_GROUP, MARKET_CALENDAR_PATH, TossMarketSessionPhase, market_calendar_path,
    };

    fn session(start_time: &str, end_time: &str) -> TossMarketSessionRaw {
        TossMarketSessionRaw {
            start_time: DateTime::parse_from_rfc3339(start_time).unwrap(),
            end_time: DateTime::parse_from_rfc3339(end_time).unwrap(),
        }
    }

    fn day(date: &str) -> TossMarketDayRaw {
        TossMarketDayRaw {
            date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            day_market: Some(session(
                &format!("{date}T10:00:00+09:00"),
                &format!("{date}T16:00:00+09:00"),
            )),
            pre_market: Some(session(
                &format!("{date}T17:00:00+09:00"),
                &format!("{date}T22:30:00+09:00"),
            )),
            regular_market: Some(session(
                &format!("{date}T22:30:00+09:00"),
                "2026-06-19T05:00:00+09:00",
            )),
            after_market: Some(session(
                "2026-06-19T05:00:00+09:00",
                "2026-06-19T09:00:00+09:00",
            )),
        }
    }

    fn calendar() -> TossMarketCalendarRaw {
        TossMarketCalendarRaw {
            previous_business_day: TossMarketDayRaw {
                date: NaiveDate::from_ymd_opt(2026, 6, 17).unwrap(),
                day_market: None,
                pre_market: None,
                regular_market: None,
                after_market: None,
            },
            today: day("2026-06-18"),
            next_business_day: TossMarketDayRaw {
                date: NaiveDate::from_ymd_opt(2026, 6, 19).unwrap(),
                day_market: None,
                pre_market: None,
                regular_market: None,
                after_market: None,
            },
        }
    }

    #[test]
    fn market_calendar_endpoint_path_and_rate_limit_are_market_info() {
        let date = NaiveDate::from_ymd_opt(2026, 6, 18).unwrap();

        assert_eq!(MARKET_CALENDAR_PATH, "/api/v1/market-calendar/US");
        assert_eq!(MARKET_CALENDAR_GROUP, TossRateLimitGroup::MarketInfo);
        assert_eq!(market_calendar_path(None), "/api/v1/market-calendar/US");
        assert_eq!(
            market_calendar_path(Some(date)),
            "/api/v1/market-calendar/US?date=2026-06-18"
        );
    }

    #[test]
    fn market_calendar_classification_uses_half_open_boundaries() {
        let calendar = calendar();

        assert_eq!(
            calendar.classify_at(
                DateTime::parse_from_rfc3339("2026-06-18T10:00:00+09:00")
                    .unwrap()
                    .to_utc()
            ),
            TossMarketSessionPhase::DayMarket
        );
        assert_eq!(
            calendar.classify_at(
                DateTime::parse_from_rfc3339("2026-06-18T16:00:00+09:00")
                    .unwrap()
                    .to_utc()
            ),
            TossMarketSessionPhase::Closed
        );
        assert_eq!(
            calendar.classify_at(
                DateTime::parse_from_rfc3339("2026-06-18T17:00:00+09:00")
                    .unwrap()
                    .to_utc()
            ),
            TossMarketSessionPhase::PreMarket
        );
        assert_eq!(
            calendar.classify_at(
                DateTime::parse_from_rfc3339("2026-06-18T22:30:00+09:00")
                    .unwrap()
                    .to_utc()
            ),
            TossMarketSessionPhase::RegularMarket
        );
        assert_eq!(
            calendar.classify_at(
                DateTime::parse_from_rfc3339("2026-06-19T05:00:00+09:00")
                    .unwrap()
                    .to_utc()
            ),
            TossMarketSessionPhase::AfterMarket
        );
        assert_eq!(
            calendar.classify_at(
                DateTime::parse_from_rfc3339("2026-06-19T09:00:00+09:00")
                    .unwrap()
                    .to_utc()
            ),
            TossMarketSessionPhase::Closed
        );
    }

    #[test]
    fn market_calendar_classification_handles_cross_midnight_kst_sessions() {
        let calendar = calendar();

        assert_eq!(
            calendar.classify_at(Utc.with_ymd_and_hms(2026, 6, 18, 18, 0, 0).unwrap()),
            TossMarketSessionPhase::RegularMarket
        );
    }

    #[test]
    fn market_calendar_classification_returns_unknown_outside_calendar_window() {
        let calendar = calendar();

        assert_eq!(
            calendar.classify_at(Utc.with_ymd_and_hms(2026, 6, 25, 0, 0, 0).unwrap()),
            TossMarketSessionPhase::Unknown
        );
    }
}
