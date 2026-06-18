use chrono::{DateTime, FixedOffset, NaiveDate};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TossInvestApiError {
    pub request_id: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OAuth2TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OAuth2ErrorResponse {
    pub error: String,
    #[serde(rename = "error_description")]
    pub error_description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ApiEnvelope<T> {
    pub result: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TossErrorEnvelope {
    pub error: TossErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TossErrorBody {
    #[serde(rename = "requestId")]
    pub request_id: Option<String>,
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl From<TossErrorBody> for TossInvestApiError {
    fn from(value: TossErrorBody) -> Self {
        Self {
            request_id: value.request_id,
            code: Some(value.code),
            message: Some(value.message),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TossPriceRaw {
    pub symbol: String,
    #[serde(rename = "lastPrice")]
    pub last_price: String,
    pub currency: String,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TossStockRaw {
    pub symbol: String,
    pub name: Option<String>,
    #[serde(rename = "shortName")]
    pub short_name: Option<String>,
    #[serde(rename = "longName")]
    pub long_name: Option<String>,
    pub currency: Option<String>,
    #[serde(rename = "quoteType")]
    pub quote_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TossExchangeRateRaw {
    #[serde(rename = "midRate")]
    pub mid_rate: String,
    #[serde(rename = "validFrom")]
    pub valid_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TossMarketCalendarRaw {
    pub today: TossMarketDayRaw,
    #[serde(rename = "previousBusinessDay")]
    pub previous_business_day: TossMarketDayRaw,
    #[serde(rename = "nextBusinessDay")]
    pub next_business_day: TossMarketDayRaw,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TossMarketDayRaw {
    pub date: NaiveDate,
    #[serde(rename = "dayMarket")]
    pub day_market: Option<TossMarketSessionRaw>,
    #[serde(rename = "preMarket")]
    pub pre_market: Option<TossMarketSessionRaw>,
    #[serde(rename = "regularMarket")]
    pub regular_market: Option<TossMarketSessionRaw>,
    #[serde(rename = "afterMarket")]
    pub after_market: Option<TossMarketSessionRaw>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TossMarketSessionRaw {
    #[serde(rename = "startTime")]
    pub start_time: DateTime<FixedOffset>,
    #[serde(rename = "endTime")]
    pub end_time: DateTime<FixedOffset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TossCandleRaw {
    pub symbol: Option<String>,
    #[serde(rename = "closePrice")]
    pub close_price: String,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CandlePageResponse {
    pub candles: Vec<TossCandleRaw>,
}

#[cfg(test)]
mod tests {
    use super::{
        ApiEnvelope, CandlePageResponse, OAuth2ErrorResponse, OAuth2TokenResponse,
        TossErrorEnvelope, TossExchangeRateRaw, TossInvestApiError, TossMarketCalendarRaw,
        TossPriceRaw, TossStockRaw,
    };

    #[test]
    fn oauth_token_response_deserializes() {
        let response = serde_json::from_str::<OAuth2TokenResponse>(
            r#"{
                "access_token": "token-value",
                "token_type": "Bearer",
                "expires_in": 3600
            }"#,
        )
        .unwrap();

        assert_eq!(response.access_token, "token-value");
        assert_eq!(response.token_type, "Bearer");
        assert_eq!(response.expires_in, 3600);
    }

    #[test]
    fn oauth_error_response_deserializes() {
        let response = serde_json::from_str::<OAuth2ErrorResponse>(
            r#"{
                "error": "invalid_client",
                "error_description": "Client authentication failed."
            }"#,
        )
        .unwrap();

        assert_eq!(response.error, "invalid_client");
        assert_eq!(
            response.error_description.as_deref(),
            Some("Client authentication failed.")
        );
    }

    #[test]
    fn oauth_error_response_allows_missing_description() {
        let response = serde_json::from_str::<OAuth2ErrorResponse>(
            r#"{
                "error": "invalid_client"
            }"#,
        )
        .unwrap();

        assert_eq!(response.error, "invalid_client");
        assert_eq!(response.error_description, None);
    }

    #[test]
    fn error_envelope_extracts_request_id_code_and_message() {
        let envelope = serde_json::from_str::<TossErrorEnvelope>(
            r#"{
                "error": {
                    "requestId": "req-123",
                    "code": "too-many-requests",
                    "message": "slow down"
                }
            }"#,
        )
        .unwrap();

        let error = TossInvestApiError::from(envelope.error);
        assert_eq!(error.request_id.as_deref(), Some("req-123"));
        assert_eq!(error.code.as_deref(), Some("too-many-requests"));
        assert_eq!(error.message.as_deref(), Some("slow down"));
    }

    #[test]
    fn error_envelope_defaults_data_to_null() {
        let envelope = serde_json::from_str::<TossErrorEnvelope>(
            r#"{
                "error": {
                    "requestId": "req-123",
                    "code": "bad-request",
                    "message": "invalid input"
                }
            }"#,
        )
        .unwrap();

        assert!(envelope.error.data.is_null());
    }

    #[test]
    fn raw_decimal_money_fields_remain_strings() {
        let prices = serde_json::from_str::<ApiEnvelope<Vec<TossPriceRaw>>>(
            r#"{
                "result": [
                    {
                        "symbol": "SOXL",
                        "lastPrice": "23.4500",
                        "currency": "USD",
                        "timestamp": "2026-06-18T01:23:45Z"
                    }
                ]
            }"#,
        )
        .unwrap();
        let exchange = serde_json::from_str::<ApiEnvelope<TossExchangeRateRaw>>(
            r#"{
                "result": {
                    "midRate": "1378.2500",
                    "validFrom": "2026-06-18T00:00:00+09:00"
                }
            }"#,
        )
        .unwrap();
        let candles = serde_json::from_str::<ApiEnvelope<CandlePageResponse>>(
            r#"{
                "result": {
                    "candles": [
                        {
                            "symbol": "SOXL",
                            "closePrice": "22.9800",
                            "timestamp": "2026-06-17T20:00:00-04:00"
                        }
                    ]
                }
            }"#,
        )
        .unwrap();
        let stocks = serde_json::from_str::<ApiEnvelope<Vec<TossStockRaw>>>(
            r#"{
                "result": [
                    {
                        "symbol": "SOXL",
                        "name": "SOXL name",
                        "shortName": "SOXL short",
                        "longName": "SOXL long",
                        "currency": "USD",
                        "quoteType": "ETF"
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(prices.result[0].last_price, "23.4500");
        assert_eq!(exchange.result.mid_rate, "1378.2500");
        assert_eq!(candles.result.candles[0].close_price, "22.9800");
        assert_eq!(stocks.result[0].quote_type.as_deref(), Some("ETF"));
    }

    #[test]
    fn market_calendar_response_deserializes_kst_sessions() {
        let calendar = serde_json::from_str::<ApiEnvelope<TossMarketCalendarRaw>>(
            r#"{
                "result": {
                    "today": {
                        "date": "2026-06-18",
                        "dayMarket": {
                            "startTime": "2026-06-18T10:00:00+09:00",
                            "endTime": "2026-06-18T16:00:00+09:00"
                        },
                        "preMarket": {
                            "startTime": "2026-06-18T17:00:00+09:00",
                            "endTime": "2026-06-18T22:30:00+09:00"
                        },
                        "regularMarket": {
                            "startTime": "2026-06-18T22:30:00+09:00",
                            "endTime": "2026-06-19T05:00:00+09:00"
                        },
                        "afterMarket": {
                            "startTime": "2026-06-19T05:00:00+09:00",
                            "endTime": "2026-06-19T09:00:00+09:00"
                        }
                    },
                    "previousBusinessDay": {
                        "date": "2026-06-17",
                        "dayMarket": null,
                        "preMarket": null,
                        "regularMarket": null,
                        "afterMarket": null
                    },
                    "nextBusinessDay": {
                        "date": "2026-06-19",
                        "dayMarket": null,
                        "preMarket": null,
                        "regularMarket": null,
                        "afterMarket": null
                    }
                }
            }"#,
        )
        .unwrap()
        .result;

        assert_eq!(calendar.today.date.to_string(), "2026-06-18");
        assert_eq!(
            calendar.today.regular_market.unwrap().end_time.to_string(),
            "2026-06-19 05:00:00 +09:00"
        );
    }

    #[test]
    fn market_calendar_allows_holiday_nullable_sessions() {
        let calendar = serde_json::from_str::<ApiEnvelope<TossMarketCalendarRaw>>(
            r#"{
                "result": {
                    "today": {
                        "date": "2026-07-03",
                        "dayMarket": null,
                        "preMarket": null,
                        "regularMarket": null,
                        "afterMarket": null
                    },
                    "previousBusinessDay": {
                        "date": "2026-07-02",
                        "dayMarket": null,
                        "preMarket": null,
                        "regularMarket": null,
                        "afterMarket": null
                    },
                    "nextBusinessDay": {
                        "date": "2026-07-06",
                        "dayMarket": null,
                        "preMarket": null,
                        "regularMarket": null,
                        "afterMarket": null
                    }
                }
            }"#,
        )
        .unwrap()
        .result;

        assert!(calendar.today.day_market.is_none());
        assert!(calendar.today.pre_market.is_none());
        assert!(calendar.today.regular_market.is_none());
        assert!(calendar.today.after_market.is_none());
    }
}
