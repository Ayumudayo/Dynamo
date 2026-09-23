use reqwest::StatusCode;

use super::TossInvestResponse;
#[test]
fn request_error_classifies_maintenance_from_api_code_not_localized_message() {
    let response = TossInvestResponse::test_json(
        StatusCode::INTERNAL_SERVER_ERROR,
        r#"{
                "error": {
                    "requestId": "request-123",
                    "code": " MAINTENANCE ",
                    "message": "점검 중입니다. 잠시 후 다시 시도해 주세요."
                }
            }"#,
    );

    let error = response.request_error("exchange-rate");

    assert!(error.is_maintenance());
    assert_eq!(error.endpoint(), "exchange-rate");
    assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error.code(), Some(" MAINTENANCE "));
    assert!(error.to_string().contains("request-123"));
    assert!(error.to_string().contains("점검 중입니다"));
}

#[test]
fn request_error_retains_non_maintenance_api_context() {
    let response = TossInvestResponse::test_json(
        StatusCode::BAD_REQUEST,
        r#"{
                "error": {
                    "requestId": "request-456",
                    "code": "invalid-request",
                    "message": "invalid currency"
                }
            }"#,
    );

    let error = response.request_error("exchange-rate");

    assert!(!error.is_maintenance());
    assert_eq!(error.code(), Some("invalid-request"));
    assert_eq!(
        error.to_string(),
        "Toss Invest exchange-rate request failed with status 400 Bad Request (request_id: request-456, code: invalid-request, message: invalid currency)"
    );
}
