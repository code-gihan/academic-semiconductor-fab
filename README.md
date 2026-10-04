# academic-semiconductor-fab

SMT2020 반도체 FAB 테스트베드(데이터셋 4종)를 정적 웹 페이지에서 사용자가 직접 실행하는 시뮬레이터. DES 엔진·이벤트 큐·루프·모델·운영 전략을 모두 Rust로 구현해 wasm으로 컴파일하고, 서버나 상용 시뮬레이터(AutoSched AP) 없이 클라이언트 브라우저에서 연산한다.

https://code-gihan.github.io/academic-semiconductor-fab/

현재: DES 코어(`fab-sim::des`)와 wasm 배포 골격 구현. 이하 구현 명세.

## 참고 문헌·데이터

- [P1] D. Kopp, M. Hassoun, A. Kalir, L. Mönch, "SMT2020—A Semiconductor Manufacturing Testbed," *IEEE Trans. Semicond. Manuf.*, 33(4), 522–531, 2020. doi:10.1109/TSM.2020.3001933
- [P2] D. Kopp, M. Hassoun, A. Kalir, L. Mönch, "Integrating Critical Queue Time Constraints into SMT2020 Simulation Models," *Proc. WSC 2020*, 1813–1824. doi:10.1109/WSC48552.2020.9383889
- 데이터: SMT2020 Testbed Release 1.0(2020-03), http://p2schedgen.fernuni-hagen.de/index.php?id=simulation&L=1 — `AutoSched/`(AutoSched AP 모델·실행 결과), `General Data/`(xlsx 일반 형식·명세). 출처 [P1] 표기. 변환 데이터 공개 전 재배포 조건 확인.

## 데이터셋

| | DS1 HV/LM | DS2 LV/HM | DS3 HV/LM+E | DS4 LV/HM+E |
|---|---|---|---|---|
| 제품(step 수) | part_3(583), part_4(343) | part_1–10(521, 529, 583, 343, 242, 293, 353, 375, 384, 390) | DS1 + part_E3 | DS2 + part_E1–E3 |
| 툴(툴그룹 105) | 1,043 | 913 | 1,135 | 1,068 |
| 생산 투입 | 주기형 `order.txt` | 목록형 `order_high_SL_92PCTL.txt`(167,129 lot) | 주기형 `order.txt` | 목록형 `order_high_SL_92PCTL.txt`(DS2와 투입 동일, 납기 상이) |
| 제품별 투입 간격 | 일반 51.69 min, hot 2,016 min | 일반 258.46 min, hot 10,080 min | DS1과 동일 | DS2와 동일 |
| super hot(part_3) | 27,397.61 min | 28,258.37 min | 27,397.61 min | 28,258.37 min |
| 엔지니어링 투입 | – | – | `E_order.txt`(16,960 lot, 주 40) | `E_order_high_SL_92PCTL.txt`(33,918 lot, 주 80, 3제품 동수) |
| 납기 | 고정 오프셋(DUE−START) | lot별 d = r + z·FF·RPT, z~U[0.8,1.2], FF = FCFS 92 백분위([P1] 식 (1)·Table II) | 생산 고정 오프셋, E 고정 47.84 d(hot 30.37 d) | lot별(식 (1)) |
| 디스패칭 3순위 | FIFO | CR | FIFO | CR |
| 초기 WIP(`WIP.txt`) | 2,256 lot | 2,156 lot | 2,489 lot | 2,697 lot |

- 공통: 10,000 WSPW(생산 400 lot/주, 25 wafer/lot, super hot 별도). 우선순위 일반 10, hot 20(생산 lot의 2.5%), super hot 30. 지연 스텝 전용 `Delay_32`(400대, 위치 Delay, load/unload 0) 별도.
- 엔지니어링(E) lot: 우선순위 10, hot 20(20%), wafer DU[1,10], 월·수 08:00에 절반씩 투입. E route = 원 route와 SETUP·WHEN(DS4는 STIME 포함)만 상이.
- 목록형 투입 범위: 2018-01-01–2025-12-31(E 목록 –2026-02-11).

