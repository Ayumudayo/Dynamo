# Dynamo 보안·성능·대시보드 UI 개선 작업 계획서

작성일: 2026-07-13

대상 저장소: Dynamo

기준 브랜치: refactor

기준 커밋: 714a17b1728889054cb6fbf45f8d98e0554b1413

예상 총공수: 71~109 엔지니어일

예상 중요 경로: 3인 병렬 기준 7~10주, 7일 관찰과 24시간 soak 포함

## 1. 문서 목적

이 문서는 Dynamo의 보안 취약 경로, 런타임 병목, 대시보드 사용성 문제를 실제 제품 코드에서 개선하기 위한 통합 실행 계획서다.

기존의 세부 감사 문서는 근거 자료로 유지하되, 실제 구현 순서와 작업 경계는 이 문서를 기준으로 관리한다.

- 보안 상세 근거: docs/superpowers/plans/2026-07-12-security-remediation.md
- 성능 상세 근거: docs/superpowers/plans/2026-07-12-performance-remediation.md
- UI 상세 근거: docs/superpowers/plans/2026-07-12-dashboard-ux-remediation.md
- 전체 의존성 근거: docs/superpowers/plans/2026-07-12-security-performance-dashboard-remediation-program.md

이 문서는 일정, 작업 소유권, 병렬화, 상태 보고의 기준이다. 각 상세 근거 문서의 인터페이스, RED/GREEN, fail-closed, rollback 계약은 계속 규범적이며 이 문서의 축약된 문구로 약화되지 않는다. 충돌하면 더 엄격한 안전·검증 기준을 적용하고, 작업 추적표에는 충돌과 결정을 기록한다.

이 계획은 새로운 마이크로서비스나 프런트엔드 프레임워크를 도입하지 않는다. 현재 Rust 단일 호스트 구조, Axum 서버 렌더링, MongoDB, vanilla CSS/JavaScript, Playwright 구성을 유지한다.

## 2. 현재 상태 정정

| 항목 | 현재 상태 | 의미 |
| --- | --- | --- |
| 상위 보안·성능·UI 계획 | 작성 완료 | 개선 대상과 장기 의존성은 정리되어 있음 |
| 실행 통제 부트스트랩 | 완료 | 커밋 714a17b, 제품 동작 변경 없음 |
| 보안 제품 개선 | 미착수 | 실제 취약 경로는 그대로 존재 |
| 성능 제품 개선 | 미착수 | 설정 반복 조회, 무제한 작업, 전체 문서 교체 등이 그대로 존재 |
| 대시보드 UI 개선 | 미착수 | 상태 오표시, false success, 모바일·접근성 문제가 그대로 존재 |
| 전체 제품 개선 진척도 | 0% | W1-01 보안 바닥과 W0-00 deterministic font를 제품 작업으로 즉시 병렬 시작 |

714a17b의 PowerShell 파일은 계획 실행을 보호하는 선행 통제일 뿐, 이 계획의 제품 개선 실적으로 계산하지 않는다.

기존 제어-plane은 현 상태로 동결한다. 외부 evidence root, publisher, integration ref 초기화는 제품 작업의 선행조건이 아니며, 별도 승인이 없는 한 추가 구현·보강하지 않는다.

## 3. 개선 목표와 완료 정의

### 3.1 보안

- 대시보드 쓰기와 Discord component/modal 액션은 요청 시점의 현재 권한을 다시 확인한다.
- 캐시된 설정은 권한 부여 근거가 될 수 없다.
- 모든 Discord side effect는 source guild에 귀속된 타입을 통해서만 실행한다.
- 일반 메시지는 mention을 기본 거부한다.
- Mongo 쓰기는 lost update가 없는 원자 전이로 수행한다.
- 무제한 세션, 작업, waiter, cache, retry 상태를 제거한다.
- 배포 시 비밀 파일 권한, SSH host key, 설치 artifact hash를 fail-closed로 검증한다.

### 3.2 성능

- warm 상태의 stats-disabled 메시지는 settings 저장소를 조회하지 않는다.
- cold settings snapshot은 deployment 1회와 guild 1회 이하로 조회한다.
- guild 상세 페이지는 대상 guild Discord 조회 1회 이하로 제한한다.
- 실제 Stock worker는 deployment/guild/user 기준 16/4/2 이하로 유지한다.
- 동일 calendar miss 32개는 upstream fetch 1회로 합친다.
- Mongo hot query는 명명된 인덱스를 사용하며 COLLSCAN을 허용하지 않는다.
- 대시보드 public HTML decoded 크기는 10 KiB 이하를 목표로 한다.
- 모든 controlled route의 p95는 기준선보다 20% 넘게 악화되지 않는다.

### 3.3 대시보드 UX

- local gate, effective state, blocker를 서로 다른 정보로 표시한다.
- 설정·Discord·audit·sync 조회 실패를 기본값이나 빈 상태로 위조하지 않는다.
- Saved는 서버의 권위 있는 최종 상태와 revision을 확인한 뒤에만 표시한다.
- pending 중 중복 제출을 막고 timeout은 Outcome unknown으로 처리한다.
- 375, 768, 1024, 1440px에서 가로 스크롤이 없어야 한다.
- 키보드만으로 모든 기능을 사용할 수 있어야 한다.
- focus-visible, label, live region, alert, 44px touch target, AA contrast를 만족한다.
- 초기 HTML에는 settings modal/form을 렌더링하지 않고 권한 확인 후 하나만 가져온다.

## 4. 핵심 설계 원칙

1. 제품 코드 우선
   - 선행 통제 결함이 발견되지 않는 한 추가 제어 스크립트 작성을 제품 작업으로 대체하지 않는다.
   - 각 단계는 실제 Rust, CSS, JavaScript, Mongo, Playwright 변경을 산출해야 한다.

2. 하나의 경계는 한 번만 구현
   - Mongo index, atomic transition, WorkSupervisor, settings snapshot, Discord client를 보안과 성능 작업에서 중복 구현하지 않는다.

3. 권한과 캐시는 분리
   - LiveSettingsSnapshot은 uncached/fail-closed다.
   - 성능용 cached snapshot은 일반 이벤트와 명령에서만 사용한다.

4. UI는 서버 상태를 소비
   - 브라우저가 receipt, outbox, sync ledger를 새로 만들지 않는다.
   - UI는 서버가 제공하는 target, revision, outcome, audit, sync 상태만 표현한다.

5. 안전 바닥은 롤백하지 않음
   - 유한 timeout, 원자 writer, additive index, work cap, cache cap, deny-all mention은 영구 안전 바닥이다.

6. 기존 디자인 보존
   - 현재 dark panel hierarchy와 Fira Sans/Fira Code를 유지한다.
   - 일반적인 portfolio grid나 다른 폰트로 재브랜딩하지 않는다.
   - hover보다 상태 명확성, 키보드 focus, 오류 복구, 모바일 작업 순서를 우선한다.

## 5. 현재 코드에서 확인된 주요 문제

| ID | 우선순위 | 문제 | 현재 코드 근거 |
| --- | --- | --- | --- |
| CUR-01 | P0 | 대시보드가 session guild 목록을 현재 권한으로 신뢰 | crates/dashboard/src/main.rs |
| CUR-02 | P0 | 길드 설정 read가 get_or_create를 사용해 GET이 write를 유발 | crates/repositories/src/lib.rs, crates/dashboard/src/main.rs |
| CUR-03 | P1 | raw ChannelId를 통해 guild 검증 없이 Discord effect 실행 | giveaway, greeting, stats, suggestion, ticket 모듈 |
| CUR-04 | P1 | giveaway/suggestion/invite/stats가 전체 문서 replace 기반 | crates/persistence-mongo/src/lib.rs |
| CUR-05 | P1 | Mongo collection은 만들지만 필수 index 계약이 없음 | crates/persistence-mongo/src/lib.rs |
| CUR-06 | P1 | 이벤트에서 deployment/guild settings를 반복 조회 | crates/access, crates/modules/stats, crates/persistence-api |
| CUR-07 | P1 | Stock map에서 제거해도 spawn된 worker가 계속 실행 | crates/modules/stock/src/state.rs |
| CUR-08 | P1 | Dashboard와 Toss HTTP client에 전체 deadline이 없음 | crates/dashboard/src/main.rs, crates/providers/tossinvest/src/client.rs |
| CUR-09 | P1 | 대시보드 스위치가 local gate 대신 effective_enabled를 표시 | crates/dashboard/src/main.rs |
| CUR-10 | P1 | response.ok만으로 Saved를 표시 | crates/dashboard/src/main.rs 내 inline JavaScript |
| CUR-11 | P1 | 설정·sync·audit·Discord 오류를 default/empty/missing으로 축약 | crates/dashboard/src/main.rs |
| CUR-12 | P2 | CSS/JavaScript/modal이 초기 HTML에 모두 inline 또는 선렌더 | crates/dashboard/src/main.rs |
| CUR-13 | P2 | 기본 Playwright smoke가 실제 ETF 설정을 변경 | tests/playwright/dashboard-guild-smoke.spec.cjs |
| CUR-14 | P2 | PM2 로그 중복 기록 및 rotation 부재 | ecosystem.config.js, ecosystem.pm2.cjs |
| CUR-15 | P2 | Pi bundle이 무압축·무검증 tar 중심 | scripts/deploy-rpi-aarch64.sh, scripts/deploy-rpi-aarch64.ps1 |

