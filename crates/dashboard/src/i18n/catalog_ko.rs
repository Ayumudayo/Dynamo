//! Korean translations for strings that modules provide to the dashboard
//! (module manifests, settings schemas, and command descriptions).
//!
//! The English source string is the lookup key, gettext-style, so a module
//! string that changes upstream falls back to English instead of showing a
//! stale translation. `tests::every_catalog_string_has_a_korean_translation`
//! keeps this table complete for the registered modules.

pub(crate) fn ko_catalog(source: &str) -> Option<&'static str> {
    Some(match source {
        // Module names and categories
        "Currency" => "환율",
        "Giveaway" => "경품 추첨",
        "Info" => "정보",
        "Game Info" => "게임 정보",
        "Greeting" => "환영 인사",
        "Use dashboard fields or advanced JSON to configure greeting output." => {
            "아래 항목으로 인사 메시지의 내용과 형식을 설정합니다."
        }
        "Configure message text, channel, and embed preview for this event." => {
            "인사 메시지 문구, 채널, 임베드 미리보기를 설정합니다."
        }
        "Invite Tracking" => "초대 추적",
        "Moderation" => "모더레이션",
        "Suggestion" => "건의",
        "Suggestions" => "건의",
        "Stats" => "통계",
        "Ticket" => "티켓",
        "Ticketing" => "티켓",
        "Stock" => "주식",
        "Utility" => "유틸리티",

        // Module descriptions
        "Exchange rate commands backed by Toss Invest midRate data." => {
            "토스증권 기준 환율(midRate) 데이터를 사용하는 환율 명령어입니다."
        }
        "Optional first-party giveaway workflow with persisted entry tracking." => {
            "참가 기록을 저장하는 선택형 자체 경품 추첨 기능입니다."
        }
        "Read-only commands that describe the bot and runtime." => {
            "봇과 실행 환경 정보를 보여주는 조회 전용 명령어입니다."
        }
        "Game utility commands and referral links." => "게임 유틸리티 명령어와 추천 링크입니다.",
        "Welcome and farewell messages with inviter placeholders." => {
            "초대한 사람 정보를 넣을 수 있는 입장·퇴장 메시지입니다."
        }
        "Tracks inviter attribution and reward roles for member joins and leaves." => {
            "멤버 입장·퇴장 시 초대한 사람을 기록하고 보상 역할을 관리합니다."
        }
        "Slash-first moderation commands with warning ledger support." => {
            "경고 기록을 지원하는 슬래시 명령어 기반 모더레이션 기능입니다."
        }
        "Guild suggestion board with approval and rejection workflows." => {
            "승인·거절 흐름을 갖춘 서버 건의 게시판입니다."
        }
        "Guild member activity tracking for messages, interactions, XP, and voice time." => {
            "메시지, 상호작용, 경험치, 음성 채널 시간 등 멤버 활동을 집계합니다."
        }
        "Ticket setup, channel creation, and close workflow." => {
            "티켓 패널 설정, 채널 생성, 종료 흐름을 제공합니다."
        }
        "Stock and ETF quote commands with refresh sessions." => {
            "자동 갱신을 지원하는 주식·ETF 시세 명령어입니다."
        }

        // Settings sections
        "Optional giveaway module configuration." => "경품 추첨 모듈의 선택 설정입니다.",
        "Referral Links" => "추천 링크",
        "Customize the links and artwork used by /wtinv." => {
            "/wtinv에서 사용하는 링크와 이미지를 설정합니다."
        }
        "Welcome" => "입장 메시지",
        "Farewell" => "퇴장 메시지",
        "Configure invite tracking and reward role thresholds." => {
            "초대 추적과 보상 역할 기준을 설정합니다."
        }
        "Configure modlog output and max-warning escalation." => {
            "모더레이션 로그 채널과 경고 누적 시 자동 조치를 설정합니다."
        }
        "Configure the suggestion board channels and moderator roles." => {
            "건의 게시판 채널과 관리 역할을 설정합니다."
        }
        "Configure XP tracking and level-up messaging." => {
            "경험치 집계와 레벨업 메시지를 설정합니다."
        }
        "Configure ticket panel text, log channel, open-ticket limit, and category roles." => {
            "티켓 패널 문구, 로그 채널, 동시 티켓 수 제한, 분류별 역할을 설정합니다."
        }
        "Quotes" => "시세",
        "Customize stock defaults and ETF ticker groups." => {
            "주식 기본값과 ETF 종목 묶음을 설정합니다."
        }
        "Exchange Defaults" => "환전 기본값",
        "Defaults applied when /exchange arguments are omitted." => {
            "/exchange에서 인자를 생략했을 때 사용할 기본값입니다."
        }
        "Rate Result Currencies" => "환율 결과 통화",
        "Only KRW and USD are supported. Blank or matching values fall back to the opposite currency for the selected base." => {
            "KRW와 USD만 지원합니다. 비워 두거나 기준 통화와 같으면 반대 통화로 표시합니다."
        }
        "ETF Tickers" => "ETF 종목",
        "Set up to five ETF tickers for /etf in display order." => {
            "/etf에 표시할 ETF 종목을 순서대로 최대 다섯 개까지 지정합니다."
        }

        // Field labels
        "Default channel" => "기본 채널",
        "Entry button label" => "참가 버튼 문구",
        "Embed title" => "임베드 제목",
        "War Thunder link" => "워썬더 링크",
        "World of Tanks link" => "월드 오브 탱크 링크",
        "Thumbnail URL" => "썸네일 URL",
        "Enabled" => "사용",
        "Channel" => "채널",
        "Content" => "본문",
        "Embed Description" => "임베드 설명",
        "Embed Color" => "임베드 색상",
        "Embed Thumbnail" => "임베드 썸네일",
        "Embed Footer" => "임베드 바닥글",
        "Embed Image" => "임베드 이미지",
        "Tracking enabled" => "추적 사용",
        "Reward ranks" => "보상 등급",
        "Modlog channel" => "모더레이션 로그 채널",
        "Max warning limit" => "최대 경고 횟수",
        "Max warning action" => "최대 경고 시 조치",
        "Suggestion channel" => "건의 채널",
        "Approved channel" => "승인 채널",
        "Rejected channel" => "거절 채널",
        "Staff roles" => "관리 역할",
        "Level-up message" => "레벨업 메시지",
        "Level-up channel" => "레벨업 채널",
        "Panel title" => "패널 제목",
        "Panel description" => "패널 설명",
        "Panel footer" => "패널 바닥글",
        "Log channel" => "로그 채널",
        "Ticket category ID" => "티켓 카테고리 ID",
        "Open ticket limit" => "동시 티켓 수 제한",
        "Categories" => "분류",
        "Default stock symbol" => "기본 종목 코드",
        "ETF tickers" => "ETF 종목",
        "Refresh interval seconds" => "갱신 간격(초)",
        "Refresh duration seconds" => "갱신 지속 시간(초)",
        "Default from currency" => "기본 기준 통화",
        "Default to currency" => "기본 환산 통화",
        "Default amount" => "기본 금액",
        "Result currency 1" => "결과 통화 1",
        "Result currency 2" => "결과 통화 2",
        "Result currency 3" => "결과 통화 3",
        "Result currency 4" => "결과 통화 4",
        "Result currency 5" => "결과 통화 5",
        "Result currency 6" => "결과 통화 6",
        "Ticker 1" => "종목 1",
        "Ticker 2" => "종목 2",
        "Ticker 3" => "종목 3",
        "Ticker 4" => "종목 4",
        "Ticker 5" => "종목 5",

        // Select option labels
        "Timeout" => "타임아웃",
        "Kick" => "추방",
        "Ban" => "차단",
        "South Korean Won (KRW)" => "대한민국 원 (KRW)",
        "United States Dollar (USD)" => "미국 달러 (USD)",
        "Unused" => "사용 안 함",

        // Field help text
        "Primary channel for giveaway announcements." => "경품 추첨 공지를 올릴 기본 채널입니다.",
        "Button text shown on giveaway messages." => "경품 추첨 메시지에 표시되는 버튼 문구입니다.",
        "Displayed at the top of the /wtinv embed." => "/wtinv 임베드 상단에 표시됩니다.",
        "Referral URL for the War Thunder button." => "워썬더 버튼에 연결할 추천 URL입니다.",
        "Referral URL for the World of Tanks button." => {
            "월드 오브 탱크 버튼에 연결할 추천 URL입니다."
        }
        "Thumbnail image shown in the /wtinv embed." => {
            "/wtinv 임베드에 표시할 썸네일 이미지입니다."
        }
        "Whether this greeting should be sent." => "이 인사 메시지를 보낼지 정합니다.",
        "Target channel for the greeting." => "인사 메시지를 보낼 채널입니다.",
        "Optional plain-text content template." => "선택 사항인 일반 텍스트 본문 템플릿입니다.",
        "Optional embed description template." => "선택 사항인 임베드 설명 템플릿입니다.",
        "Hex color like `#068ADD`." => "#068ADD 같은 16진수 색상 코드입니다.",
        "Hex color such as #068ADD." => "#068ADD 형식의 16진수 색상 코드입니다.",
        "Use the member avatar as the embed thumbnail." => {
            "멤버 아바타를 임베드 썸네일로 사용합니다."
        }
        "Optional embed footer template." => "선택 사항인 임베드 바닥글 템플릿입니다.",
        "Optional embed image URL template." => "선택 사항인 임베드 이미지 URL 템플릿입니다.",
        "Enable inviter attribution for member joins and leaves." => {
            "멤버 입장·퇴장 시 초대한 사람을 기록합니다."
        }
        "Configure automatic role rewards granted when a member reaches an invite count." => {
            "지정한 초대 횟수를 달성한 멤버에게 자동으로 부여할 보상 역할을 설정합니다."
        }
        "Optional channel for moderation action embeds." => {
            "모더레이션 조치 기록을 남길 선택 채널입니다."
        }
        "Auto-action threshold for warnings. Set 0 to disable." => {
            "자동 조치가 실행되는 경고 횟수입니다. 0이면 사용하지 않습니다."
        }
        "Action to take when warning threshold is reached." => {
            "경고 횟수에 도달했을 때 실행할 조치입니다."
        }
        "Guild text channel where new suggestions are posted." => {
            "새 건의가 올라갈 텍스트 채널입니다."
        }
        "Optional target channel for approved suggestions. Leave empty to edit in place." => {
            "승인된 건의를 옮길 채널입니다. 비워 두면 원래 메시지를 수정합니다."
        }
        "Optional target channel for rejected suggestions. Leave empty to edit in place." => {
            "거절된 건의를 옮길 채널입니다. 비워 두면 원래 메시지를 수정합니다."
        }
        "Roles that may moderate suggestions." => "건의를 관리할 수 있는 역할입니다.",
        "Enable message, interaction, XP, and voice tracking." => {
            "메시지, 상호작용, 경험치, 음성 시간 집계를 사용합니다."
        }
        "Template used when a member levels up." => "멤버가 레벨업할 때 보낼 메시지 템플릿입니다.",
        "Optional channel override for level-up messages." => {
            "레벨업 메시지를 보낼 채널입니다. 비워 두면 해당 대화 채널에 보냅니다."
        }
        "Embed title used in the ticket creation panel." => "티켓 생성 패널의 임베드 제목입니다.",
        "Embed description used in the ticket creation panel." => {
            "티켓 생성 패널의 임베드 설명입니다."
        }
        "Embed footer used in the ticket creation panel." => {
            "티켓 생성 패널의 임베드 바닥글입니다."
        }
        "Optional channel that receives ticket closed notifications and transcripts." => {
            "티켓 종료 알림과 대화 기록을 받을 선택 채널입니다."
        }
        "Optional Discord category ID where new ticket channels are created." => {
            "새 티켓 채널을 생성할 선택 사항 Discord 카테고리 ID입니다."
        }
        "Maximum number of concurrently open ticket channels." => {
            "동시에 열 수 있는 티켓 채널의 최대 개수입니다."
        }
        "Add ticket topics and select the staff roles notified for each topic." => {
            "문의 분류를 추가하고 각 분류를 담당할 관리 역할을 선택합니다."
        }
        "Used by /stock when no symbol is supplied." => {
            "/stock에서 종목을 지정하지 않았을 때 사용합니다."
        }
        "ETF tickers used by /etf, one per line." => {
            "/etf에서 사용할 ETF 종목입니다. 한 줄에 하나씩 입력하세요."
        }
        "Controls live embed edit cadence. Minimum 3 seconds." => {
            "실시간 임베드를 갱신하는 간격입니다. 최소 3초입니다."
        }
        "Controls how long live embeds update. Allowed range: 60-180 seconds." => {
            "실시간 임베드를 갱신하는 총 시간입니다. 60~180초 사이로 지정하세요."
        }
        "Used when /exchange is called without a source currency." => {
            "/exchange에서 기준 통화를 생략했을 때 사용합니다."
        }
        "Used when /exchange is called without a target currency." => {
            "/exchange에서 환산 통화를 생략했을 때 사용합니다."
        }
        "Used when /exchange is called without an amount." => {
            "/exchange에서 금액을 생략했을 때 사용합니다."
        }
        "Only KRW and USD are supported. Leave blank or repeat the base currency to use the opposite currency." => {
            "KRW와 USD만 지원합니다. 비워 두거나 기준 통화와 같게 두면 반대 통화를 사용합니다."
        }
        "Optional ticker symbol. Leave blank to skip this slot." => {
            "선택 사항인 종목 코드입니다. 비워 두면 이 칸을 건너뜁니다."
        }

        // Command descriptions
        "Convert one currency amount into another currency." => "금액을 다른 통화로 환산합니다.",
        "Show the configured exchange-rate board for one base currency." => {
            "기준 통화 하나에 대해 설정된 환율표를 보여줍니다."
        }
        "Start a new giveaway message in the configured channel." => {
            "설정된 채널에서 새 경품 추첨을 시작합니다."
        }
        "List the giveaways currently tracked in this guild." => {
            "이 서버에서 진행 중인 경품 추첨 목록을 보여줍니다."
        }
        "End an active giveaway immediately." => "진행 중인 경품 추첨을 즉시 종료합니다.",
        "Pause an active giveaway so no new entries can be accepted." => {
            "진행 중인 경품 추첨을 일시 정지해 새 참가를 막습니다."
        }
        "Resume a paused giveaway and extend its end time by the paused duration." => {
            "일시 정지된 경품 추첨을 재개하고, 정지했던 시간만큼 종료 시각을 늦춥니다."
        }
        "Pick a fresh set of winners for an already ended giveaway." => {
            "이미 종료된 경품 추첨의 당첨자를 다시 뽑습니다."
        }
        "Edit the schedule, prize, or winner count of an active giveaway." => {
            "진행 중인 경품 추첨의 일정, 상품, 당첨자 수를 수정합니다."
        }
        "Check whether the bot is alive and responding." => {
            "봇이 정상적으로 응답하는지 확인합니다."
        }
        "Check gateway and REST response latency." => {
            "게이트웨이 및 REST 응답 지연 시간을 확인합니다."
        }
        "Show runtime details such as loaded modules, uptime, and persistence status." => {
            "불러온 모듈, 가동 시간, 저장소 상태 등 실행 정보를 보여줍니다."
        }
        "Show runtime details such as version, loaded modules, uptime, and persistence status." => {
            "버전, 불러온 모듈, 가동 시간, 저장소 상태 등 실행 정보를 보여줍니다."
        }
        "List commands currently enabled for this server." => {
            "현재 이 서버에서 활성화된 명령어 목록을 보여줍니다."
        }
        "View level, XP, message count, and voice activity for a server member." => {
            "서버 멤버의 레벨, 경험치, 메시지 수, 음성 활동 시간을 확인합니다."
        }
        "View the top members in this server ranked by XP, messages, or voice time." => {
            "경험치, 메시지 수, 음성 활동 시간 기준 상위 멤버 순위를 보여줍니다."
        }
        "View tracked invite statistics for yourself or another member." => {
            "본인 또는 다른 멤버의 초대 통계를 확인합니다."
        }
        "Show the top inviters in this server." => "이 서버의 상위 초대자 순위를 보여줍니다.",
        "Manage stored invite counts for server members." => {
            "서버 멤버의 초대 횟수 기록을 관리합니다."
        }
        "Add bonus invites to a member." => "멤버에게 보너스 초대 횟수를 추가합니다.",
        "Deduct invites from a member." => "멤버의 초대 횟수를 차감합니다.",
        "Reset all tracked invite counters for a member." => {
            "멤버의 모든 초대 집계 기록을 초기화합니다."
        }
        "Manage giveaway lifecycle actions for this guild." => {
            "이 서버의 경품 추첨 진행 상태를 관리합니다."
        }
        "Manage greeting previews for the configured welcome and farewell templates." => {
            "설정된 입장·퇴장 인사 템플릿의 미리보기를 관리합니다."
        }
        "Manage the stored warning history for a guild member." => {
            "서버 멤버의 경고 기록을 관리합니다."
        }
        "Manage the guild's ticket panel, open tickets, and ticket access controls." => {
            "서버의 티켓 패널, 열린 티켓 및 접근 권한을 관리합니다."
        }
        "Show the War Thunder and World of Tanks referral panel for this guild." => {
            "이 서버의 워썬더·월드 오브 탱크 추천 패널을 보여줍니다."
        }
        "Show the latest known FFXIV global maintenance window." => {
            "가장 최근에 확인된 FFXIV 글로벌 서버 점검 일정을 보여줍니다."
        }
        "Show the latest known Producer Letter Live schedule." => {
            "가장 최근에 확인된 프로듀서 레터 라이브 일정을 보여줍니다."
        }
        "Preview a welcome or farewell message using the current guild settings." => {
            "현재 서버 설정으로 입장·퇴장 메시지를 미리 봅니다."
        }
        "Issue a warning to a guild member." => "멤버에게 경고를 부여합니다.",
        "List the warnings currently stored for a guild member." => {
            "멤버에게 저장된 경고 목록을 보여줍니다."
        }
        "Clear all stored warnings for a guild member." => "멤버의 경고를 모두 삭제합니다.",
        "Timeout a guild member for a specific duration." => {
            "멤버를 지정한 시간 동안 타임아웃합니다."
        }
        "Remove an active timeout from a guild member." => "멤버의 타임아웃을 해제합니다.",
        "Kick a guild member from the server." => "멤버를 서버에서 추방합니다.",
        "Ban a user from the server." => "사용자를 서버에서 차단합니다.",
        "Unban a user by their Discord user ID." => "Discord 사용자 ID로 차단을 해제합니다.",
        "Ban and immediately unban a user to remove recent messages." => {
            "사용자를 차단했다가 바로 해제해 최근 메시지를 삭제합니다."
        }
        "Change or clear a guild member's nickname." => "멤버의 별명을 바꾸거나 지웁니다.",
        "Submit a new suggestion to the guild suggestion board." => {
            "서버 건의 게시판에 새 건의를 올립니다."
        }
        "Post or refresh the ticket creation panel in a target channel." => {
            "지정한 채널에 티켓 생성 패널을 올리거나 새로 고칩니다."
        }
        "Set or clear the channel that receives ticket transcripts and close logs." => {
            "티켓 대화 기록과 종료 로그를 받을 채널을 지정하거나 해제합니다."
        }
        "Set the maximum number of open ticket channels for this guild." => {
            "이 서버에서 동시에 열 수 있는 티켓 채널 수를 정합니다."
        }
        "Close the current ticket channel and archive its transcript." => {
            "현재 티켓 채널을 닫고 대화 기록을 보관합니다."
        }
        "Close every open ticket channel in the current guild." => {
            "이 서버의 열린 티켓 채널을 모두 닫습니다."
        }
        "Grant a user or role access to the current ticket channel." => {
            "현재 티켓 채널에 사용자나 역할의 접근을 허용합니다."
        }
        "Remove a user or role from the current ticket channel." => {
            "현재 티켓 채널에서 사용자나 역할을 제거합니다."
        }
        "Show a quote for one stock symbol and keep it refreshable for a short time." => {
            "종목 하나의 시세를 보여주고 잠시 동안 자동으로 갱신합니다."
        }
        "Show the configured ETF watchlist for this guild." => {
            "이 서버에 설정된 ETF 관심 종목을 보여줍니다."
        }
        "No description provided." => "설명이 없습니다.",

        _ => return None,
    })
}
