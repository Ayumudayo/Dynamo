use crate::{
    lodestone::{parse_article_title_and_body, parse_detail_links},
    maintenance::{
        MaintenanceNoticeKind, build_maintenance_embed, classify_maintenance_notice,
        extract_maintenance_summary_ja, format_maintenance_title, has_unknown_end_time,
        maintenance_scope, next_maintenance_poll_seconds, parse_maintenance_schedule,
        parse_maintenance_start_stamp, should_recheck_notice_detail, should_skip_historical_notice,
        trim_maintenance_intro,
    },
    pll::{
        PllInfo, build_pll_stream_buttons, extract_pll_start, generate_pll_title,
        is_valid_pll_schedule, parse_pll_candidates, parse_pll_detail,
    },
};
use chrono::TimeZone;

fn maintenance_window(start: i64, end: i64) -> crate::maintenance::MaintInfo {
    crate::maintenance::MaintInfo {
        id: "notice".into(),
        start_stamp: start,
        end_stamp: end,
        end_time_unknown: false,
        completed: false,
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
        completed: false,
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

#[test]
fn parses_valid_pll_detail_with_schedule_and_stream_links() {
    let url = "https://jp.finalfantasyxiv.com/lodestone/topics/detail/24840cb713392c9615cf58fde06dbd8f940a16e8";
    let html = r#"
        <article class="news__detail">
          <header class="news__header">
            <h1>「第94回 FFXIV PLL」12月4日（金）放送決定！</h1>
          </header>
          <div class="news__detail__wrapper">
            <img class="mdl-img__visual" src="https://img.finalfantasyxiv.com/t/24840cb713392c9615cf58fde06dbd8f940a16e8.png">
            <p class="mdl-text__sm-m16 mdl-space--lg">次回のプロデューサーレターLIVEは、「白銀のワンダラー」でアップデート予定のキャラクターメイキングについて、実機プレイを交えつつ最新情報をお届けします！</p>
            <h3 class="mdl-title__heading--lg">第94回 FFXIVプロデューサーレターLIVE</h3>
            <h4 class="mdl-title__heading--sm mdl-color__green">日時</h4>
            <p class="mdl-text__sm-m16 mdl-space--md"><span class="mdl-color__orange"><strong>2026年12月4日（金）20:00頃～</strong></span><br><span class="mdl-color__notes">※開始時間は変更される場合があります。</span></p>
            <h4 class="mdl-title__heading--sm mdl-color__green">視聴方法</h4>
            <ul class="mdl-list__basic mdl-space--md">
              <li class="mdl-space--xs"><a href="https://sqex.to/pPY9t" target="_blank">YouTube</a></li>
              <li class="mdl-space--xs"><a href="https://sqex.to/QUOIA" target="_blank">ニコニコ生放送</a></li>
              <li class="mdl-space--xs"> <a href="https://sqex.to/htj" target="_blank">Twitch</a></li>
            </ul>
            <h4 class="mdl-title__heading--sm mdl-color__green">放送内容</h4>
            <ul class="mdl-list__basic mdl-space--md">
              <li class="mdl-space--xs">「白銀のワンダラー」キャラクターメイキング 最新情報</li>
              <li class="mdl-space--xs">その他お知らせ</li>
            </ul>
          </div>
        </article>
    "#;

    let parsed = parse_pll_detail(url, html)
        .expect("pll detail parse succeeded")
        .expect("pll detail matched");
    assert!(is_valid_pll_schedule(&parsed.info));
    assert_eq!(
        parsed.info.fixed_title,
        "제 94회 프로듀서 레터 라이브 12월 4일 방송 결정!"
    );
    let expected_start = chrono_tz::Asia::Tokyo
        .with_ymd_and_hms(2026, 12, 4, 20, 0, 0)
        .unwrap()
        .timestamp();
    assert_eq!(parsed.info.start_stamp, Some(expected_start));
    assert_eq!(parsed.info.stream_links.len(), 3);
    assert!(parsed.summary_ja.is_some());
    assert_eq!(parsed.content_items_ja.len(), 2);
    assert_eq!(build_pll_stream_buttons(&parsed.info).len(), 1);
}

#[test]
fn rejects_pll_recap_detail_without_schedule_or_stream_links() {
    let url = "https://jp.finalfantasyxiv.com/lodestone/topics/detail/example-recap";
    let html = r#"
        <article class="news__detail">
          <header class="news__header">
            <h1>「第93回 FFXIV PLL」まとめ公開！</h1>
          </header>
          <div class="news__detail__wrapper">
            <p>先日放送された「第93回 FFXIV PLL」のまとめを公開しました。</p>
          </div>
        </article>
    "#;

    let parsed = parse_pll_detail(url, html)
        .expect("pll detail parse succeeded")
        .expect("pll detail matched");
    assert!(!is_valid_pll_schedule(&parsed.info));
}

#[test]
fn parses_pll_candidates_from_topics_list_even_if_image_link_precedes_title() {
    let html = r#"
        <ul>
          <li class="news__list--topics">
            <div><a href="/lodestone/topics/detail/24840cb713392c9615cf58fde06dbd8f940a16e8"><img src="https://img.finalfantasyxiv.com/t/24840cb713392c9615cf58fde06dbd8f940a16e8.png" alt=""></a></div>
            <p class="news__list--title"><a href="/lodestone/topics/detail/24840cb713392c9615cf58fde06dbd8f940a16e8">「第94回 FFXIV PLL」12月4日（金）放送決定！</a></p>
          </li>
          <li class="news__list--topics">
            <p class="news__list--title"><a href="/lodestone/topics/detail/a063e78ef6679960efafe09001200eebbb2173b7">第65回FFXIVプロデューサーレター：「白銀のワンダラーへの冒険準備キャンペーン」について</a></p>
          </li>
        </ul>
    "#;

    let candidates = parse_pll_candidates(html);
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].url,
        "https://jp.finalfantasyxiv.com/lodestone/topics/detail/24840cb713392c9615cf58fde06dbd8f940a16e8"
    );
}