### 5.1 문제-작업 추적

| 문제 | 해결 작업 | 최종 증거 |
| --- | --- | --- |
| CUR-01 | W2-01, W3-04, W5-03 | stale grant RED/GREEN, current-auth trace, receipt reconciliation |
| CUR-02 | W1-03 | GET write counter 0, absent/unavailable/present fixture |
| CUR-03 | W2-03A, W2-03B | guild mismatch fixture, raw effect inventory |
| CUR-04 | W3-02, W3-03A, W3-03B | isolated Mongo concurrency 결과 |
| CUR-05 | W3-01 | 11개 exact index readback, hot-query explain |
| CUR-06 | W4-02, W4-04C | cold/warm repository call counter |
| CUR-07 | W4-01, W4-03 | permit/task gauge, cancel/join fixture |
| CUR-08 | W1-02, W4-04A, W4-04B | delayed transport deadline·retry fixture |
| CUR-09 | W5-01 | local-on/effective-off 고정 fixture |
| CUR-10 | W2-01, W3-04, W5-03 | ack-loss, duplicate click, Saved tuple |
| CUR-11 | W5-02, W5-03 | unavailable/empty/missing 상태 matrix |
| CUR-12 | W0-00, W1-04, W5-05 | transfer/DOM budget과 modal 0→1→0 |
| CUR-13 | W0-01, W1-04 | isolated launcher, selected test 수, mutation 0 |
| CUR-14 | W6-02 | PM2 설정 readback과 rotation fixture |
| CUR-15 | W6-03 | deterministic archive, manifest/hash/install readback |

추가 보안 폐쇄 대상인 36개 occurrence는 W6-05에서 원 경로, 회귀 테스트, 통제, rollout 증거까지 별도로 추적한다.

### 5.2 상세 규범 매핑

| 통합 작업 | 상세 근거 작업 |
| --- | --- |
| W0-00 | DUI-00 deterministic font |
| W0-01 | Performance E0, DUI baseline extension |
| W1-01 | Security Task 1 |
| W1-02 | Performance E1a |
| W1-03 | DUI-01의 write-free read 선행 작업 |
| W1-04 | DUI-01, Performance E8a |
| W1-05 | DUI-04 |
| W2-01 | Security Task 2A |
| W2-02 | Security Task 3 |
| W2-03A/B | Security Task 4~5의 typed Discord effect 경계 |
| W3-01 | Security Task 6, Performance E3 |
| W3-02 | Performance E4 |
| W3-03A | Security Task 7, Performance E7a |
| W3-03B | Security Task 8 |
| W3-04 | Security Task 2B, Performance E2a |
| W4-01 | Security Task 9 |
| W4-02 | Performance E2b |
| W4-03/04A/B/C | Security Task 10, Performance E5/E6 |
| W4-05 | Performance E7b |
| W4-06 | Security Task 11 |
| W5-01~06 | DUI-02~08 |
| W6-03 | secure deployment task, Performance E10 |
| W6-05 | Security Task 12 |

각 통합 작업의 완료는 이 표에 매핑된 상세 근거의 필수 fixture와 rollback 하한도 함께 통과해야 한다.

## 6. 전체 실행 순서

    W1-01 tactical security  ───────────────────────────────┐
                                                           │
    Phase 0 baseline                                       │
       ├─ W1-02/03/04/05 ── Phase 2 auth/effect ───────────┤
       ├─ W3-01 index ── W3-02/03 ── W3-04 durability ─────┤
       └─ W4-01 supervisor foundation ── W4 consumers ─────┤
                                                           │
    W1-03/04 ── W5-01/02 early UI                          │
    W3-04 + W4-02 ── W5-03/04/05 ── W5-06 UX gate ────────┤
                                                           │
    7일 관찰 + W6-01/02/03/05 ── W6-04 canary/soak ────────┘

병렬화 가능:

- Phase 1의 HTTP 경계와 UI 자산/접근성은 서로 다른 파일 소유권으로 병렬 진행 가능하다.
- Phase 3의 Mongo index 작업과 Phase 4의 WorkSupervisor 기반은 병렬 준비 가능하다.
- Phase 5의 상태 독립적 반응형·접근성 작업은 서버 mutation durability와 병렬 진행 가능하다.

병렬화 금지:

- atomic writer cutover는 index/duplicate gate 전에 진행하지 않는다.
- settings cache는 live authorization 타입을 감싸거나 대체하지 않는다.
- Stock, giveaway, suggestion 소비자는 WorkSupervisor와 atomic transition 양쪽이 준비되기 전 전환하지 않는다.
- authoritative save UI는 서버 receipt/CAS/outcome 계약 전에 구현하지 않는다.

### 6.1 공용 경계의 단일 소유권

| 공용 경계 | 단일 소유 작업 | 소비 작업 |
| --- | --- | --- |
| isolated Mongo runner | W0-01 | W3-01~04, W6-05 |
| DiscordApiClient와 GuildPresenceCache | W1-02 | W2-01, W5-02; W4-04C는 API/bytes를 바꾸지 않고 budget만 재검증 |
| mutation request ID와 receipt ABI | W2-01 | W3-04가 같은 ABI를 durable하게 확장, W5-03이 소비 |
| live uncached settings snapshot | W2-02 | W4-02는 별도 cached snapshot만 제공 |
| Mongo index manifest | W3-01 | W3-02, W3-03A/B, W3-04 |
| audit idempotency와 retention eligibility | W3-03B | W3-04 outbox/materialization |
| WorkSupervisor, CappedTtlMap, 범용 SingleFlight | W4-01 | W4 이후 신규 소비자; W1-02 GuildPresenceCache 내부 구현은 제외 |
| deterministic font/license/route | W0-00 | W0-01, W1-04, W5-06 |
| hashed CSS/JavaScript pipeline | W1-04 | W5-01~06 |
| UX 연구 protocol과 baseline schema | W0-01 | W5-06 |

소비 작업은 같은 타입이나 저장 형식을 새로 만들지 않는다. 필요한 계약이 부족하면 소유 작업으로 변경 요청을 되돌리고 한 경계에서만 수정한다.

## 7. 단계별 작업 계획

## Phase 0. 기준선과 테스트 기반

공수 합계: 6~8 엔지니어일

### W0-00. Self-hosted deterministic Fira 폰트

예상: 1 엔지니어일

대상:

- crates/dashboard/assets/fonts/FiraSans-Variable.woff2
- crates/dashboard/assets/fonts/FiraCode-Variable.woff2
- crates/dashboard/assets/fonts/OFL.txt
- crates/dashboard/assets/fonts/fonts.lock.json
- crates/dashboard/build.rs
- crates/dashboard/Cargo.toml
- crates/dashboard/src/main.rs

작업:

- Google Fonts 요청을 제거한다.
- 검증된 Fira Sans/Code WOFF2와 OFL-1.1을 저장소에 포함한다.
- build.rs가 font의 SHA-256 content path를 생성한다.
- same-origin font/woff2 route에 정확한 content type, immutable cache, ETag를 적용한다.

완료 기준:

- 외부 font 요청 0.
- license, lock, family/weight, route, header, asset body hash 일치.
- timing과 CLS는 이 작업에서 판정하지 않고 W0-01의 고정 harness에서 baseline으로 기록한다.

### W0-01. 재현 가능한 성능·브라우저·UX 기준선

예상: 4~6 엔지니어일

선행조건:

- W0-00

대상:

- scripts/perf
- tests/perf
- tests/playwright/helpers
- 격리 dashboard launcher
- isolated Mongo runner와 contract
- MongoDB 7 CI job

측정:

- route p50/p95
- HTML/CSS/JS/font 전송 bytes
- Discord/Toss outbound 수
- Mongo operation 수
- active task/waiter/cache cardinality
- process RSS
- layout overflow와 DOM node 수
- 첫 텍스트 표시, font 적용 시간, CLS, computed font face
- W5-06의 다섯 사용자 과업별 시간, 오류, 도움 요청, SEQ

UX baseline protocol:

- baseline 10명과 final 10명은 서로 다른 참가자로 구성한다.
- 각 cohort는 Dynamo를 가끔 쓰는 관리자 5명과 정기적으로 쓰는 관리자 5명으로 구성하고 구현 참여자는 제외한다.
- 참가자 1명당 다섯 과업을 한 번씩 수행해 phase당 정확히 50 trial을 만든다.
- 과업 순서는 balanced Latin-square로 배치한다.
- browser, viewport, font-ready 조건, 평가자 script, fixture hash를 두 phase에서 고정한다.
- completed, unassisted, duration_ms, help_count, critical_error, SEQ 1~7을 trial별로 기록한다.
- shared, staging, production guild와 database는 사용하지 않는다.

