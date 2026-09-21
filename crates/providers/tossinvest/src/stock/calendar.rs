use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use dynamo_service_stock::Error;
use tokio::sync::Mutex;

use crate::{TossInvestMarketCalendarService, TossMarketSessionPhase};

use super::{MARKET_CALENDAR_TTL, REGULAR_CLOSE_SETTLE_GRACE, types::BaselineCloseTarget};
#[derive(Clone, Default)]
pub(super) struct MarketCalendarCache {
    entry: Arc<Mutex<Option<CachedMarketCalendar>>>,
}

impl MarketCalendarCache {
    pub(super) async fn fetch_at(
        &self,
        now: DateTime<Utc>,
        service: &TossInvestMarketCalendarService,
    ) -> Result<crate::models::TossMarketCalendarRaw, Error> {
        {
            let entry = self.entry.lock().await;
            if let Some(cached) = entry.as_ref().filter(|cached| cached.expires_at > now) {
                return Ok(cached.calendar.clone());
            }
        }

        let calendar = service.fetch_today().await?;
        let expires_at = now.checked_add_signed(MARKET_CALENDAR_TTL).unwrap_or(now);
        let mut entry = self.entry.lock().await;
        *entry = Some(CachedMarketCalendar {
            calendar: calendar.clone(),
            expires_at,
        });
        Ok(calendar)
    }
}

#[derive(Clone)]
pub(super) struct CachedMarketCalendar {
    calendar: crate::models::TossMarketCalendarRaw,
    expires_at: DateTime<Utc>,
}

pub(super) fn baseline_close_target_for_phase(
    calendar: &crate::models::TossMarketCalendarRaw,
    phase: TossMarketSessionPhase,
    at: DateTime<Utc>,
) -> BaselineCloseTarget {
    match phase {
        TossMarketSessionPhase::AfterMarket => {
            let date = active_session_day_date(calendar, phase, at)
                .or_else(|| most_recent_regular_close_date(calendar, at))
                .unwrap_or(calendar.previous_business_day.date);
            settled_regular_close_target(calendar, date, at)
        }
        TossMarketSessionPhase::Closed | TossMarketSessionPhase::Unknown => {
            let date = most_recent_regular_close_date(calendar, at)
                .unwrap_or(calendar.previous_business_day.date);
            settled_regular_close_target(calendar, date, at)
        }
        TossMarketSessionPhase::DayMarket
        | TossMarketSessionPhase::PreMarket
        | TossMarketSessionPhase::RegularMarket => BaselineCloseTarget::Before(
            active_session_day_date(calendar, phase, at).unwrap_or(calendar.today.date),
        ),
    }
}

pub(super) fn settled_regular_close_target(
    calendar: &crate::models::TossMarketCalendarRaw,
    date: NaiveDate,
    at: DateTime<Utc>,
) -> BaselineCloseTarget {
    if regular_close_is_settled(calendar, date, at) {
        BaselineCloseTarget::Exact(date)
    } else {
        BaselineCloseTarget::Before(date)
    }
}

pub(super) fn active_session_day_date(
    calendar: &crate::models::TossMarketCalendarRaw,
    phase: TossMarketSessionPhase,
    at: DateTime<Utc>,
) -> Option<NaiveDate> {
    [
        &calendar.previous_business_day,
        &calendar.today,
        &calendar.next_business_day,
    ]
    .into_iter()
    .find(|day| {
        day_session_for_phase(day, phase).is_some_and(|session| session_contains(session, at))
    })
    .map(|day| day.date)
}

pub(super) fn regular_close_is_settled(
    calendar: &crate::models::TossMarketCalendarRaw,
    date: NaiveDate,
    at: DateTime<Utc>,
) -> bool {
    [
        &calendar.previous_business_day,
        &calendar.today,
        &calendar.next_business_day,
    ]
    .into_iter()
    .find(|day| day.date == date)
    .and_then(|day| day.regular_market.as_ref())
    .and_then(|session| {
        session
            .end_time
            .to_utc()
            .checked_add_signed(REGULAR_CLOSE_SETTLE_GRACE)
    })
    .is_none_or(|settled_at| settled_at <= at)
}

pub(super) fn day_session_for_phase(
    day: &crate::models::TossMarketDayRaw,
    phase: TossMarketSessionPhase,
) -> Option<&crate::models::TossMarketSessionRaw> {
    match phase {
        TossMarketSessionPhase::DayMarket => day.day_market.as_ref(),
        TossMarketSessionPhase::PreMarket => day.pre_market.as_ref(),
        TossMarketSessionPhase::RegularMarket => day.regular_market.as_ref(),
        TossMarketSessionPhase::AfterMarket => day.after_market.as_ref(),
        TossMarketSessionPhase::Closed | TossMarketSessionPhase::Unknown => None,
    }
}

pub(super) fn session_contains(
    session: &crate::models::TossMarketSessionRaw,
    at: DateTime<Utc>,
) -> bool {
    let start = session.start_time.to_utc();
    let end = session.end_time.to_utc();
    start < end && start <= at && at < end
}

pub(super) fn most_recent_regular_close_date(
    calendar: &crate::models::TossMarketCalendarRaw,
    at: DateTime<Utc>,
) -> Option<NaiveDate> {
    [
        &calendar.previous_business_day,
        &calendar.today,
        &calendar.next_business_day,
    ]
    .into_iter()
    .filter_map(|day| {
        let regular_end = day.regular_market.as_ref()?.end_time.to_utc();
        (regular_end <= at).then_some((regular_end, day.date))
    })
    .max_by_key(|(regular_end, _)| *regular_end)
    .map(|(_, date)| date)
}