#[test]
fn parses_pll_start_with_ascii_parentheses_and_tilde() {
    let start = extract_pll_start("2026年12月4日(金) 20:00頃~")
        .expect("pll parse succeeded")
        .expect("pll start exists");
    let expected = chrono_tz::Asia::Tokyo
        .with_ymd_and_hms(2026, 12, 4, 20, 0, 0)
        .unwrap()
        .timestamp();
    assert_eq!(start, expected);
}

#[test]
fn parses_rescheduled_follow_up_using_after_schedule_not_before() {
    let body = "お知らせしておりますメンテナンス作業について、以下のとおり、実施日時を変更させていただきます。 変更前：2024年6月26日(水) 18:00より 2024年6月28日(金) 18:00頃まで 変更後：2024年6月28日(金) 11:00より18:00頃まで 記 日　時：2024年6月28日(金) 11:00より18:00頃まで";
    let (start, end) = parse_maintenance_schedule(body)
        .expect("schedule parsed")
        .expect("schedule present");
    assert_eq!(
        start,
        chrono_tz::Asia::Tokyo
            .with_ymd_and_hms(2024, 6, 28, 11, 0, 0)
            .unwrap()
            .timestamp()
    );
    assert_eq!(
        end,
        chrono_tz::Asia::Tokyo
            .with_ymd_and_hms(2024, 6, 28, 18, 0, 0)
            .unwrap()
            .timestamp()
    );

    let body_without_datetime_block = "変更前：2024年6月26日(水) 18:00より 2024年6月28日(金) 18:00頃まで 変更後：2024年6月28日(金) 11:00より18:00頃まで";
    let (after_start, after_end) = parse_maintenance_schedule(body_without_datetime_block)
        .expect("schedule parsed")
        .expect("schedule present");
    assert_eq!(after_start, start);
    assert_eq!(after_end, end);
}

