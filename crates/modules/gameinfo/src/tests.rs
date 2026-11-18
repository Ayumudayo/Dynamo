use crate::{
    lodestone::{parse_article_title_and_body, parse_detail_links},
    maintenance::{
        MaintenanceNoticeKind, build_maintenance_embed, classify_maintenance_notice,
        extract_maintenance_summary_ja, format_maintenance_title, maintenance_scope,
        next_maintenance_poll_seconds, parse_maintenance_schedule, parse_maintenance_start_stamp,
        should_recheck_notice_detail, should_skip_historical_notice, trim_maintenance_intro,
    },
    pll::{PllInfo, extract_pll_start, generate_pll_title, is_valid_pll_schedule},
};
use chrono::TimeZone;

fn maintenance_window(start: i64, end: i64) -> crate::maintenance::MaintInfo {
    crate::maintenance::MaintInfo {
        id: "notice".into(),
        start_stamp: start,
        end_stamp: end,
        end_time_unknown: false,
        title_kr: "점검".into(),
        url: "https://jp.finalfantasyxiv.com/lodestone/news/detail/notice".into(),
        scope: "전 월드".into(),
        body_fingerprint: 0,
        translated_description: None,
        stale: false,
    }
}

#[test]
fn planned_end_keeps_listing_polling_active_without_discarding_cached_schedule() {
    let info = maintenance_window(10_000, 20_000);
    assert!(!should_skip_historical_notice(&info, None, 20_001));
    assert_eq!(next_maintenance_poll_seconds(Some(&info), 20_001), 60);
    let after_grace = 20_000 + 24 * 60 * 60;
    assert!(!should_skip_historical_notice(
        &info,
        Some(&info),
        after_grace
    ));
    assert_eq!(next_maintenance_poll_seconds(Some(&info), after_grace), 300);
    assert!(should_skip_historical_notice(&info, None, after_grace));
}

#[test]
fn expired_cached_detail_is_skipped_but_new_follow_up_is_fetched() {
    let mut info = maintenance_window(10_000, 20_000);
    assert!(should_recheck_notice_detail(&info.url, Some(&info), 19_999));
    assert!(!should_recheck_notice_detail(
        &info.url,
        Some(&info),
        20_000
    ));
    assert!(!should_recheck_notice_detail(
        &info.url,
        Some(&info),
        200_000
    ));
    assert!(should_recheck_notice_detail(
        "new-follow-up-url",
        Some(&info),
        20_001
    ));
    info.end_time_unknown = true;
    assert!(should_recheck_notice_detail(&info.url, Some(&info), 20_001));
}

#[test]
fn elapsed_planned_end_is_displayed_as_unconfirmed_without_countdown() {
    let now = chrono::Utc::now().timestamp();
    let info = maintenance_window(now - 3600, now - 60);
    let embed = serde_json::to_value(build_maintenance_embed(&info)).unwrap();
    let fields = embed["fields"].as_array().unwrap();
    assert!(
        !fields
            .iter()
            .any(|field| field["name"] == "종료까지 남은 시간")
    );
    assert!(
        embed["footer"]["text"]
            .as_str()
            .unwrap()
            .contains("종료 여부 미확인")
    );
}

#[test]
fn scopes_japan_region_platform_notices_separately() {
    assert_eq!(
        maintenance_scope("全ワールド メンテナンス作業", ""),
        "전 월드"
    );
    assert_eq!(
        maintenance_scope("日本データセンター メンテナンス作業", ""),
        "일본 DC 한정"
    );
    assert_eq!(
        maintenance_scope(
            "全ワールド メンテナンス作業",
            "日本リージョンのPlayStation®4版"
        ),
        "일본 리전 · PS4 한정"
    );
}

#[test]
fn poll_interval_wakes_at_one_hour_boundary() {
    use crate::maintenance::MaintInfo;
    let info = MaintInfo {
        id: "id".into(),
        start_stamp: 10_000,
        end_stamp: 20_000,
        end_time_unknown: false,
        title_kr: "title".into(),
        url: "url".into(),
        scope: "전 월드".into(),
        body_fingerprint: 0,
        translated_description: None,
        stale: false,
    };
    assert_eq!(next_maintenance_poll_seconds(Some(&info), 6_300), 100);
    assert_eq!(next_maintenance_poll_seconds(Some(&info), 6_399), 1);
    assert_eq!(next_maintenance_poll_seconds(Some(&info), 6_400), 60);
}

