# Dynamo 축소 작업 실행 계획서

갱신일: 2026-08-31

대상 브랜치: `refactor`

인계 기준: `c3d8bb9` (`docs(plan): reprioritize remaining remediation work`)

장기 backlog: [DYNAMO_IMPROVEMENT_WORK_PLAN.md](DYNAMO_IMPROVEMENT_WORK_PLAN.md)

## 1. 범위 결정

이번 iteration의 목적은 전면적인 재설계가 아니라 다음 두 가지다.

1. 운영 중 불필요한 작업과 무한 대기·잔류 worker를 줄이는 내부 최적화
2. 상태를 오해하지 않고 기본 조작이 편하도록 만드는 소규모 dashboard UI 개선

이 문서만 현재 실행 범위를 정의한다. 장기 backlog의 항목은 이 문서에 명시적으로 승격되지 않는 한 구현하지 않는다.

사람 대상 UX 연구, 전체 시스템 재설계, 정식 배포 체계 구축은 이번 범위가 아니다. UI 편의성은 사용자가 직접 확인하고 판단한다.

## 2. 현재 상태

### 2.1 완료된 기반

| 항목 | 증거 |
| --- | --- |
| deterministic self-hosted font | `609d3c9` |
| strict Clippy/locked CI | `99dff55`, `36fef09` |
| moderation hierarchy와 owner-only env | `0f6be8d` |
| isolated runner와 Public A/B baseline | `60bdf63`~`067b044` |
| 선택적 UX evidence 계약 | `edbf95c` |
| 대규모 계획의 비필수 UX gate 제거 | `c3d8bb9` |

Public A/B는 동일 revision·fixture·environment에서 budget GREEN, failed 0, repository/outbound counter 0, exit Job PID 0이었다. 이 결과를 다시 만들기 위해 runner를 추가 설계하지 않는다.

### 2.2 현재 완료 집계

기존 3/36 집계는 장기 backlog 진행률이며 이번 iteration의 목표 수가 아니다. 이번 축소 범위는 아래 6개 checkpoint로 새로 관리한다.

현재 상태: `0/6 완료`

## 3. 활성 작업

| 순서 | ID | 작업 | 중요도 | 예상 | 완료 핵심 |
| --- | --- | --- | --- | --- | --- |
| 1 | C1 / W1-03A | guild settings 조회의 DB write 제거 | 필수 | 1~2일 | GET write 0, absent/existing/unavailable 구분 |
| 2 | C2 / W1-02A | dashboard·Toss HTTP deadline 바닥 | 필수 | 1~2일 | connect/body/total timeout, 신규 retry 없음 |
| 3 | C3 / W2-01A | dashboard write의 현재 권한 재확인 | 보안 필수 | 2~3일 | stale grant에서 write 0 |
| 4 | C4 / W4-03A | Stock worker의 로컬 취소·정리 | 필수 | 1~2일 | map 제거 뒤 해당 worker effect 0 |
| 5 | C5 / W5-01·02A | 상태와 조회 오류를 사실대로 표시 | UI 핵심 | 2~3일 | local/effective/blocker 및 실패 상태 분리 |
| 6 | C6 | 작은 UI pass와 기존 회귀 검증 | UI·gate | 1~2일 | 국소 UI 개선, 기존 test GREEN, 사용자 확인 |

전체 예상은 8~14 엔지니어일이다. 이는 상한 약속이 아니라 범위 이탈을 감지하기 위한 기준이다. 한 checkpoint가 예상의 두 배를 요구하거나 공용 framework·새 crate·새 persistence protocol을 요구하면 구현을 중단하고 backlog로 돌린다.

## 4. checkpoint 상세

### C1. Write-free guild settings read

다음 세션의 첫 작업이다.

대상:

- `crates/repositories/src/lib.rs`
- `crates/persistence-api/src/lib.rs`
- `crates/persistence-mongo/src/lib.rs`
- `crates/settings/src/lib.rs`
- 필요한 dashboard 호출부와 focused test

계약:

- `GuildSettingsRepository::get(guild_id) -> Result<Option<GuildSettings>, Error>` 경계를 제공한다.
- Mongo GET은 `find_one`만 사용하고 upsert하지 않는다.
- absent는 표현용 default와 함께 “아직 저장되지 않음”으로 표시할 수 있다.
- unavailable은 absent로 위장하지 않는다.
- mutation 경로만 absent 상태에서 새 문서를 만들 수 있다.
- 기존 bot/module 소비자는 호환 helper를 유지해 전체 호출부를 함께 재설계하지 않는다.