#[test]
fn parses_follow_up_with_koro_yori_start_time() {
    let body = "記 日　時：2025年4月9日(水) 12:30頃より14:05頃まで";
    let (start, end) = parse_maintenance_schedule(body)
        .expect("schedule parsed")
        .expect("schedule present");
    let expected_start = chrono_tz::Asia::Tokyo
        .with_ymd_and_hms(2025, 4, 9, 12, 30, 0)
        .unwrap()
        .timestamp();
    let expected_end = chrono_tz::Asia::Tokyo
        .with_ymd_and_hms(2025, 4, 9, 14, 5, 0)
        .unwrap()
        .timestamp();
    assert_eq!(start, expected_start);
    assert_eq!(end, expected_end);
    assert_eq!(
        parse_maintenance_start_stamp(body)
            .expect("start stamp parsed")
            .expect("start stamp present"),
        expected_start
    );
}

#[test]
fn parses_extension_follow_up_with_unknown_end_time_variants() {
    let body = "記 日　時：2026年10月21日(水) 17:30より ※終了予定時刻は未定です。";
    assert!(has_unknown_end_time(body));
    assert!(has_unknown_end_time("終了時刻未定"));
    assert!(has_unknown_end_time("終了時間未定"));
    assert!(has_unknown_end_time("終了時刻は未定"));
    assert_eq!(
        parse_maintenance_start_stamp(body)
            .expect("start stamp parsed")
            .expect("start stamp present"),
        chrono_tz::Asia::Tokyo
            .with_ymd_and_hms(2026, 10, 21, 17, 30, 0)
            .unwrap()
            .timestamp()
    );
}

#[test]
fn parses_completion_follow_up_and_renders_completed_embed() {
    let title = "[続報]【日本リージョンPlayStation®4版のみ】 全ワールド メンテナンス作業 終了のお知らせ(11/11)";
    let kind = classify_maintenance_notice(title);
    assert_eq!(kind, MaintenanceNoticeKind::Completed);
    let body = "下記日時にメンテナンス作業を終了いたしました。 記 日　時：2025年11月11日(火) 15:00より19:01頃まで 対　象：日本リージョンPlayStation®4版ファイナルファンタジーXIVをご利用のお客様";
    let (start, end) = parse_maintenance_schedule(body)
        .expect("schedule parsed")
        .expect("schedule present");
    let scope = maintenance_scope(title, body);
    let formatted = format_maintenance_title(start, end, kind).replacen("전 월드", scope, 1);
    assert_eq!(formatted, "일본 리전 · PS4 한정 유지보수 작업 종료 (11/11)");

    let mut info = maintenance_window(start, end);
    info.completed = true;
    info.title_kr = formatted;
    info.scope = scope.to_string();
    let embed = serde_json::to_value(build_maintenance_embed(&info)).unwrap();
    let fields = embed["fields"].as_array().unwrap();
    assert!(
        !fields
            .iter()
            .any(|field| field["name"] == "종료까지 남은 시간")
    );
    assert_eq!(
        embed["footer"]["text"].as_str(),
        Some("점검 종료 공지 확인됨")
    );
}

#[test]
fn preserves_published_at_when_pinned_duplicate_lacks_time() {
    let html = r#"
        <div>
          <a href="/lodestone/news/detail/pinned-item">全ワールド メンテナンス作業のお知らせ</a>
          <a href="/lodestone/news/detail/newer-follow-up">
            [続報] 全ワールド メンテナンス作業 終了時間変更のお知らせ
            <time><script>ldst_strftime(1794300900, 'YMD');</script></time>
          </a>
          <a href="/lodestone/news/detail/pinned-item">
            全ワールド メンテナンス作業のお知らせ
            <time><script>ldst_strftime(1793948400, 'YMD');</script></time>
          </a>
        </div>
    "#;
    let links = parse_detail_links(html, r#"a[href*="/lodestone/news/detail/"]"#);
    assert_eq!(links.len(), 2);
    let pinned = links
        .iter()
        .find(|link| link.url.ends_with("/pinned-item"))
        .unwrap();
    assert_eq!(pinned.published_at, Some(1_793_948_400));
}