## 원천 데이터: AutoSched `.asd`

기준 결과(AutoSched AP 11.3, 1,460일 1회 실행)의 실제 입력인 `.asd`(UTF-16LE TSV)를 변환 원본으로 사용한다. `General Data` xlsx는 다음 결함으로 시맨틱 참조에만 사용:

- route_3·E3 cascading 간격 30곳 오기(예: step 96 xlsx 7.1964, `.asd` 70.1964. `.asd`는 툴그룹별 c/p ≈ 0.70·0.75·0.85·0.90 일정).
- DS4 E lot ROUTE NAME이 생산 route(Route_Product_1–3)로 기재.
- 초기 WIP 없음, E 목록 2025-12-31 절단(`.asd`는 –2026-02-11), SuperHotLot SUPERHOTLOT=yes(`.asd` HOTLOT=no).
- 서식만 있는 빈 행(DS2·DS4 Toolgroups 1,048,364행 중 값 있는 행 107)으로 18·28 MB.

사용 파일(`options.def`의 `~` 접두 = 비활성, 제외): `options.def`(SIM_START, ORDER_FILES, SEQ_ADDS_SETUP_DELAYS=N), `part.txt`, `tool.txt`, `route_*.txt`, 활성 ORDER_FILES(`order.txt`·`order_high_SL_92PCTL.txt`·`E_order*.txt`·`WIP.txt`), `setup.txt`, `setupgrp.txt`, `downcal.txt`, `pmcal.txt`, `attach.txt`, `fromto.txt`, `period.txt`. IGNORE 열은 주석.

비활성 대체 입력: DS2 `order_medium_SL_50PCTL.txt`·`order_low_SL_25PCTL.txt`(납기만 다름), DS2·4 `order.txt`(주기형, 고정 납기), DS4 `E_order.txt`(납기만 다름). 기본 변환은 활성 파일, 납기 시나리오 비교 시 convert 인자로 지정.

## 모델 명세

문서에 없는 동작은 `(가정)`으로 표기한다. 검증으로 확인 후 수정한다.

### 시간·난수

- 시각 `i64` ms, 0 = 2018-01-01 00:00:00(SIM_START). 입력 min/hr/day·날짜를 ms 정수로 변환.
- 이벤트 순서 = (시각, 삽입 순번) → 결정적.
- 표본은 f64로 추출해 ms로 1회 반올림(누적 오차 없음). `U[m±h]` = PTIME±PTIME2 형식(평균±반폭), Exp(평균).
- 비율(CR·QTCR)은 f64 계산(i64 교차곱은 약 10^19로 overflow 가능).
- PRNG Xoshiro256++. 스트림 = (seed, 복제 번호, 용도: 공정시간·반송·고장·PM·샘플링·리워크·setup) → 전략 간 공통 난수(CRN).
- 네이티브·wasm 동일 결과: 초월함수 `libm` crate, 해시 순회 미사용.

### 투입

- 주기형(`order.txt`): START, REPEAT 간격, RPT#, LOTSPERRPT, 오프셋 DUE−START, PRIOR, PIECES.
- 목록형: lot별 START·DUE·PRIOR·PIECES.
- 초기 WIP: `WIP.txt` lot을 t=0에 CURSTEP 대기열에 투입. START = t0이므로 웜업 이후 통계만 유효.
- 부하 계수 ℓ(운영 곡선): 투입 시각 t → t/ℓ, 납기 오프셋 유지. 목록형은 목록 끝/ℓ ≥ 종료 시각 검증.

### 스텝 처리

순서(`CustomActl.txt`): 샘플링 → 반송 → 대기 → setup → load → 공정 → unload → 리워크 판정 → 다음 스텝.

