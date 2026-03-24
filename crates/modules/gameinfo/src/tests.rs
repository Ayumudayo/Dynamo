use crate::{
    maintenance::{
        classify_maintenance_notice, extract_maintenance_summary_ja, format_maintenance_title,
        parse_maintenance_schedule, trim_maintenance_intro, MaintenanceNoticeKind,
    },
    pll::{extract_pll_start, generate_pll_title, is_valid_pll_schedule, PllInfo},
};

#[test]
fn formats_same_day_maintenance_window() {
    assert_eq!(
        format_maintenance_title(1_750_104_000, 1_750_154_400, MaintenanceNoticeKind::Regular,),
        "전 월드 유지보수 작업 (6/17)"
    );
}

#[test]
fn formats_emergency_maintenance_window() {
    assert_eq!(
        format_maintenance_title(1_750_104_000, 1_750_154_400, MaintenanceNoticeKind::Emergency,),
        "전 월드 긴급 유지보수 작업 (6/17)"
    );
}

#[test]
fn generates_pll_title_with_round_and_date() {
    assert_eq!(
        generate_pll_title(Some("87"), Some(1_750_417_200)),
        "제 87회 프로듀서 레터 라이브 6월 20일 방송 결정!"
    );
}

#[test]
fn generates_pll_title_without_date() {
    assert_eq!(
        generate_pll_title(Some("XX"), None),
        "제 XX회 프로듀서 레터 라이브 X월 XX일 방송 결정!"
    );
}

#[test]
fn parses_regular_maintenance_schedule() {
    let body = "記 日　時：2026年3月24日(火) 15:00より19:00頃まで ※終了予定時刻に関しては、状況により変更する場合があります。";
    let parsed = parse_maintenance_schedule(body)
        .expect("schedule parsed")
        .expect("schedule present");
    assert_eq!(parsed.0, 1_774_332_000);
    assert_eq!(parsed.1, 1_774_346_400);
}

#[test]
fn parses_follow_up_maintenance_schedule() {
    let body = "記 日　時：2026年2月5日(木) 14:00より17:10頃まで 対　象：ファイナルファンタジーXIVをご利用のお客様";
    let parsed = parse_maintenance_schedule(body)
        .expect("schedule parsed")
        .expect("schedule present");
    assert_eq!(parsed.0, 1_770_267_600);
    assert_eq!(parsed.1, 1_770_279_000);
}

#[test]
fn parses_pll_start_from_detail_text() {
    let body = "第91回 FFXIVプロデューサーレターLIVE 日時 2026年3月13日（金）20:00頃～ ※開始時間は変更される場合があります。";
    let start = extract_pll_start(body)
        .expect("pll parse succeeded")
        .expect("pll start exists");
    assert_eq!(start, 1_773_399_600);
}

#[test]
fn extracts_maintenance_summary_without_cutting_at_shaki() {
    let html = r#"
        <article>
          <h1>[メンテナンス]全ワールド メンテナンス作業のお知らせ(3/24)</h1>
          <div class="news__detail__wrapper">
            下記日時におきまして、パッチ7.45 HotFixesに伴う全ワールドのメンテナンス作業を実施いたします。
            メンテナンス作業中、ファイナルファンタジーXIVをご利用いただくことができません。
            記
            日　時：2026年3月24日(火) 15:00より19:00頃まで
          </div>
        </article>
        "#;

    assert_eq!(
        extract_maintenance_summary_ja(html).as_deref(),
        Some("パッチ7.45 HotFixesに伴う全ワールドのメンテナンス作業を実施いたします。")
    );
}

#[test]
fn trims_maintenance_intro_phrase() {
    assert_eq!(
        trim_maintenance_intro(
            "下記日時におきまして、パッチ7.45 HotFixesに伴う全ワールドのメンテナンス作業を実施いたします。"
        ),
        "パッチ7.45 HotFixesに伴う全ワールドのメンテナンス作業を実施いたします。"
    );
}

#[test]
fn rejects_pll_cache_without_schedule_details() {
    let info = PllInfo {
        fixed_title: "제 90회 프로듀서 레터 라이브 X월 XX일 방송 결정!".to_string(),
        url: "https://example.com".to_string(),
        start_stamp: None,
        expire_time: 0,
        translated_description: Some("요전날 방송된 ...".to_string()),
        translated_contents: Vec::new(),
        stream_links: Vec::new(),
    };

    assert!(!is_valid_pll_schedule(&info));
}

#[test]
fn classifies_follow_up_emergency_maintenance() {
    assert_eq!(
        classify_maintenance_notice(
            "[続報]全ワールド 緊急メンテナンス作業 終了時間変更のお知らせ(12/25)"
        ),
        MaintenanceNoticeKind::EmergencyFollowUp
    );
}