완료 기준:

- 동일 revision과 fixture를 두 번 실행해 동일한 schema의 JSON 결과를 생성한다.
- 테스트용 Mongo URI와 production URI가 같으면 실행을 거부한다.
- 성공, 오류, panic 모두 exact database cleanup을 확인한다.
- Playwright는 local fake transport를 사용하며 live Discord/Toss에 요청하지 않는다.
- UX baseline은 동일한 비운영 fixture, 참가자 구성, 과업 문구, 도움 규칙으로 기록한다.
- benchmark metadata에 revision, OS, power profile, CPU, Rust/Node/browser version, build profile, fixture hash를 기록한다.

### W0-02. Strict Clippy 기준선 복구

예상: 0.5 엔지니어일

대상:

- crates/providers/tossinvest/src/stock.rs

작업:

- ActivePriceField 변형명을 Pre, Regular, Post로 정리한다.
- 기존 phase-to-price-field 동작은 유지한다.

완료 기준:

- focused Toss regression 통과.
- workspace strict Clippy 통과.
- 이 작업 전 strict Clippy는 known RED로 기록하고, 완료 뒤부터 공통 필수 GREEN으로 사용한다.

## Phase 1. 빠른 위험 감소와 안전한 UI 기반

공수 합계: 7~11 엔지니어일

### W1-01. Moderation 권한과 secret file 보호

예상: 1~2 엔지니어일

대상:

- crates/modules/moderation
- scripts/lib/secure-env.sh
- production build/bootstrap/postdeploy scripts
- 관련 CI

작업:

- 경고 삭제 대상의 역할 계층과 protected-target 정책을 한 함수로 통합한다.
- .env 생성은 no-clobber와 mode 0600을 강제한다.
- concurrent creator, umask 000, symlink, pre-existing target, wrong owner/mode, failed copy와 temp cleanup을 테스트한다.
- 두 Pi build bundle의 scripts/lib/secure-env.sh exact path/hash/executable과 세 production entrypoint의 source 계약을 검증한다.

완료 기준:

- 모든 denial에서 WarningRepository::clear_for_member 호출 0, 승인 경로는 정확히 1회.
- .env owner/mode/readback 정확.
- 기존 insecure 일반 cp 경로 제거.

### W1-02. 유한 HTTP client와 Discord directory

예상: 2~3 엔지니어일

대상:

- 신규 crates/dashboard/src/discord_api.rs
- 신규 crates/dashboard/src/guild_presence.rs
- crates/dashboard/src/main.rs
- crates/providers/tossinvest/src/client.rs

작업:

- 하나의 공유 DiscordApiClient를 만든다.
- guild selector와 detail의 presence/directory 경계를 분리한다.
- Dashboard connect 3초, directory 8초 deadline을 적용한다.
- Toss connect 5초, total 15초 deadline을 적용한다.
- token refresh 중 lock을 잡고 무기한 network await하지 않는다.
- presence cache는 positive TTL 60초, negative TTL 15초, cap 2,048, same-key single-flight를 이 작업에서 한 번만 구현한다.
- route authorization은 바꾸지 않는다. 5초 current-auth budget과 DashboardDiscordApi adapter는 W2-01이 같은 client 위에 구현한다.

완료 기준:

- guild detail cold lookup 1회 이하, warm 0회.
- selector upstream 동시성 8 이하.
- stalled upstream이 budget 내 종료.
- Discord 오류는 Missing이 아니라 Unavailable.

### W1-03. Write-free guild settings read

예상: 1 엔지니어일

대상:

- crates/repositories/src/lib.rs
- crates/persistence-api/src/lib.rs
- crates/persistence-mongo/src/lib.rs
- crates/dashboard/src/main.rs

인터페이스:

- GuildSettingsRepository::get(guild_id) -> Result<Option<GuildSettings>, Error>

작업:

- GET/read path에서 get_or_create를 제거한다.
- absent document는 domain default presentation으로만 처리한다.
- mutation만 새 guild document를 만들 수 있다.

완료 기준:

- dashboard GET과 default smoke가 Mongo write 0회.
- absent, unavailable, existing 상태가 서로 구분된다.

### W1-04. CSS/JavaScript 자산 추출과 read-only smoke

예상: 1~2 엔지니어일

대상:

- crates/dashboard/assets/dashboard.css
- crates/dashboard/assets/dashboard.js
- crates/dashboard/build.rs
- crates/dashboard/Cargo.toml
- crates/dashboard/src/main.rs
- tests/playwright/dashboard-guild-smoke.spec.cjs
- tests/playwright/helpers/dashboard.cjs

작업:

- inline CSS와 JavaScript를 content-hashed same-origin asset으로 이동한다.
- W0-00 asset pipeline을 확장해 CSS/JavaScript의 SHA-256 content path를 생성한다.
- Brotli/gzip, 정확한 content type, immutable cache, ETag를 적용한다.
- 기본 smoke에서 ETF toggle/save와 SOXL/TQQQ write oracle을 삭제한다.
- smoke 중 PATCH/POST/PUT/DELETE를 발견하면 즉시 실패한다.
- fresh worktree에서 npm ci와 repository-local Playwright 1.58.2를 사용하고 npx를 호출하지 않는다.
- fake auth로 authenticated smoke를 반드시 실행하고 unmocked outbound는 abort한다.

완료 기준:

- root HTML decoded 10 KiB 이하.
- default smoke mutation 0.
- selected test 1개 이상, skipped 0.
- unmocked outbound 0, Mongo insert/update/delete 0.
- package-lock.json 불변.
- 외부 script/font 요청 0.
- W0-00의 font body/header hash가 변경되지 않는다.

### W1-05. 상태 독립적 접근성과 반응형 기반

예상: 2~3 엔지니어일

대상:

- crates/dashboard/assets/dashboard.css
- crates/dashboard/assets/dashboard.js
- crates/dashboard/src/main.rs
- tests/playwright/dashboard-responsive.spec.cjs
- tests/playwright/dashboard-accessibility.spec.cjs

작업:

- lang, skip link, main landmark, aria-current를 추가한다.
- 검색, select, form field, switch에 label을 연결한다.
- category button은 aria-pressed를 사용한다.
- status는 aria-live, 오류는 role=alert로 발표한다.
- 모든 pointer target을 44x44px 이상으로 만든다.
- focus-visible과 reduced-motion을 보장한다.
- 820px 이하에서 task-first navigation을 제공한다.

필수 route/state matrix:

- / signed-out
- /selector authenticated
- guild와 deployment의 modules, commands, logs
- settings modal open
- unavailable/error

W5-06 최종 matrix는 여기에 save pending/error/success/outcome-unknown, deployment confirmation, Undo, lazy modal Retry, dirty close, 409를 추가한다.

완료 기준:

- 375/768/1024/1440px에서 scrollWidth <= innerWidth.
- keyboard-only로 모든 read-only 화면 탐색 가능.
- AA contrast와 focus visibility 통과.
- 기존 modal focus trap, Escape, focus return 유지.
- 375/768px navigation은 기본 collapsed, 정확한 aria-expanded, Escape·link 선택 닫기, toggle focus return을 만족한다.
- 1024/1440px resize에서 stale mobile state를 초기화하고 logs는 모바일 card로 표시한다.

## Phase 2. 현재 권한과 Discord effect 경계

공수 합계: 8~13 엔지니어일

### W2-01. Dashboard current authorization과 mutation identity

예상: 4~6 엔지니어일

대상:

- crates/dashboard/src/auth.rs
- crates/dashboard/src/mutation.rs
- crates/dashboard/src/discord_api.rs
- crates/dashboard/src/main.rs
- crates/ops
- crates/repositories
- crates/persistence-api
- crates/persistence-mongo

핵심 타입:

- DashboardDiscordApi
- MutationRequestId
- ExpectedValue / NextValue
- DashboardMutationReceipt

작업:

- 쓰기 요청마다 현재 Discord 권한을 5초 budget 안에서 재확인한다.
- canonical UUID request ID를 요구하고 응답에서 echo한다.
- same target in-flight 중복 mutation을 차단한다.
- timeout과 acknowledgement loss를 outcome unknown으로 기록한다.
- outcome unknown 상태에서 settings를 재전송하지 않는다.
- 현재 inventory의 모든 보호 route는 read lease 최대 5초, write lease 0초로 재검사한다.
- missing, non-UTF-8, oversize, control-character request ID는 로그·DB 접근 전에 거부한다.
- PreparedDashboardMutation capability 없이는 business apply를 호출할 수 없게 한다.
- receipt cap 1,024개, row 16 KiB, document 2 MiB, in-flight lease 30초, terminal retention 30일을 ABI에 고정한다.
- target/outcome/sync 조회는 same-ID를 echo하고 Cache-Control: no-store를 사용한다.
- receipt, token, private payload가 settings DTO, cache, HTML, 로그로 투영되지 않는지 검사한다.
- SSR JSON은 less-than, greater-than, ampersand, U+2028, U+2029를 안전하게 escape한다.