- 샘플링: StepPercent p%로 수행. 미수행 시 즉시 다음 스텝(시간 0).
- 반송: `fromto.txt` 정의 쌍(이전 수행 스텝 위치 → 현 위치)만 적용, Fab→Fab U[5,10] min. 미정의(Delay 관련)·투입 직후 첫 스텝 0.
- load/unload: LTIME/ULTIME 1 min(`Delay_32` 0).
- 공정 시간 p ~ U[PTIME±PTIME2](±5%), lot(배치)·스텝당 1회 추출(가정). per_lot p, per_piece n·p(n = wafer 수), per_batch 배치당 p.
- cascading(STNCAP=2, 45 TG): wafer 단위 PartInterval(Implant 9, Wet_Etch 14), lot 단위 BatchInterval(Planar 6, TF 11, Dielectric 5). 툴당 동시 2 lot, 직렬 2슬롯. 단위는 슬롯1에서 c, 슬롯2에서 p−c(슬롯2 점유 시 대기). lot 첫 단위의 슬롯1 진입 = max(자기 load 종료, 선행 단위의 슬롯1 이탈). lot 완료 = 마지막 단위의 슬롯2 이탈 + unload. 단독 lot의 per_piece 시간 p + (n−1)c. load/unload는 다른 lot 공정과 병렬. c는 setup 미포함(SEQ_ADDS_SETUP_DELAYS=N).
- 배치(Diffusion 10 TG, per_batch): BATCHMN–BATCHMX wafer(75–100, 100–125, 125–150, 최소 = 최대 − 1 lot). 호환: DS1·2 동일 route·step(crit_sameroutestep), DS3·4 동일 PARTFAM·step 이름(생산·E 혼합, crit_samepartfam + crit_samestepname). 순위 순 후보마다 호환 lot을 순위 순으로 최대까지 채워 최소 이상이면 시작, 없으면 대기.
- setup: SETUP·WHEN(need = 툴 현재 setup과 다를 때, always = 매번). 시간 = route STIME(상수), 없으면 `setup.txt`(CURSETUP→NEWSETUP 우선, 빈 CURSETUP = 임의→NEWSETUP), SDIST constant/uniform(STIME±STIME2). 미정의 0, 툴 초기 setup 없음(가정). 수행 후 툴 setup = NEWSETUP. cascading 툴은 빈 상태에서만 setup(가정). rank_RSETUP용 시간은 평균값.
  - 데이터: DE_BE_13(1→2 7, 2→1 12 min)·DE_BE_66(15, 10 min) 순서의존. Implant gas SU128 72, SU132 60, SU91 80 min. Implant_119·90 route STIME. reticle LithoTrack_FE_115 8, FE_95 15 min. E lot 보정(always): FE_115 U[16,32], FE_95 U[30,60], 마지막 3개 litho U[52.5,127.5], Planar U[15,60] min.
- 리워크: REWORK r% 확률로 RWKSTEP로 돌아가 구간 재수행(lot 단위, 데이터상 litho → … → Litho_REG 3스텝).
- LTL 전용: SVESTN=yes·FORSTEP=j 스텝에서 쓴 툴만 스텝 j 수행 가능. 연쇄 적용(예: 13→113→351→362→390).
- CQT: STEP_CQT=e, CQT(h) = 본 스텝 종료 ~ e 시작 허용 대기. 기본 모델은 측정만 하고 강제하지 않음([P1]). DS2 구간 264개([P2] Table 1 대비 제품별 1개 적음, 논문 합 274). 중첩 없음, 연쇄(종료 = 다음 시작) 있음, 시작·종료 스텝 샘플링 100%.

### 디스패칭

