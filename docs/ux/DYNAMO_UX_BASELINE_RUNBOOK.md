# Dynamo 대시보드 UX baseline 실행 runbook

작성일: 2026-08-31

대응 작업: W0-01A / R1B

상태: `HOLD — evidence schema/validator는 준비됨; deterministic study fixture, runner, 참가자 cohort는 아직 준비되지 않음`

## 1. 목적

이 runbook은 대시보드 UI 변경 전의 사용성을 10명·50 trial로 고정하기 위한 실행 계약이다. 자동 접근성·브라우저 테스트를 사람의 편의성 증거로 대체하지 않으며, 실제 참가자 없이 결과를 생성하지 않는다.

baseline은 현재 UI의 실패도 그대로 측정한다. 현재 UI에 기능이 없어 과업을 완료하지 못한 경우 성공으로 꾸미지 않고 `completed=false`로 기록한다. final 연구는 같은 과업·fixture·시간 상한을 사용한다.

## 2. 연구 시작 전 필수 preflight

다음 항목이 모두 GREEN이 되기 전에는 참가자를 모집하거나 trial을 시작하지 않는다.

- [ ] exact Git revision과 clean worktree를 기록했다.
- [ ] 비운영 deterministic fake dashboard가 다섯 과업의 현재 UI를 충실히 재현한다.
- [ ] trial마다 fixture를 같은 SHA-256 상태로 초기화할 수 있다.
- [ ] shared, staging, production guild/database를 사용하지 않는다.
- [ ] Discord, Toss, MongoDB, 외부 font/script로 나가는 요청을 모두 차단한다.
- [ ] 예상 route allowlist 밖의 요청은 즉시 실패한다.
- [ ] browser, version, viewport, zoom, OS scale, font-ready 조건을 고정했다.
- [ ] evaluator가 성공·실패 상태를 판정할 DOM/state oracle을 가진다.
- [ ] task 4의 response-loss와 task 5의 Undo를 deterministic하게 재현한다.
- [ ] fixture reset, process, port, temp cleanup을 독립 계약으로 검증했다.
- [ ] pseudonymous participant ID 외 개인정보를 기록하지 않는다.

### 2.1 현재 준비도 — 2026-08-31

| 과업 | 준비도 | 현재 코드 근거와 누락 |
| --- | --- | --- |
| T1 guild/module 찾기 | UI READY, 실행 경로 HOLD | selector guild filter와 module filter/modal은 존재하지만 승인된 human-study launcher가 없음 |
| T2 상태와 blocker 설명 | 조건부 READY | deployment/guild/effective 값은 보이나 명시적인 blocker 설명은 없어 현재 해석 난이도 baseline으로만 사용 가능 |
| T3 dirty close | HOLD | form/filter는 있으나 dirty tracking, close 거부/수락, committed seed reset이 없음 |
| T4 response loss | HOLD | durable request receipt/status API, outcome-unknown, duplicate-write oracle이 없음 |
| T5 영향 확인/Undo | HOLD | confirmation, Undo, previous presence/value restore와 mutation oracle이 없음 |

공통 실행 기반도 HOLD다.

- production runner는 현재 `Public+Load`만 허용한다.
- 기존 Playwright 12-cell runner는 자동 spec 전용이며 사람이 조작하는 headed session이 아니다.
- trial별 fresh context, fixture hash reset, mutable state restore, participant evidence recorder가 없다.
- 현재 ReadOnly transport는 모든 business write를 차단하므로 T3~T5 baseline을 실행할 수 없다.

연구 fixture는 미래 기능이 현재 이미 존재하는 것처럼 구현해서는 안 된다. 현재 UI와 현재 server semantics를 그대로 연결하고, response loss·network failure 같은 외부 조건만 deterministic transport가 주입해야 한다. baseline에서 없는 기능은 미완료로 기록한다.

R1B를 여는 최소 기술 선행물은 다음 네 가지다.