완료 기준:

- expired/outage/current-role-change 테스트가 fail-closed.
- 한 사용자 action당 mutation 요청 1건.
- 202/incomplete/malformed success는 Saved가 아니라 status-only.
- 기존 session guild 목록만으로 PATCH를 승인하는 경로 0.
- command-sync는 per-ID A/B/A 이력을 보존하고 TerminalApplied 전에 claim하지 않는다.
- epoch-1 admission seed/readback과 exact cap fixture가 통과한다.

### W2-02. Raw component와 modal의 live policy gate

예상: 1~2 엔지니어일

선행조건:

- W1-03 write-free read
- 공유 component action interface

대상:

- crates/module-kit
- crates/registry
- crates/access
- crates/persistence-api
- crates/app
- crates/bot
- stock, ticket, giveaway, suggestion 모듈

핵심 타입:

- RawInteractionKind
- ComponentActionManifest
- AuthorizedComponentAction
- load_live_settings_snapshot

완료 기준:

- 오래된 버튼/모달도 클릭 시점에 live settings를 다시 읽는다.
- disabled, expired, unavailable 상태의 외부 side effect 0.
- cached settings snapshot은 이 경로에서 사용되지 않는다.
- 현 11개 action manifest가 등록되고 duplicate kind/custom_id는 startup 실패.
- unknown ID는 dispatch 0.
- 결정당 deployment 1 + guild 1 live read 이하.
- 중앙 gate rollback 시에도 기존 모듈의 local effective-state 검사를 유지.

### W2-03A. Guild-scoped Discord effect 계약

대상:

- 신규 crates/discord-effects

핵심 타입:

- GuildScopedChannel
- SafeMessage
- SafeEditMessage

작업:

- constructor를 crate-private으로 두고 raw ChannelId를 guild-bound handle로 변환하는 검증 경계를 만든다.
- source guild 불일치, DM, missing resource를 거부한다.
- ordinary message는 deny-all mentions가 기본이다.
- 필요한 mention만 정확한 allowlist로 연다.

완료 기준:

- same-guild fixture 성공.
- cross-guild/DM/missing/timeout fixture 실패.
- denial 시 send/edit/delete 0.

예상: 1~2 엔지니어일

### W2-03B. 여섯 모듈 Discord effect 전환

선행조건:

- W2-03A
- raw component 소비자는 W2-02

대상:

- giveaway, greeting, moderation, stats, suggestion, ticket
- bot message defaults
- Discord effect inventory CI

작업:

- configured side-effect sink를 GuildScopedChannel, SafeMessage, SafeEditMessage로 전환한다.
- Poise와 일반 메시지의 allowed mentions 기본값을 deny-all로 고정한다.
- ticket transcript 저장이 성공한 뒤에만 source channel을 삭제한다.

완료 기준:

- 구조화 interaction response 예외를 제외한 configured raw ChannelId effect inventory 0.
- transcript 실패 시 source channel 보존.
- CI가 새 raw send/edit/delete sink를 차단.

예상: 2~3 엔지니어일

## Phase 3. Mongo index와 원자 persistence

공수 합계: 14~21 엔지니어일

### W3-01. 애플리케이션 소유 Mongo index 계약

예상: 3~5 엔지니어일, 실제 duplicate 발견 시 repair/review 일정 별도 산정

대상:

- crates/persistence-mongo/src/indexes.rs
- crates/persistence-mongo/src/migrations.rs
- crates/persistence-mongo/src/lib.rs
- crates/bootstrap
- isolated Mongo tests와 CI

작업:

- 11개 필수 index를 이름, key, uniqueness, partial filter까지 명시한다.
- duplicate preflight와 승인된 repair/quarantine 경로를 만든다.
- apply는 idempotent하고 readback으로 실제 정의를 검증한다.
- 대표 query의 explain을 저장하고 IXSCAN을 강제한다.

완료 기준:

- duplicate 0 또는 승인된 backup/repair 완료.
- 11개 index exact readback.
- totalDocsExamined / max(nReturned, 1) <= 2.
- 자동 index 삭제 없음.

### W3-02. Member stats 원자 갱신

예상: 2 엔지니어일

대상:

- crates/domain-stats
- crates/repositories
- crates/persistence-api
- crates/persistence-mongo
- crates/modules/stats

핵심 타입:

- MemberStatsDelta
- apply_delta
- try_level_up

완료 기준:

- 동일 member에 대한 동시 increment 100개의 최종 증가량이 정확히 100.
- level CAS 승자 1명.
- legacy read/modify/replace writer 0.

### W3-03A. Giveaway와 suggestion 원자 전이

선행조건:

- W3-01

예상: 2~3 엔지니어일

대상:

- crates/domain-giveaway
- crates/domain-suggestion
- crates/repositories
- crates/persistence-api
- crates/persistence-mongo

작업:

- giveaway toggle/finalize를 versioned atomic transition으로 만든다.
- suggestion reservation/terminal 상태를 원자 전이로 만든다.
- 이 단계는 domain/repository/persistence adapter만 landing하고 module caller와 Cargo.lock을 바꾸지 않는다.

완료 기준:

- unique actor fixture와 동일 actor even/odd fixture의 최종 membership parity가 정확하다.
- finalize는 단조 증가하며 stale writer가 Ended를 Active로 복구하지 못한다.
- duplicate terminal owner 0.

### W3-03B. Invite delta와 audit retention primitive

선행조건:

- W2-01
- W3-02

예상: 2~3 엔지니어일

대상:

- crates/domain-invite
- crates/repositories
- crates/persistence-api
- crates/persistence-mongo
- dashboard audit adapter

작업:

- invite counter를 atomic delta로 갱신한다.
- deletion이나 수동 변경의 불충분한 증거는 InviteUseEvidence::AmbiguousDeletion으로 남기고 추측 보상하지 않는다.
- dashboard audit의 skip pagination을 keyset cursor로 교체한다.
- exact manifest로 검증된 retention eligibility와 accepted tip을 제공한다.
- 선택적 audit TTL은 필수 11개 index 밖에서 기본 비활성으로 둔다.

완료 기준:

- invite delta 손실 0.
- 감사 페이지 중복/누락 0.
- ambiguous deletion에서 임의 counter write 0.
- W3-04가 소비할 retention primitive와 accepted tip readback 일치.

### W3-04. Dashboard mutation durability

예상: 5~8 엔지니어일

선행조건:

- W2-01
- W3-01의 index 11
- W3-03B의 audit retention primitive와 exact accepted tip

작업:

- target, bounded audit outbox, receipt TerminalApplied를 하나의 원자 경계로 묶는다.
- authoritative target/outcome/audit/sync read API를 제공한다.
- old binary와 incomplete epoch를 거부한다.
- W2-01의 request ID와 receipt 타입을 그대로 durable하게 확장하고 두 번째 ABI를 만들지 않는다.
- audit/outbox cap, crash-before/after-apply, acknowledgement-loss, old-binary cutover를 격리 Mongo에서 검증한다.

완료 기준:

- acknowledgement loss와 crash barrier에서 결과가 재수렴한다.
- audit/outbox cap 초과 시 business write 0.
- Saved에 필요한 target, revision, audit, sync tuple이 완전하다.
- v2 TerminalApplied, nonempty frozen audit manifest, exact same-ID sync 결과가 원자적으로 재수렴한다.

## Phase 4. Bounded work와 런타임 비용 제거

공수 합계: 14~23 엔지니어일 + 7일 observe-only

### W4-01. 공용 WorkSupervisor

선행조건:

- W0-01
- W1-02의 timeout/metrics seam

예상: 2~3 엔지니어일

대상:

- 신규 crates/work-control
- crates/runtime-api
- crates/app
- crates/bot
- crates/dashboard

핵심 타입:

- WorkSupervisor
- WorkPermit
- WorkClass
- CappedTtlMap
- SingleFlight

완료 기준:

- deployment/guild/user/resource별 admission cap.
- noisy guild가 다른 guild를 고갈시키지 않음.
- generation cancellation과 shutdown drain 검증.
- map entry 제거 후 detached task가 남지 않음.
- supervisor 내부 scope/resource/generation/queue map도 bounded.
- 10배 cardinality와 ObserveOnly map exhaustion에서 fail-closed.

### W4-02. Settings snapshot cache

예상: 2~4 엔지니어일

선행조건:

- live settings 타입과 W3-04 server boundary.

작업:

- uncached LiveSettingsSnapshot과 별도의 cached snapshot API를 둔다.
- ordinary event/command만 10초 cache를 사용한다.
- same-key miss는 single-flight한다.
- guild cap 2,048, per-key follower 64, waiter deadline을 적용한다.

완료 기준:

- warm stats-disabled message repository read 0.
- 32 same-key cold miss가 deployment 1 + guild 1 read.
- component/modal/current auth는 cache 사용 0.

### W4-03. Stock worker lifecycle

선행조건:

- W4-01
- W2-02 live component policy

예상: 2~3 엔지니어일

대상:

- crates/modules/stock
- crates/service-stock
- WorkSupervisor adapter

작업:

- 실제 worker가 permit을 소유한다.
- disable/reconfigure/shutdown 시 generation을 취소하고 join한다.
- map eviction과 task cancellation을 같은 전이로 묶는다.

완료 기준:

- active worker deployment/guild/user <= 16/4/2.
- 17/5/3번째 admission은 upstream 호출 전 거부.
- cancel 후 추가 fetch/edit 0.
- registry count와 actual task gauge 일치.

### W4-04A. Session, OAuth, GameInfo와 provider retry 제한

선행조건:

- W1-02
- W2-01
- W4-01

예상: 2~3 엔지니어일

대상:

- GameInfo session/translation cache
- currency cooldown
- Toss RetryGate와 rate-limit
- dashboard session store
- OAuth state

기본 cap:

- dashboard session 4,096, per-user 8
- OAuth state 2,048
- Toss active 8, queue 64

완료 기준:

- 10배 cardinality 부하 후 cap 이하.
- leader 오류/cancel에서도 waiter가 해제됨.
- deadline이 retry마다 재시작되지 않음.
- DashboardSessionStore의 secondary index cleanup과 refresh-lock 수명 일치.
- redirect 512 bytes, ETF symbol/portfolio 50/800 bytes, GameInfo candidates 5, currency cooldown 3초, translation backoff 60초→15분.

### W4-04B. Toss calendar, metadata와 baseline single-flight

선행조건:

- W4-04A

예상: 1~2 엔지니어일

기본 cap:

- baseline 8,192
- metadata 2,048

완료 기준:

- calendar concurrent miss 32개가 upstream fetch 1회.
- limiter 대기, token refresh, body read, retry가 하나의 15초 total deadline 안에 종료.
- leader error/cancel에서 모든 follower가 deadline 안에 해제.

### W4-04C. Stats/invite cache adapter와 presence 재검증

선행조건:

- W1-02
- W4-01
- W4-02

예상: 2~3 엔지니어일

기본 cap:

- XP/voice 8,192
- invite 2,048
- voice reconcile batch 128

작업:

- stats/invite의 feature-local map을 CappedTtlMap adapter로 전환한다.
- W1-02 GuildPresenceCache는 API와 내부 coalescing을 byte-for-byte 유지하고 cap/call budget만 재검증한다.
- 두 번째 presence cache나 in-flight registry를 만들지 않는다.

완료 기준:

- 모든 cache가 10배 cardinality 후 cap 이하.
- warm/cold settings와 presence call budget 유지.
- eviction/cancel 뒤 stale write 0.

### W4-05. Giveaway edit coalescing과 feature admission

예상: 1~2 엔지니어일

작업:

- 클릭 DB transition과 Discord public count edit를 분리한다.
- 1초 window에서 edit를 coalesce한다.
- feature-local cap 128/32/4와 map 65,536/4,096을 적용한다.

완료 기준:

- 서로 다른 admitted actor 100명의 동일 message 1초 burst에서 authoritative Discord edit 1회 이하.
- 최종 rendered count가 committed membership과 일치.
- 동일 actor cooldown은 1.999초 거부와 2초 허용 fixture로 검증.
- cap 초과 작업은 DB/Discord 호출 전 거부.
- late ack가 중복 side effect를 만들지 않음.

### W4-06. Suggestion effect ownership

선행조건:

- W3-03A
- W4-01

예상: 2~3 엔지니어일

작업:

- stable nonce와 Dispatching → Recorded 또는 OutcomeUnknown 상태를 영속화한다.
- Discord send 전에 Dispatching을 기록한다.
- acknowledgement loss에서는 최근 bot message 최대 100개만 검색하고 확인 불가 시 자동 재전송하지 않는다.
- per-user pending 20, per-guild active 1,000 cap을 적용한다.
- optional terminal TTL 30~3,650일은 운영 승인 전 비활성으로 둔다.

완료 기준:

- crash-before-send, send-after-before-ack, record-after-before-commit, compensation failure fixture 통과.
- OutcomeUnknown에서 duplicate Discord send 0.
- cap 초과 시 persistence/Discord side effect 0.

## Phase 5. 권위 있고 작업 중심인 대시보드

공수 합계: 15~21 엔지니어일

### W5-01. Local gate, Effective state, Blocker 분리

예상: 2 엔지니어일

대상:

- 신규 crates/dashboard/src/ui_state.rs
- crates/dashboard/src/main.rs

표시 규칙:

- deployment 화면 switch는 deployment_enabled만 편집한다.
- guild 화면 switch는 guild_enabled만 편집한다.
- effective 상태는 별도 badge로 표시한다.
- blocker 우선순위는 installed, parent module, deployment gate, guild gate 순이다.

핵심 fixture:

- deployment=false, guild=true이면 guild switch는 checked.
- Effective: Off.
- Blocker: Disabled deployment-wide.

### W5-02. 조회 실패를 사실대로 표시

예상: 2 엔지니어일

작업:

- settings unavailable은 HTTP 503, no-store, Retry, support reference로 표시한다.
- sync unavailable과 empty를 구분한다.
- audit unavailable과 no events를 구분한다.
- Discord Present, Missing, Unavailable을 구분한다.
- unavailable 화면에는 save/form/toggle을 렌더링하지 않는다.
- settings와 Discord presence 상태는 W1-02/03 뒤 즉시 구현하고, authoritative audit/sync 상태는 W3-04 뒤에 연결한다.

### W5-03. 서버 권위 기반 Save와 outcome reconciliation

선행조건:

- W1-04
- W3-04
- W4-02

예상: 4~6 엔지니어일

대상:

- crates/dashboard/assets/dashboard.js
- crates/dashboard/assets/dashboard.css
- crates/dashboard/src/main.rs
- tests/playwright/dashboard-state.spec.cjs

작업:

- committed seed와 revision을 보관한다.
- seed는 Missing 또는 Present(value)를 보존하고 browser default로 absent를 materialize하지 않는다.
- pending 중 control disable, aria-busy, Saving 발표.
- 401/403/409/5xx/network/timeout을 별도로 표현한다.
- timeout과 incomplete success는 Outcome unknown.
- 같은 request ID로 target, receipt/audit, sync 상태만 조회한다.
- 새 settings write를 자동 재시도하지 않는다.
- Missing 저장의 Undo는 Set(default)가 아니라 Remove를 수행한다.
- Settings가 적용됐지만 sync pending이면 Saved 대신 Settings committed; sync pending으로 분리한다.

완료 기준:

- double click도 PATCH 1건.
- Saved는 v2 TerminalApplied, exact target/revision, nonempty frozen audit manifest, exact same-ID sync 결과가 모두 완전할 때만 표시.
- sync가 NotRequired/Unsupported이면 exact same-ID no_row, Required이면 TerminalSucceeded fingerprint를 요구한다.
- receipt absence, empty audit, Pending, OutcomeUnknown, TerminalFailed는 Saved 불가.
- 409는 최신 상태를 다시 읽고 사용자의 변경을 덮어쓰지 않음.
- outcome unknown에서는 원래 request ID와 Check status again만 활성화하고 새 ID/write를 만들지 않음.
- 성공 response로 switch, effective badge, blocker, count, sync panel을 함께 갱신.

### W5-04. Dirty form, 영향 확인, Undo

예상: 2~3 엔지니어일

작업:

- dirty modal close 시 discard 확인.
- 취소하면 modal/focus/값 유지.
- deployment-wide 변경은 entity, 새 gate 값, 모든 연결 guild 영향, local 값은 유지되지만 effective state가 바뀔 수 있음, command sync 가능성을 확인한다.
- Undo는 새 mutation ID와 이전 revision expectation을 사용한다.
- Undo는 countdown 없이 navigation, 같은 target의 다음 성공 mutation, 명시적 dismiss 중 하나까지 유지한다.
- discard 수락 시 마지막 서버 확정 seed/default로 복구하고 abandoned DOM 값을 제거한다.

완료 기준:

- confirm 취소 시 write 0.
- intervening write가 있으면 Undo 409.
- focus trap, Escape, trigger focus return 유지.
- Undo는 keyboard focus 가능하고 live region으로 발표되며 prior Missing은 정확히 Remove.
- Undo 409에서는 최신 상태를 표시하고 덮어쓰지 않는다.

### W5-05. 긴 목록과 on-demand modal

선행조건:

- W1-04
- W2-01
- W5-01
- W5-03
- W5-04

예상: 2~3 엔지니어일

작업:

- command 초기 12개, Load more 12개.
- 검색/category 변경 시 limit reset.
- Showing N of M을 표시한다.
- 초기 modal DOM 0.
- Settings 클릭 후 권한 확인된 fragment 1개만 fetch.
- close 후 modal DOM 제거.
- fragment는 same-origin, authorization-checked, text/html만 허용하고 script, inline event handler, external URL을 거부한다.
- 모든 동적 문자열은 server-side escape하고 JSON seed는 less-than, greater-than, ampersand, U+2028, U+2029를 escape한다.
- pending 중 trigger를 disable하고 두 번째 modal open을 막는다.

완료 기준:

- modal lifecycle 0 -> 1 -> 0.
- 401/403/404/5xx/timeout에서 modal 생성 0.
- 실패 시 trigger 근처 Retry 제공.
- No matching commands와 Showing N of M을 live region으로 발표하고 Load more는 keyboard reachable.
- hydrate된 form이 W5-03/04의 committed seed, dirty, reconcile 계약을 그대로 사용.

### W5-06. UI 품질 검증

예상: 3~5 엔지니어일, 참가자 모집 대기시간 별도

고정 작업:

1. 지정 guild를 검색하고 지정 module control을 연다.
2. deployment=false, guild=true에서 local gate, effective state, blocker를 정확히 설명한다.
3. 지정 command를 filter하고 form을 수정한 뒤 dirty close를 거부·수락해 committed 값으로 돌아간다.
4. 응답 유실에서 재전송하지 않고 원래 request ID의 status 조회로 결론을 얻는다.
5. deployment 영향 확인 후 변경하고 Undo해 정확한 이전 presence/value로 돌아간다.

연구 protocol:

- W0-01과 다른 final cohort 10명, occasional 5명/regular 5명, 구현 참여자 제외.
- 참가자당 5개 과업, 정확히 50 final trial, balanced Latin-square.
- 같은 fixture hash, browser, viewport, font-ready, 평가자 script를 사용한다.
- 도움은 참가자가 30초 동안 진전하지 못하고 요청했을 때만 정해진 한 문장을 제공하며 해당 trial은 assisted다.
- timer는 과업 카드 공개 시 시작하고 성공 DOM/state 도달 또는 중단 선언 시 종료한다.
- critical error는 false_success, wrong_scope, duplicate_write, unsafe_cleanup, unrecovered_data_loss의 닫힌 목록이다.
- 과업 4·5는 deterministic fake dashboard에서만 수행하고 trial마다 fixture를 초기화한다.
- 예상 route를 모두 mock하고 unmocked request를 abort하며 외부 Mongo/Discord/Toss 요청은 0이다.
- shared/staging/production guild를 사용하지 않는다.

mutation oracle:

- 과업 4는 최초 PATCH 1건, 응답 유실 뒤 status GET만 허용하고 두 번째 write는 0.
- 과업 5는 confirmation 취소 시 write 0, 확정 후 forward write 1, Undo는 새 ID write 1, 최종 state는 시작 state와 동일.

완료 기준:

- critical error 0.
- 50회 중 46회 이상 도움 없이 완료.
- 각 작업 median SEQ 5/7 이상.
- 어떤 작업·경험 집단도 baseline보다 느려지지 않음.
- 최소 3개 작업 median 시간이 10% 이상 개선.
- keyboard-only와 실제 Windows Narrator로 pending/error/success/outcome-unknown, confirmation, Undo, lazy modal Retry, dirty close, 409를 확인한다.
- field error는 aria-invalid와 aria-describedby로 연결하고 submit 실패 후 첫 invalid field로 focus 이동.
- normal/hover/focus/disabled control과 muted/accent text의 계산된 contrast 통과.
- 별도 375px human sanity pass와 전체 route/state 자동 matrix에서 overflow tolerance 0.

## Phase 6. 의존성, 배포, canary

공수 합계: 7~12 엔지니어일 + 24시간 soak

### W6-01. 의존성 보안 갱신

예상: 2~4 엔지니어일, MSRV·compiler·aarch64 fallout 발생 시 별도 contingency

작업:

- cargo audit advisory DB를 갱신한다.
- dependency group별 reverse tree, feature, MSRV, compiler fallout를 검토한다.
- npm audit는 0을 유지한다.
- 모든 dependency를 한 번에 lockfile만 바꾸지 않는다.

완료 기준:

- 각 advisory가 fix 또는 명시적 applicability decision을 가짐.
- workspace test와 aarch64 build 통과.
- advisory DB revision, cargo audit raw result, reverse dependency decision을 증거로 보존.

### W6-02. PM2 로그 보존

예상: 1 엔지니어일

작업:

- canonical PM2 config 하나로 정리한다.
- pm2-logrotate 3.0.0을 설치·검증한다.
- max_size 25M, retain 7, compress true, daily rotation을 적용한다.

완료 기준:

- 실제 PM2 module 설정 readback 일치.
- 중복 combined/out/error 기록 정책 정리.
- rollback은 사전 캡처 설정을 복원하며 로그를 자동 삭제하지 않음.

### W6-03. Deterministic Pi 배포 artifact

선행조건:

- W1-01
- W6-01의 exact final binary
- W6-02 canonical config

예상: 2~3 엔지니어일

작업:

- Windows/Bash 양쪽에서 같은 deterministic tar.gz를 생성한다.
- revision, SHA-256, mode, size, manifest를 기록한다.
- 원격 staging에서 hash를 검증한 뒤 atomic install한다.
- strict host-key checking과 pinned known-hosts를 강제한다.
- build/staging/network 전에 APP_DIR, ASCII DNS/IPv4 host, user, canonical decimal port를 검증한다.
- SSH key와 known-hosts는 regular file이어야 하며 non-22 port는 bracket host:port lookup이 성공해야 한다.
- 모든 ssh/scp는 BatchMode=yes, StrictHostKeyChecking=yes, exact UserKnownHostsFile을 사용하고 key 사용 시 IdentitiesOnly=yes를 적용한다.
- accept-new, implicit user known-hosts, shell-built target을 금지한다.

완료 기준:

- tar.gz가 raw tar보다 작음.
- 동일 committed blobs와 동일 binary stage에서 양 OS archive body hash 일치.
- 설치된 세 binary hash가 manifest와 일치.
- 불완전 또는 hash mismatch artifact는 실행 전 거부.
- invalid input matrix에서 build/create/upload/chmod/remote mutation 0.
- 이 작업은 packaging pipeline과 staging candidate를 검증한다. exact final artifact는 W6-05 GREEN commit 뒤 W6-04 첫 단계에서 같은 pipeline으로 다시 생성한다.

### W6-04. Canary와 rollback rehearsal

선행조건:

- W4의 7일 관찰 완료
- W5-06
- W6-01, W6-02, W6-03
- W6-05 GREEN
- W6-05 closure commit의 exact final HEAD

예상: 1~2 엔지니어일 + 24시간 soak

완료 기준:

- W6-05 GREEN HEAD를 고정하고 full gate를 다시 실행한 뒤 W6-03 pipeline으로 exact final candidate를 생성한다.
- 30분 active canary.
- 24시간 soak.
- error rate 1% 미만.
- controlled p95 악화 20% 이하.
- RSS, cache cardinality, worker gauge 안정.
- current auth shadow mismatch 0.
- rollback artifact도 동일한 보안 바닥, index, atomic writer, 16/4/2 cap을 유지.
- 30분 active canary가 성공한 뒤 exact final candidate로 24시간 soak를 시작.
- relevant code/config/artifact/restart 변경 시 24시간 clock reset.
- 최소 표본이 부족하면 soak를 연장하고 error rate를 단정하지 않음.
- canary/soak evidence는 immutable external artifact에 run ID와 digest로 저장하고 tracked file을 바꾸지 않는다.
- soak 뒤 W6-05 verifier를 read-only로 다시 실행하며 final HEAD에 새 commit을 만들지 않는다.

### W6-05. Security closure matrix

선행조건:

- W1~W6-03의 정적 제품 작업과 W5-06 완료
- W6-03 staging candidate와 binary-affecting tree hash 고정

예상: 1~2 엔지니어일

대상:

- .github/security/2026-07-12-remediation-evidence.md
- scripts/verify-remediation-evidence.cjs
- malformed, duplicate, blank, count fixtures

작업:

- 정확히 36개 unique finding slug를 original path, test, control, rollout, evidence와 연결한다.
- live dependency가 있는 finding은 W6-03 staging candidate로 생성한 immutable external evidence의 URI와 digest를 첨부한다.
- clean committed HEAD에서 locked gates, secure-env, Discord-effect inventory, isolated Mongo, read-only Playwright를 재실행한다.
- W6-04 전에 static과 live-dependent 36개 행을 모두 GREEN으로 만든다.

완료 기준:

- 누락, 중복, blank, 잘못된 count fixture가 verifier RED.
- 36개 행이 모두 exact source와 재현 가능한 GREEN 증거를 가짐.
- closure commit 뒤 새 HEAD에서 전체 gate를 다시 실행해 통과.
- closure evidence-only commit 전후 binary-affecting tree hash가 동일함을 증명하고 이 HEAD를 W6-04 exact final HEAD로 고정한다.
- W6-04 시작 뒤 tracked commit 0.

## 8. 공통 성능·보안·UI 예산

| 항목 | 목표 |
| --- | --- |
| Dashboard connect timeout | 3초 이하 |
| Live authorization total | 5초 이하 |
| Discord directory total | 8초 이하 |
| Dashboard mutation handler | 10초 이하 |
| Browser mutation timeout | 12초 |
| Toss connect / total | 5초 / 15초 |
| Settings warm read | 0 |
| Settings cold read | deployment 1 + guild 1 이하 |
| Settings cache | TTL 10초, guild 2,048, per-key follower 64 |
| Guild detail bot-presence lookup | cold 1, warm 0; current-auth 호출은 별도 |
| Selector concurrency | 8 이하 |
| Presence cache | positive 60초, negative 15초, 2,048 |
| Dashboard session/OAuth | global 4,096, per-user 8 / OAuth 2,048 |
| XP/voice/baseline | 각 8,192 |
| Invite/metadata | 각 2,048 |
| Stock worker | deployment/guild/user 16/4/2 이하 |
| Toss admission | active 8, queue 64 |
| Calendar single-flight | 32 miss -> 1 fetch |
| Giveaway admission/maps | 128/32/4, cooldown 65,536, dirty 4,096 |
| Public HTML decoded | 10 KiB 이하 |
| HTML compression | wire bytes < decoded bytes, br 또는 gzip |
| Immutable asset repeat navigation | unchanged asset body transfer 0 |
| First text / font applied | 고정 harness에서 1초 / 2초 이하 |
| Cumulative Layout Shift | 0.02 이하 |
| Initial settings modal | 0 |
| Responsive widths | 375, 768, 1024, 1440px |
| Touch target | 44x44px 이상 |
| Text contrast | 4.5:1 이상 |
| Controlled p95 regression | 20% 이하 |
| Controlled error rate | 1% 미만 |
| PM2 retention | 25M, 7개, 압축, daily |

성능 판정 protocol:

- baseline/current는 동일 machine, power profile, OS, Rust/Node/browser, build profile, fixture hash, 요청 수와 동시성에서 인접 실행한다.
- warm-up을 제외하고 3~5회 반복한 median p95를 비교한다.
- controlled fixture는 실패 0건을 요구한다. 1% 기준은 canary의 numerator/denominator와 최소 표본을 함께 보고하고 표본 부족 시 관찰을 연장한다.
- RSS 안정은 30분 rolling slope와 95% band가 연속 세 window에서 증가하지 않고 모든 cache/task gauge가 cap 안에 있을 때로 정의한다.
- COLLSCAN 중단은 W3-01이 지정한 대표 hot-query explain fixture에 적용한다.
- Dashboard 10초/browser 12초 mutation budget은 W3-04 durable receipt가 활성화된 뒤 적용한다.
- Toss 15초는 limiter wait, token refresh, body read, 허용 retry 전체를 포함하고 retry가 deadline을 재시작하지 않는다.

## 9. 검증 전략

### 9.1 단위 작업 규칙

- 기존 동작을 고정하는 characterization test를 먼저 작성한다.
- 수정 대상 결함을 증명하는 RED가 실패해야 한다.
- 최소 구현으로 focused GREEN을 만든다.
- 해당 crate와 workspace 전체 검증을 실행한다.
- 한 커밋에 하나의 제품 경계를 담는다.

### 9.2 공통 명령

    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo check --workspace --locked
    cargo test --workspace --all-targets --locked
    cargo audit
    npm ci
    npm audit --json
    npm run dashboard:smoke:isolated

적용 시점:

- strict Clippy는 W0-02 완료 뒤부터 필수 GREEN이다.
- 현재 raw npm run dashboard:smoke는 write 동작이 있어 실행 금지한다.
- dashboard:smoke:isolated는 W1-04가 dynamic loopback, fake auth/transport, read-only fixture, mutation counter 0을 구현한 뒤에만 공통 gate로 승격한다.
- npm 명령은 repository-local CLI만 사용하고 package-lock.json 불변을 확인한다.

추가:

- isolated Mongo Tests/SchemaCheck
- 4-viewport Playwright
- local delayed HTTP/Discord/Toss fixtures
- exact outbound/mutation counters
- aarch64 cross-build
- PM2/canary readback

isolated Mongo 검증은 production URI 불일치 preflight, selected test 1개 이상, ignored test의 명시적 실행, 성공·실패·panic 뒤 exact database cleanup을 요구한다. cargo test --workspace만으로 Mongo gate를 통과한 것으로 간주하지 않는다.

### 9.3 필수 동시성 테스트

- 동일 member 100 increment.
- settings/presence/calendar 동일 miss 32개.
- Stock 17/5/3번째 admission reject.
- cancel/replace/shutdown race.
- giveaway 100 concurrent toggle.
- suggestion duplicate owner/terminal race.
- cache cardinality 10배 부하.
- ack loss와 timeout 후 no-resend.

### 9.4 프런트엔드 보안 기준

- untrusted data를 innerHTML, outerHTML, insertAdjacentHTML에 전달하지 않는다.
- eval, Function constructor, string timeout을 사용하지 않는다.
- browser storage에 session/token을 저장하지 않는다.
- external script/font를 추가하지 않는다.
- navigation URL은 same-origin 또는 명시된 allowlist만 허용한다.
- JavaScript 분리는 unsafe-inline을 추가하기 위한 것이 아니라 strict CSP가 가능한 구조를 만들기 위한 것이다.

## 10. 롤백 정책

롤백 가능:

- 성능 settings cache TTL을 0으로 설정.
- presence cache TTL을 0으로 설정.
- UI business behavior를 이전의 검증된 hashed asset으로 복귀.
- PM2 설정을 사전 캡처 값으로 복원.
- current schema/index와 호환되는 이전 atomic binary로 복귀.

롤백 금지:

- current authorization을 session guild grant로 되돌림.
- 원자 writer를 legacy read/modify/replace로 되돌림.
- 적용 완료된 additive index를 자동 삭제.
- WorkSupervisor hard cap과 cancellation 제거.
- bounded cache를 unbounded HashMap으로 복귀.
- deny-all mention 또는 guild-scoped effect 제거.
- 외부 Google Fonts와 inline asset 경로 복원.
- hash 미검증 배포 artifact 사용.

## 11. 즉시 중단 조건

다음 중 하나라도 발생하면 해당 단계의 promotion을 중단한다.

- 권한 outage/expiry에서 write 또는 Discord effect 발생.
- cross-guild side effect 또는 implicit mention 발생.
- lost/duplicate stats, giveaway, suggestion, invite transition.
- index readback 불일치, duplicate repair 모호성, COLLSCAN.
- active worker가 16/4/2를 초과.
- map count와 실제 task gauge 불일치.
- cache cardinality, waiter, RSS가 계속 증가.
- cache hit가 current authorization 결과를 바꿈.
- timeout 뒤 settings 또는 Discord effect를 자동 재전송.
- outcome unknown인데 UI가 Saved를 표시.
- 375px horizontal overflow.
- default Playwright smoke가 production-like guild/database를 변경.
- p95 20% 초과 악화 또는 error rate 1% 이상.
- candidate/rollback artifact의 revision/hash/mode 불일치.

## 12. 일정과 병렬 실행안

권장 역할:

| 역할 | 주 소유 범위 |
| --- | --- |
| A 보안·런타임 | W1-01, W1-02, W2, W4-01/03/06 |
| B persistence·성능 | W0-01/02, W3, W4-02/04/05 |
| C 대시보드·UX | W1-03/04/05, W5 |
| 운영 지원 | W6-02/03/04, staging evidence |

3인 병렬 계획:

| 주차 | A 보안·런타임 | B persistence·성능 | C 대시보드·UX |
| --- | --- | --- | --- |
| 1주차 | W1-01 즉시 RED/GREEN | W0-00 font, W0-01 기준선, W0-02 | W1-04 asset/test 설계 |
| 2주차 | W1-02, W2-03A | W1-03, W3-01 preflight | W1-04, W1-05 |
| 3주차 | W2-01/02, W2-03B | W3-01 apply/readback | W5-01/02를 조기 구현 |
| 4주차 | W3-04 계약·failure fixture | W3-02, W3-03A/B | W5-01/02 상태 matrix 완료 |
| 5주차 | W4-01, W4-03 | W3-04 구현·cutover | W5-03 준비와 Playwright matrix |
| 6주차 | W4-04A/B/C, W4-06 | W4-02, W4-05 | W5-03/04 |
| 7주차 | 7일 observe-only 시작 | W6-01/02/03 준비 | W5-05/06 final cohort |
| 8주차 | 관찰·closure 지원 | W6-03 staging과 closure 뒤 final artifact | UX evidence와 W6-05 |
| 9~10주차 | duplicate/dependency contingency | W6-04 canary/soak | regression·접근성 재검증 |