- 순위 = `tool.txt` FWLRANK 순: rank_HP(우선순위 높은 순, hot lot이 setup 유발 가능) → rank_RSETUP(필요 setup 시간 짧은 순) → rank_FIFO(대기열 도착 순, DS1·3) 또는 rank_CR(작은 순, DS2·4). DS3·4는 LithoTrack_FE_95·115, Planar 6 TG에 rank_RSETUP 없음. 동률은 lot 번호.
- CR = (납기 − t) / 잔여 공정시간. 잔여 공정시간 = 현재 스텝부터 기대 스텝시간 합(load + 공정 + unload, 샘플링 확률 가중, 반송·리워크 제외). 스텝별 a + b·n 형태 접미합을 사전 계산.
- 자격 필터: LTL 전용 툴, setup run, super hot 예약, 전략 보류(Stopping·CAtE·CoT).
- rule_LSSU(Implant_128·132·91, setup 그룹 Implant_Gas, MINRUN 7): setup 변경 후 해당 setup으로 7 lot 처리 전 재변경 금지. 미완 run 중엔 setup 불필요 lot만 자격, 없으면 대기(가정).
- 툴 선택(유휴 툴 복수): wake_LeastSetupTime TG(DS1·2 9개, DS3·4 15개)는 setup 시간 최소. 그 외·동률은 유휴 최장(가정).
- super hot(우선순위 30, rule_HotLotFIRST): HOTLOT=yes면 현 스텝 공정 시작 시(HOTLOTDELAY%=0) 다음 스텝 TG 툴 1대 예약, 예약 툴은 도착까지 대기, setup은 도착 후. rule_LSSU TG 제외. `.asd`는 전부 HOTLOT=no라 기본 비활성, 옵션으로 활성(xlsx 시맨틱).

### 가용성

- UDT(`downcal`·`attach`, 영역 11개): TTF·첫 TTF Exp(10,080 min), TTR Exp(MTTR min: Def_Met·Litho_Met·TF_Met 35.28, Diffusion 151.2, Planar 201.6, Wet_Etch 221.76, Dry_Etch 231.84, TF 453.6, Dielectric·Implant 604.8, Litho 705.59). 달력 기준. 공정 중 고장 시 중단 후 수리 뒤 재개(가정).
- SDT(`pmcal`·`attach`, TG별 292개 = MN 105, QT 105, WK 82). 시간형 79(Def_Met·Diffusion·Litho_Met·Litho·TF_Met: MN 30 d, QT 91 d, Litho WK 7 d)은 시작 간격 = MTBPM(소요 포함). 카운터형 213(그 외)은 툴별 처리 wafer 수가 MTBPM에 도달하면 시작. 소요 U[MTTR±MTTR2] h. 첫 PM은 TG 내 k번째 툴(k = 1..N)에 FOA·k/N(일 또는 wafer). 진행 중 공정 완료 후 시작하고 대기 중 신규 착수 금지(가정). 동시 도래 PM은 순차 수행.
- 정지 구간이 겹치면 모두 끝나야 가용. `Delay_32`는 정지 없음.

### 통계

- 기간: WarmUp(2018, 종료 시 초기화) + 연도별 누적 Period_1–7(`period.txt`).
- lot: TH, CT 평균·표준편차·분위수, ONTIME%(완료 ≤ 납기), FF = CT/RPT. 제품 × 유형(PRL·PHL·super hot·ERL·EHL)별.
- RPT = 빈 fab 기대 CT = Σ 샘플링 가중(스텝시간 + 반송) + 리워크 기대분. [P1] Table II 대비 −0.1 ~ +1.1%(10제품 확인).
- WIP 시간가중 평균.
- 툴·TG·영역: DOWN·PM·SETUP·LOAD·UNLOAD·PROC·IDLE %, UTIL = SETUP + LOAD + UNLOAD + PROC, 가용도 = 100 − DOWN − PM, SDT 비중 = PM/(DOWN + PM)(`stnfam.rep` 정의와 동일).
- CQT([P2]): %VL(위반 / 구간 완료), %VL1h·2h·4h, AVL·AONT(h, 구간 완료 전체 평균 위반·여유 시간). Litho(LithoTrack_FE_95·115 포함 구간)·Rest·Total.
- 스텝 추적은 저장하지 않고 실행 중 집계(4년 lot-step 3,000만 건 이상 → 저장 시 GB 규모). 분위수용으로 lot별 CT만 보관.

## 운영 전략