1. locked Chromium을 쓰는 격리 headed human-study runner
2. 현재 동작을 보존하는 mutable fake transport와 task별 write/response-loss oracle
3. trial별 start-state hash, reset, end-state readback
4. 50-trial schema, timer/help/SEQ/critical-error와 Latin-square 완전성을 검증하는 recorder/validator — schema와 fail-closed validator 완료, recorder 미완료

현재 준비된 증거 계약:

- JSON Schema: `tests/perf/dashboard-ux-study-schema-v1.json`
- authoritative cross-field validator: `scripts/perf/validate-dashboard-ux-study.cjs`
- 계약 테스트: `tests/perf/dashboard-ux-study-validator.test.cjs`
- 실행: `node scripts/perf/validate-dashboard-ux-study.cjs <evidence.json>`
- validator 출력은 aggregate/binding만 담는 safe summary이며 participant/trial raw record를 다시 내보내지 않는다.

권장 고정 환경은 Windows, repository-local Playwright 1.58.2, locked Chromium revision 1208/version 145.0.7632.6, 1440×900 viewport, zoom 100%, OS scale 100%다. 실제 환경이 다르면 baseline과 final에 동일한 값을 사용하고 manifest에 기록한다.

## 3. 참가자 구성

- 총 10명
- Dynamo 또는 유사 Discord 관리 도구를 월 1회 이하 사용하는 occasional 관리자 5명
- 주 1회 이상 사용하는 regular 관리자 5명
- Dynamo 구현·리뷰·테스트 참여자는 제외
- final cohort와 중복 금지
- 이름, Discord ID, guild ID, 이메일, 화면 녹화 원본을 repository에 저장하지 않음

participant ID는 baseline에서 `B-O01`~`B-O05`, `B-R01`~`B-R05`, final에서 `F-O01`~`F-O05`, `F-R01`~`F-R05` 형식의 연구용 pseudonym만 사용한다.

## 4. 고정 과업

각 과업의 제한 시간은 180초다. 제한 시간 또는 참가자의 중단 선언에 도달하면 `completed=false`로 종료한다. 미완료 trial의 비교용 시간은 `comparison_duration_ms=180000`으로 right-censor한다. 실제 `duration_ms`는 카드 공개부터 종료까지의 시간을 그대로 기록한다.

### T1. guild와 module control 찾기

참가자 문구:

> 지정된 guild를 찾아 `Stock` module control을 여세요.

성공 oracle:

- 정확한 fixture guild가 선택됨
- `Stock` module control 영역이 열림
- 잘못된 guild나 다른 module을 열지 않음

### T2. local gate, effective state, blocker 설명

참가자 문구:

> 현재 화면에서 이 기능이 이 guild에 설정되어 있는지, 실제로 동작하는지, 동작하지 않는다면 무엇이 막고 있는지 설명하세요.

fixture 조건:

- deployment=false
- guild=true

성공 oracle:

- local guild setting은 enabled라고 답함
- effective state는 disabled라고 답함
- deployment blocker를 원인으로 지목함
- 단순히 토글 색상만 보고 “동작 중”이라고 답하지 않음

### T3. filter, dirty close, committed value 복귀

참가자 문구:

> 지정 command 설정을 변경하세요. 닫기를 한 번 취소해 편집을 계속한 뒤, 다시 닫아 변경을 버리고 서버에 확정된 값으로 돌아가세요.

성공 oracle:

- 지정 command만 찾음
- dirty close에서 취소 후 편집값이 유지됨
- 다시 닫아 discard 후 committed 값으로 돌아감
- 의도하지 않은 write 0

### T4. response loss 뒤 outcome 확인

참가자 문구:

> 저장 응답이 사라졌습니다. 같은 변경을 다시 보내지 말고 방금 요청의 결과를 확인해 작업을 끝내세요.

성공 oracle:

- 최초 PATCH 정확히 1건
- 재전송 write 0
- 원래 request ID의 status 조회만 수행
- outcome을 success, failure 또는 outcome-unknown 중 하나로 정확히 설명

### T5. 영향 확인, 변경, Undo

참가자 문구:

> 배포 전체 영향을 확인한 뒤 지정 값을 변경하세요. 이후 방금 변경을 되돌려 시작 전 상태와 정확히 같게 복구하세요.