기간 가정:

- 1인: 16~24주.
- 2인: 10~15주.
- 3인: 7~10주.
- 참가자 모집, 실제 Mongo duplicate repair, dependency compiler fallout는 범위 밖 지연으로 별도 보고한다.
- crates/dashboard/src/main.rs, dashboard.css, dashboard.js를 함께 바꾸는 작업은 동시에 병합하지 않고 C 소유 queue에서 순차 통합한다.

7일 관찰은 W4-01과 W4-03의 exact binary가 staging에 배포되고 hard cap/gauge가 활성화된 시점부터 센다. supervisor, Stock, cap 코드·설정이 바뀌면 clock을 reset한다. actual worker, permit, rejection, cancellation, provider error, p99 concurrency를 기록하고 W6-04 전에 완료한다.

## 13. 작업 추적표

| ID | 작업 | 우선순위 | 상태 | 선행조건 | 예상 |
| --- | --- | --- | --- | --- | --- |
| W0-00 | self-hosted deterministic Fira | P1 | 미착수 | 없음 | 1일 |
| W0-01 | 성능·Mongo·브라우저·UX 기준선 | P0 | 미착수 | W0-00 | 4~6일 |
| W0-02 | strict Clippy blocker | P1 | 미착수 | 없음 | 0.5일 |
| W1-01 | moderation/env tactical floor | P0 | 미착수 | 없음, focused RED부터 | 1~2일 |
| W1-02 | finite HTTP/Discord directory | P1 | 미착수 | W0-01 | 2~3일 |
| W1-03 | write-free settings read | P1 | 미착수 | W0-01 | 1일 |
| W1-04 | hashed CSS/JS + read-only smoke | P1 | 미착수 | W0-01 | 1~2일 |
| W1-05 | accessibility/responsive foundation | P1 | 미착수 | W1-04 | 2~3일 |
| W2-01 | current auth/mutation identity | P0 | 미착수 | W1-02/03 | 4~6일 |
| W2-02 | component/modal live policy | P1 | 미착수 | W1-03 | 1~2일 |
| W2-03A | guild-scoped effect 계약 | P1 | 미착수 | W1-01 | 1~2일 |
| W2-03B | 여섯 모듈 effect 전환 | P1 | 미착수 | W2-02/W2-03A | 2~3일 |
| W3-01 | Mongo index contract | P1 | 미착수 | W0-01 | 3~5일 + duplicate contingency |
| W3-02 | atomic stats | P1 | 미착수 | W3-01 | 2일 |
| W3-03A | giveaway/suggestion atomic adapter | P1 | 미착수 | W3-01 | 2~3일 |
| W3-03B | invite delta/audit retention | P1 | 미착수 | W2-01/W3-02 | 2~3일 |
| W3-04 | dashboard mutation durability | P0 | 미착수 | W2-01/W3-01/W3-03B | 5~8일 |
| W4-01 | WorkSupervisor | P1 | 미착수 | W0-01/W1-02 | 2~3일 |
| W4-02 | settings snapshot cache | P1 | 미착수 | W2-02/W3-04 | 2~4일 |
| W4-03 | Stock worker lifecycle | P1 | 미착수 | W2-02/W4-01 | 2~3일 |
| W4-04A | session/OAuth/GameInfo/retry bound | P1 | 미착수 | W1-02/W2-01/W4-01 | 2~3일 |
| W4-04B | Toss single-flight/cache | P1 | 미착수 | W4-04A | 1~2일 |
| W4-04C | stats/invite adapter + presence 재검증 | P1 | 미착수 | W1-02/W4-01/W4-02 | 2~3일 |
| W4-05 | giveaway edit coalescing | P2 | 미착수 | W3-03A/W4-01 | 1~2일 |
| W4-06 | suggestion effect ownership | P1 | 미착수 | W3-03A/W4-01 | 2~3일 |
| W5-01 | local/effective/blocker UI | P1 | 미착수 | W1-03/04 | 2일 |
| W5-02 | fail-closed read UI | P1 | 미착수 | W1-02/03, audit/sync close는 W3-04 | 2일 |
| W5-03 | authoritative save/reconcile | P0 | 미착수 | W1-04/W3-04/W4-02 | 4~6일 |
| W5-04 | dirty/confirm/Undo | P1 | 미착수 | W5-03 | 2~3일 |
| W5-05 | long list/lazy modal | P2 | 미착수 | W1-04/W2-01/W5-01/03/04 | 2~3일 |
| W5-06 | 재현 가능한 UI quality gate | Gate | 미착수 | W0-01/W5-01~05 | 3~5일 + 모집 대기 |
| W6-01 | dependency remediation | P1 | 미착수 | W5-06, audit는 병렬 가능 | 2~4일 + fallout |
| W6-02 | PM2 retention | P2 | 미착수 | W1-01 | 1일 |
| W6-03 | deterministic Pi artifact | P2 | 미착수 | W1-01/W6-01/02 | 2~3일 |
| W6-04 | exact-HEAD canary/rollback | Gate | 미착수 | W4 관찰/W5-06/W6-01~03/W6-05 GREEN | 1~2일 + soak |
| W6-05 | 36-finding closure matrix | Gate | 미착수 | W1~W6-03/W5-06/staging evidence | 1~2일 |

공수 원장 합계는 71~109 엔지니어일이다. 병렬화는 달력 기간만 줄이며 이 합계를 줄이지 않는다. Mongo duplicate repair, dependency fallout, 참가자 모집 대기는 합계 밖 contingency다.

## 14. 첫 delivery checkpoints

지원 checkpoint:

1. test(remediation): establish isolated performance, Mongo, browser, and UX baselines after deterministic fonts
2. fix(clippy): restore strict workspace lint baseline

제품 checkpoint:

1. fix(security): enforce moderation and environment safety floor
2. fix(dashboard): self-host deterministic Fira fonts
3. fix(dashboard): make guild settings reads side-effect free

W1-01과 W0-00은 제어-plane이나 전체 benchmark 완료를 기다리지 않고 refactor에서 즉시 병렬 시작할 수 있다. 세 제품 checkpoint의 병합 순서는 RED/GREEN 준비 상태에 따라 달라질 수 있지만, W0-01 측정은 W0-00 뒤에만 시작하고 대규모 authorization/persistence 변경 전에는 관련 기준선을 통과해야 한다.

## 15. 최종 완료 체크리스트

- [ ] W6-05 verifier가 정확히 36개 unique 보안 occurrence와 재현 가능한 증거를 확인
- [ ] stale guild grant와 retained component bypass 0
- [ ] Discord effect가 모두 guild-scoped이고 implicit mention 0
- [ ] Mongo 11개 index exact readback 및 hot query IXSCAN
- [ ] stats/giveaway/suggestion/invite lost update 0
- [ ] suggestion OutcomeUnknown에서 duplicate send 0
- [ ] actual worker/waiter/cache가 설정 cap 이하
- [ ] warm settings read 0, guild detail cold 1/warm 0
- [ ] local/effective/blocker UI가 정확함
- [ ] fabricated default와 false Saved 0
- [ ] 4개 viewport overflow 0
- [ ] keyboard/Narrator/focus/contrast 기준 통과
- [ ] isolated default smoke selected test 1개 이상, skip 0, outbound/mutation/Mongo write 0
- [ ] 서로 다른 baseline/final cohort 각 10명·50 trial에서 UX 기준 통과
- [ ] npm audit 0 및 dependency advisory 처리 완료
- [ ] deterministic candidate/rollback artifact hash 검증
- [ ] W4 exact binary 7일 관찰 통과
- [ ] 30분 canary와 24시간 soak 통과
- [ ] rollback이 보안 바닥과 원자·bounded 경계를 유지

## 16. 승인 후 바로 수행할 작업

1. 현재 refactor 브랜치를 유지하고 추가 control-plane 또는 하위 작업 브랜치를 만들지 않는다.
2. W1-01 moderation/env focused RED를 즉시 작성한다.
3. W0-00 deterministic font를 별도 파일 소유권으로 병렬 시작한다.
4. W0-00 GREEN 뒤 W0-01 isolated baseline을 시작하고, W1-01 GREEN과 secure-env 계약을 통과시켜 P0 제품 커밋을 만든다.
5. W1-03과 W1-04를 준비하고 작업 추적표에 실제 owner, 시작일, evidence path를 기록한다.

이 시점부터 진척률은 제어 스크립트 수가 아니라 완료된 제품 작업 패키지와 통과한 acceptance criterion으로 보고한다.