완료:

- fake repository와 isolated Mongo에서 GET 전후 write/document count가 같다.
- absent, existing, unavailable focused test가 통과한다.
- 관련 package test와 workspace strict gate가 GREEN이다.

범위 제한:

- Mongo index 전면 정비를 함께 하지 않는다.
- dashboard save receipt나 cache layer를 만들지 않는다.

### C2. Bounded HTTP floor

계약:

- dashboard shared HTTP client에 connect와 request timeout을 둔다.
- Toss public operation은 token lock, limiter wait, 기존 401 retry 1회와 body read를 하나의 total deadline 안에 둔다.
- Discord GET retry가 이미 있다면 최대 1회와 동일 total deadline 안으로 제한한다.
- timeout, unavailable, not-found를 구분한다.

완료:

- delayed connect, delayed body, 기존 401 retry test가 bounded time 안에 끝난다.
- timeout 뒤 자동 mutation 또는 Discord effect 재전송이 없다.
- 정상 경로의 repository/outbound 수가 불필요하게 증가하지 않는다.

범위 제한:

- 범용 SingleFlight나 공용 cache framework를 만들지 않는다.
- Toss 전체 client 재작성은 하지 않는다.
- 신규 Discord directory abstraction, TTL cache와 일반 retry 정책을 만들지 않는다.

### C3. Current authorization only

W2-01에서 현재 iteration에 필요한 보안 부분만 수행한다.

계약:

- guild module PATCH, command PATCH와 guild command sync POST 직전에 해당 guild의 현재 권한을 재확인한다.
- session에 저장된 과거 guild 목록을 write 권한 근거로 사용하지 않는다.
- 권한 회수, timeout, Discord unavailable이면 fail closed로 write 0이다.
- upstream 401은 session을 폐기하고 재로그인을 요구하며 refresh-token 시스템을 새로 만들지 않는다.
- network await 동안 session write lock을 보유하지 않는다.
- 캐시는 권한 부여 근거가 아니다.

완료:

- stale session grant RED/GREEN fixture가 있다.
- unauthorized/timeout/unavailable에서 repository mutation과 Discord effect가 0이다.

명시적 제외:

- mutation request ID
- durable receipt/status API
- audit outbox
- acknowledgement-loss reconciliation

이 제외 항목은 장기 backlog의 W2-01B/W3-04/W5-03으로 남긴다.

### C4. Local Stock worker cancellation

계약:

- Stock map entry가 worker handle 또는 cancellation token을 함께 소유한다.
- replacement, eviction, removal된 worker는 cancel signal을 받고 이후 fetch/edit effect를 만들지 않는다.
- 재등록 시 이전 worker와 새 worker가 동시에 실행되지 않는다.

완료:

- add/remove/re-add focused test가 있다.
- cancel 뒤 fetch/edit effect가 0이다.

범위 제한:

- 범용 WorkSupervisor crate를 만들지 않는다.
- 다른 모듈 worker를 함께 이전하지 않는다.
- 16/4/2 hard cap, gauge, LRU와 process-wide shutdown join을 만들지 않는다.

### C5. Truthful dashboard state

W5-01과 W5-02의 guild/deployment read-side 정확성만 수행한다.

계약:

- local guild gate와 실제 effective state를 별도 표시한다.
- deployment, guild, unavailable blocker를 구분한다.
- absent, empty, unavailable을 같은 default 화면으로 축약하지 않는다.
- Discord presence는 Present, Missing, Unavailable을 구분한다.
- 실패 상태에는 다음 행동 또는 Retry를 제공한다.

완료:

- deployment=false/guild=true 고정 fixture가 있다.
- absent/existing/unavailable UI matrix가 있다.
- 내부 오류, URI, collection name과 secret 노출이 0이다.

명시적 제외:

- durable save reconciliation
- authoritative Saved/receipt와 outcome-unknown 처리
- Undo와 dirty-form protocol
- lazy modal과 긴 목록 재설계
- dashboard 전체 JavaScript 구조 변경

### C6. Small UI pass and existing regression gate

사용자가 실제 화면을 보고 판단할 수 있을 정도의 작은 개선만 한다.

허용:

- 375/768/1024/1440px에서 page horizontal overflow 제거
- visible focus, 자연스러운 tab order, 명확한 label과 accessible name
- loading, empty, error 상태 문구 정리
- 오류의 `role=alert` 또는 `aria-live`
- dialog Escape와 focus return
- 과도한 간격·정보 밀도·버튼 배치의 국소 수정

제외:

- 디자인 시스템 도입
- 전체 dashboard 재작성
- 대규모 component abstraction
- animation framework
- CSS/JavaScript asset pipeline 재설계
- 사람 대상 정량 UX 연구

자동 검증:

- 관련 Rust package test
- `cargo test --workspace --all-features --locked`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `npm run perf:test`
- focused render/handler assertion으로 local/effective/blocker, unavailable control 0, auth failure write 0 검증
- 기존 browser/static contract 유지
- 새 Playwright harness나 12-cell evidence runner가 필요해지면 이번 범위에서 제외

사용자 검토:

- 사용자가 dashboard 화면과 주요 동선을 직접 확인한다.
- 시각적·조작상 문제는 구체적인 후속 항목으로만 기록한다.
- 사용자 검토는 자동 보안·데이터 무결성 실패를 덮어쓸 수 없다.

전체 12-cell matrix, participant cohort, SEQ, timer와 final 비교는 수행하지 않는다.

## 5. 장기 backlog로 내린 작업

이번 iteration에서 구현하지 않는다.

- W1-04 전체 hashed CSS/JavaScript pipeline
- W1-02의 신규 Discord directory/cache/single-flight와 W1-05 전체 accessibility program
- W2-01B mutation identity/receipt와 W2-02 component/modal 중앙 gate
- W2-03A/B 전체 guild-scoped effect 타입 전환
- W3-01~04 Mongo index·원자 transition·audit·durable mutation
- W4-01/02/04/05/06 공용 supervisor, cache, retry, coalescing과 W4-03 hard cap/LRU/gauge
- W5-03~05 authoritative save, Undo, dirty form, lazy modal, 긴 목록 재설계
- W5-06 사람 대상 UX 연구
- R3 실제 12-cell Playwright evidence runner
- W6-01~05 dependency campaign, deterministic artifact, canary/soak, 36-finding closure
- 보안 deep scan 재개

이 항목들은 삭제하지 않고 [장기 개선 backlog](DYNAMO_IMPROVEMENT_WORK_PLAN.md)에 보존한다. 사용자 요청이나 현재 checkpoint의 직접적인 blocker가 아니면 승격하지 않는다.

## 6. 범위 통제 규칙

- 한 checkpoint는 하나의 제품 문제만 해결한다.
- “나중에 필요할 수 있음”을 이유로 공용 framework를 만들지 않는다.
- 기존 trait와 모듈 경계 안에서 해결할 수 있으면 새 crate를 만들지 않는다.
- unrelated cleanup, naming sweep, broad refactor를 같은 커밋에 넣지 않는다.
- 도구·runner 보강 자체를 제품 진척으로 계산하지 않는다.
- production/shared guild, DB, Discord, Toss에서 검증하지 않는다.
- deep scan과 사람 UX 연구는 명시적 재요청 없이는 시작하지 않는다.
- 각 checkpoint는 focused RED, 구현, GREEN, rollback 메모, 한 개의 제품 커밋으로 끝낸다.

## 7. 새 세션 인계

첫 확인:

```powershell
git branch --show-current
git status --short
git log -3 --oneline
rg -n "### C1|## 3\. 활성 작업" DYNAMO_REMAINING_WORK_PLAN.md
```

예상 상태:

- branch: `refactor`
- worktree: clean
- 최신 문서-only scope-cut 커밋이 HEAD
- 다음 작업: C1 / W1-03A write-free guild settings read

새 세션 첫 요청:

> `DYNAMO_REMAINING_WORK_PLAN.md`만 현재 실행 범위로 사용하고 C1/W1-03A write-free guild settings read를 focused RED부터 구현하라. 장기 backlog 작업은 승격하지 말라.

## 8. 완료 보고 형식

각 checkpoint 종료 시 다음만 기록한다.

- 변경한 제품 동작
- focused RED/GREEN test
- 관련 package/workspace gate 결과
- 보안·성능 counter 변화
- rollback 단위
- commit SHA
- 다음 checkpoint 또는 명확한 blocker

전체 iteration 완료는 C1~C6이 끝나고 사용자가 dashboard를 직접 확인했을 때 선언한다.