| 전략 | 출처 | 내용 |
|---|---|---|
| BASE | P1·P2 | 데이터 순위(HP → RSETUP → FIFO/CR) |
| QTCR | P2 식 (1) | 순위 HP → RSETUP → QTCR → CR. d^Q = C_s + CQT. t ≤ d^Q면 (d^Q − t)/Σ_{k=i..n} p_k, 아니면 (d^Q − t)·Σ_{k=i..n} p_k. 작을수록 우선, 구간 밖 lot = +∞ |
| QTS | P2 식 (2)–(6) | QTCR 자리에 d_i. TW_k = (FF_k − 1)p_k, TT = Σ_{k=s+1..n−1} FF_k·p_k + TW_n, Ratio_k = FF_k·p_k/TT(k < n), TW_n/TT(k = n), FCQT_k = CQT·Ratio_k, d_i = C_s + Σ_{k=s+1..i} FCQT_k − p_i(i < n), d_n = C_s + CQT. 스텝별 FF_k = 평균 스텝 CT/p_k(사전 BASE 장기 실행으로 산출) |
| Stopping | P2 §3.2·Table 3 | TG별 임계 ①TG 앞 CQT lot(공정 중 포함) ②① + 그 스텝 이전 구간 내 CQT lot. 구간의 TG 중 하나라도 도달하면 구간 시작 스텝에서 보류. 직전 구간 종료 = 현 구간 시작이면 무시. BASE·QTCR·QTS와 결합. 임계(①/②) LithoTrack_FE_95: none 1000/1000, high 90/130, medium 60/95, small 50/85. FE_115: 1000/1000, 90/220, 70/150, 55/125. 그 외 1000/1000 |
| EF | P1 §V | 우선순위 EHL 25, PHL 20, ERL 15, PRL 10(전 TG) |
| CAtE | P1 §V | LithoTrack_FE_95·115만. 생산·엔지니어링 구간 (lp, le) h 교대, t=0 생산 구간부터(가정). DS3 (151.2, 16.8)·(75.6, 8.4)·(21.6, 2.4), DS4 (134.6, 33.4)·(67.2, 16.8)·(19.2, 4.8). 구간 유형 lot만, 없으면 다른 유형 |
| CoT | P1 §V | LithoTrack_FE_95·115만. 대기 EL ≥ 한계(100·50·25·10)면 그 수만큼 EL 우선. 그 외 PL 우선, PL 없으면 EL(가정) |

- QTCR·QTS의 p_k = CR과 같은 기대 스텝시간.
- [P2] complex CQT(441 구간)는 추가 구간의 CQT 값이 미공개라 재현 불가, default만 재현.

## 실험·검증

| 실험 | 데이터 | 조건 | 비교 대상 |
|---|---|---|---|
| 기준 실행 | DS1–4 | 1,460 d(2018–2021), 웜업 1년, AutoSched 1회 | `.rep` Period_3 누적: part·order(LOTCOMPS, CYCLEAVG·STD, ONTIME%), stnfam·stngrp(DOWN·PM·SETUP·PROC·UTIL %), perf(WIPLOTAVG) |
| 운영 곡선 | DS1·2 | 8년(웜업 1년), 1회, 부하 50–100%(용량 10,200·10,250 WSPW) | [P1] Fig. 2·3(일반 lot FF 분위수): 10,000 WSPW에서 중앙값 1.85·1.95, 5–95% 1.73–2.01·1.79–2.13, 최대 2.37·2.46. Table III·IV(영역 가용도·SDT 비중·평균/최대 가동률) |
| 엔지니어링 전략 | DS3·4 | 2년(웜업 1년), 20회. BASE, EF, CAtE 3종, CoT 4종 | [P1] Fig. 4·5(유형별 ACT), 부록 Table AII-1–4(유형별 TH·ACT·%ONTIME) |
| CQT | DS2 | 2년(웜업 1년), 10회, 초기 WIP. BASE·QTCR·QTS(+Stopping) | [P2] Table 4·5(default) |