성공 oracle:

- confirmation 취소 시 write 0
- 확정 후 forward write 정확히 1
- Undo는 새 request ID의 write 정확히 1
- 최종 presence/value가 시작 상태와 동일

## 5. balanced Latin-square 배치

5개 과업의 first-order carryover를 균형화하기 위해 다음 10개 순서를 참가자에게 하나씩 배정한다. 각 ordered task pair는 전체 cohort에서 정확히 두 번 나타난다.

| Sequence | 과업 순서 |
| --- | --- |
| S01 | T1 → T2 → T5 → T3 → T4 |
| S02 | T4 → T3 → T5 → T2 → T1 |
| S03 | T2 → T3 → T1 → T4 → T5 |
| S04 | T5 → T4 → T1 → T3 → T2 |
| S05 | T3 → T4 → T2 → T5 → T1 |
| S06 | T1 → T5 → T2 → T4 → T3 |
| S07 | T4 → T5 → T3 → T1 → T2 |
| S08 | T2 → T1 → T3 → T5 → T4 |
| S09 | T5 → T1 → T4 → T2 → T3 |
| S10 | T3 → T2 → T4 → T1 → T5 |

experience group은 S01부터 S10까지 정확히 교차 배정한다. S01을 occasional로 시작하거나 regular로 시작하는 두 방향 중 하나를 연구 시작 전에 고정하고, 이후 sequence마다 반대 group을 배정한다.

## 6. 평가자 script

세션 시작 문구:

> 지금부터 관리 대시보드에서 다섯 과업을 수행합니다. 화면에 보이는 정보만 사용해 주세요. 막히면 도움을 요청할 수 있으며, 생각한 내용을 소리 내어 말해 주세요. 이것은 사용자를 평가하는 시험이 아니라 화면을 평가하는 조사입니다.

도움 규칙:

- 30초 동안 진전이 없고 참가자가 도움을 요청했을 때만 제공
- 제공 문구는 항상 다음 한 문장으로 고정

> 화면에서 현재 상태를 확인한 뒤, 다음 단계로 진행할 수 있는 버튼이나 링크를 다시 찾아보세요.

도움을 제공한 trial은 결과와 관계없이 `unassisted=false`다. 평가자는 control 위치를 가리키거나 정답·상태 의미·복구 절차를 설명하지 않는다.

SEQ 질문:

> 이 과업은 전반적으로 얼마나 쉬웠습니까? 1은 매우 어려움, 7은 매우 쉬움입니다.

완료·미완료와 관계없이 각 trial 직후 한 번만 묻는다.

## 7. trial 기록 schema

각 trial은 다음 필드를 가진다.

| 필드 | 계약 |
| --- | --- |
| `schema_version` | `1` |
| `study_phase` | `baseline` 또는 `final`; bundle과 모든 trial이 동일 |
| `study_revision` | 40자 Git SHA |
| `fixture_sha256` | 64자 lower-hex |
| `evaluator_script_sha256` | manifest의 평가자 script 64자 lower-hex digest |
| `browser_manifest_sha256` | 고정 browser manifest의 canonical JSON digest |
| `participant_id` | phase에 맞는 `B-(O|R)0[1-5]` 또는 `F-(O|R)0[1-5]` |
| `experience_group` | `occasional` 또는 `regular` |
| `sequence_id` | `S01`~`S10`, cohort 내 unique |
| `task_id` | `T1`~`T5` |
| `position` | 1~5 |
| `completed` | boolean |
| `unassisted` | boolean |
| `duration_ms` | 실제 경과 시간, 1~180000 |
| `comparison_duration_ms` | completed면 duration, 미완료면 180000 |
| `help_count` | 0 또는 1 |
| `critical_error` | 아래 닫힌 목록 또는 `none` |
| `seq` | integer 1~7 |
| `write_count` | fixture oracle의 write 수 |
| `outbound_count` | 반드시 0 |
| `start_state_hash` | trial reset 직후 fixture state digest |
| `end_state_hash` | fixture end state digest |