#[test]
fn parses_start_for_maintenance_with_unknown_end_time() {
    let start = parse_maintenance_start_stamp("2026年10月15日(木) 18:00より 終了時刻未定")
        .expect("valid date")
        .expect("start found");
    assert_eq!(
        start,
        chrono_tz::Asia::Tokyo
            .with_ymd_and_hms(2026, 10, 15, 18, 0, 0)
            .unwrap()
            .timestamp()
    );
}

#[test]
fn parses_maintenance_article_after_lodestone_wrapper_changes() {
    let html = r#"
        <html>
          <head>
            <meta property="og:title" content="[続報]全ワールド メンテナンス作業 延長のお知らせ(3/24-25)">
          </head>
          <body>
            <h1>ニュース</h1>
            <main>
              <section class="updated-detail-layout">
                延長後の作業予定です。<br>
                日 時：2025年3月24日(月) 19:00より<br>
                2025年3月25日(火) 17:15頃まで
              </section>
            </main>
          </body>
        </html>
    "#;

    let (title, body) = parse_article_title_and_body(html).expect("maintenance article found");
    assert!(title.contains("全ワールド"));
    assert!(
        parse_maintenance_schedule(&body)
            .expect("schedule parsed")
            .is_some()
    );
}

#[test]
fn finds_maintenance_title_outside_link_in_changed_list_layout() {
    let html = r#"
        <ul>
          <li class="new-list-card">
            <span>[続報]全ワールド メンテナンス作業 終了時間変更のお知らせ(3/24)</span>
            <a href="/lodestone/news/detail/example-maintenance-id" aria-label="お知らせ詳細"></a>
          </li>
        </ul>
    "#;

    let links = parse_detail_links(html, r#"a[href*="/lodestone/news/detail/"]"#);
    assert_eq!(links.len(), 1);
    assert!(links[0].title.contains("全ワールド"));
}

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
        format_maintenance_title(
            1_750_104_000,
            1_750_154_400,
            MaintenanceNoticeKind::Emergency,
        ),
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
fn parses_historical_overnight_maintenance_follow_up() {
    // Lodestone's 2025-03-24 follow-up puts the next day's full date after より.
    let body = "日 時：2025年3月24日(月) 19:00より 2025年3月25日(火) 17:15頃まで";
    let parsed = parse_maintenance_schedule(body)
        .expect("schedule parsed")
        .expect("schedule present");
    assert!(parsed.1 > parsed.0);
}

#[test]
fn parses_lodestone_october_2026_maintenance_time_change() {
    let body = "記 日　時：2026年10月12日(月) 19:00より 2026年10月13日(火) 18:25頃まで";
    let parsed = parse_maintenance_schedule(body)
        .expect("schedule parsed")
        .expect("schedule present");

    assert_eq!(parsed.1 - parsed.0, 84_300);
    assert_eq!(
        classify_maintenance_notice(
            "[続報]全ワールド メンテナンス作業 終了時間変更のお知らせ(10/12-13)"
        ),
        MaintenanceNoticeKind::FollowUp
    );
}

#[test]
fn parses_announced_emergency_schedule_without_date_label() {
    // Lodestone's planned emergency notice states the window in prose, not a 日時 block.
    let body = "緊急メンテナンス作業の実施時間は、「2026年8月7日(金) 15:00より19:00頃まで」を予定しております。";
    assert!(
        parse_maintenance_schedule(body)
            .expect("schedule parsed")
            .is_some()
    );
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

#[test]
fn classifies_extension_title_as_follow_up() {
    assert_eq!(
        classify_maintenance_notice("[続報]全ワールド メンテナンス作業 延長のお知らせ(12/16)"),
        MaintenanceNoticeKind::FollowUp
    );
}