- AutoSched XTHEOR는 내부 이론 CT 기준이라 FF와 직접 비교하지 않음(CT·TH·ONTIME·가동률로 비교).
- 난수 생성기가 달라(AutoSched CMRG) 경로 일치는 불가. 복제 평균과 95% 신뢰구간으로 비교한다.

## 성능 측정

- 지표: 이벤트/초, 복제 1회 벽시계 시간, 최대 메모리. 네이티브(`cli`)와 wasm(브라우저)을 같은 코드·입력으로 비교.
- 참고 기준(하드웨어 상이): AutoSched AP 1,460 d 1회 — DS1 36:02(lot-step 34.69 M), DS2 31:30(30.01 M), DS3 40:48(39.04 M), DS4 39:58(38.15 M).

## 구현 구조

```text
crates/sim/   코어 lib(패키지 fab-sim, std): des(DES 코어), 모델 타입(serde), 전략, 통계. wasm 의존 없음
crates/cli/   네이티브: convert(.asd → .bin), run(검증·벤치마크)
crates/wasm/  wasm-bindgen cdylib(패키지 fab-wasm): load(bytes), run(config) → 결과
www/          index.html, main.js, worker.js, data/ds1–4.bin(변환 결과, 커밋), pkg/(빌드 산출)
data/raw/     SMT2020 원본(커밋 제외)
```

- DES 코어(`des`, 구현됨): 사건 스케줄링 관점, 다음 사건 시각으로 시계 진행.
  - `Model`: 상태 + 초기화 루틴 `init`(t=0, 1회) + 사건 루틴 `handle`.
  - `Scheduler`: 시계 `now`, `schedule_at`·`schedule_in`. 과거 시각 예약은 panic(인과성 위반).
  - `Simulation`: `run_until(end)` = end 이하 사건 전부(처리 중 예약분 포함) 처리 후 시계 = end, 연속 호출로 이어서 실행. `events_processed` 집계.
  - 미래 사건 목록: (시각, 예약 순번) 최소 힙(`BinaryHeap`, 동시각 FIFO, 페이로드 비교 없음).
- 도메인 엔진: 엔티티 `Vec` + `u32` id(lot·툴·TG·스텝, route는 평탄 배열), `Event`는 작은 enum. 고장 중단 시 툴 epoch를 올려 기존 사건을 무효화(lazy deletion)하고 재스케줄.
- 대기열: TG별 `Vec`. 디스패칭은 순위 키 선형 스캔(대기열 수십~수백). 유휴 툴은 TG별 FIFO.
- 전략: `enum` + `match`(고정 집합, 동적 디스패치 없음).
- 데이터 파일: postcard 직렬화 + 포맷 버전 필드. 주기형 투입은 규칙만, 목록형·WIP는 lot 레코드(DS2·4 약 20만 lot). 브라우저는 xlsx를 읽지 않는다.
- 메모리: 모델 1 MB 미만, 동시 WIP 약 2,000–2,800 lot, 대기 이벤트 수천 → 작업 집합 수십 MB 이내(추정).
- 웹: 복제 1회 = Web Worker 1개(`navigator.hardwareConcurrency`만큼 병렬). SharedArrayBuffer 미사용(GitHub Pages는 COOP/COEP 헤더 설정 불가). 경계 입출력은 serde-wasm-bindgen, `i64`는 경계에서 f64(2^53 ms까지 정확).
- 의존성: serde, postcard, rand_xoshiro, libm, wasm-bindgen, serde-wasm-bindgen.

## 로컬 빌드·테스트

```bash
cargo test
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129   # crates/wasm/Cargo.toml의 wasm-bindgen 버전과 같아야 함
cargo build --release --target wasm32-unknown-unknown -p fab-wasm
wasm-bindgen --target web --no-typescript --out-dir www/pkg target/wasm32-unknown-unknown/release/fab_wasm.wasm
```

`www/`를 정적 서버로 열어 확인한다(예: `python -m http.server -d www`). `file://`로 열면 ES 모듈이 막힌다.

## 배포

`main`에 push하면 `.github/workflows/pages.yml`이 테스트·빌드 후 GitHub Pages로 배포한다.