critical error 닫힌 목록:

- `none`
- `false_success`
- `wrong_scope`
- `duplicate_write`
- `unsafe_cleanup`
- `unrecovered_data_loss`

자유서술 관찰 메모는 participant ID와 task ID만 연결하고 개인정보·비밀값·guild 식별자를 포함하지 않는다.

JSON Schema는 형태와 단일 필드 범위를 고정한다. 최종 유효성은 validator의 교차 필드 계약으로 판정한다.

- 모든 critical error trial은 `completed=false`다.
- T1~T3은 write 0이며 end state가 start state와 같아야 한다.
- T4 완료는 write 정확히 1이다. write가 2 이상이면 `duplicate_write`, `completed=false`다.
- T5 완료는 forward+Undo write 정확히 2이고 end state가 start state와 같아야 한다. 복구되지 않은 state drift는 `unrecovered_data_loss`, `completed=false`다.

## 8. 세션 실행 절차

1. exact revision, clean status, fixture hash, evaluator script hash, browser/runtime version을 manifest에 기록한다.
2. 참가자에게 연구 설명과 개인정보 비수집 원칙을 알린다.
3. 참가자에게 배정된 sequence를 확인한다.
4. 각 trial 전에 fixture를 초기화하고 `start_state_hash`가 seed digest와 같은지 재확인한다.
5. 과업 카드를 공개하면서 timer를 시작한다.
6. 성공 oracle, 180초 제한 또는 중단 선언에서 timer를 종료한다.
7. write/outbound/end-state oracle을 읽고 참가자 답변과 함께 판정한다.
8. SEQ를 기록한다.
9. 다음 trial 전 process, port, temp, browser storage를 정리한다.
10. 50번째 trial 뒤 schema, sequence, task count, counter, 개인정보 부재를 검증한다.

## 9. baseline evidence 완료 조건

- 참가자 10명과 sequence 10개가 일대일 대응
- occasional 5명, regular 5명
- 참가자당 T1~T5 정확히 한 번
- 총 trial 정확히 50개
- ordered task pair 각각 정확히 두 번
- 누락·중복 trial 0
- fixture/revision/browser 조건 drift 0
- outbound 0
- raw secret, guild ID, participant PII 0
- 완료율, unassisted율, task별 median comparison duration, help, critical error, median SEQ를 산출
- evidence bundle은 승인된 immutable 위치에 보존하고 repository에는 schema version, digest, evidence URI만 기록

baseline 자체에는 성공률 합격선을 적용하지 않는다. 현재 UI의 실패를 사실대로 남기는 것이 목적이다. final 합격 기준은 `DYNAMO_REMAINING_WORK_PLAN.md`의 W5-06 계약을 따른다.

T3~T5에서 현재 기능이 없어 미완료가 되는 것은 유효한 baseline trial이다. launch/reset drift, 외부 요청, fixture mismatch, 평가자 script 위반만 trial invalid 사유다. 연구용 transport와 oracle은 현재 제품의 동작을 관찰할 뿐 status endpoint, confirmation, Undo UI 같은 미래 해법을 제공하지 않는다.

## 10. UI/UX 기준 적용 메모

UI/UX 검색 결과의 horizontal-scroll journey, floating CTA, landing-page 전환 패턴은 관리 대시보드와 과업 연구에 부적합해 채택하지 않는다. 다음 항목만 연구 관찰 기준에 포함한다.

- 오류가 다음 행동과 복구 경로를 제공하는가
- 오류가 시각 표현뿐 아니라 `role=alert` 또는 `aria-live`로 전달되는가
- keyboard tab 순서와 시각 순서가 일치하는가
- focus가 보이고 dialog 종료 뒤 원래 control로 돌아오는가
- 색상만으로 상태를 전달하지 않는가
- label과 accessible name이 명확한가
- reduced motion과 375/768/1024/1440 반응형 계약을 위반하지 않는가

이 항목의 자동 검증은 R3~R5가 소유한다. R1B 참가자 trial 결과와 혼합해 하나의 “접근성 점수”로 축약하지 않는다.
