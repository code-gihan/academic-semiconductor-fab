# academic-semiconductor-fab

SMT2020 반도체 FAB 테스트베드(데이터셋 4종) 시뮬레이터. DES 엔진·이벤트 큐·루프·모델·운영 전략을 Rust 라이브러리(`smt2020`) 하나로 구현하고, 같은 코어를 웹 페이지(wasm, 클라이언트 브라우저에서 연산)·JavaScript 모듈·Python 패키지·네이티브 CLI로 포장한다. 서버나 상용 시뮬레이터(AutoSched AP)가 필요 없다.

https://code-gihan.github.io/academic-semiconductor-fab/

현재: 구현 단계 1–11 완료, 12–14 진행 예정([구현 단계](#구현-단계)). 이하 사용법과 구현 명세. 엔진 구동 원리·상태 전이·메커니즘 상세는 [SIMULATION.md](SIMULATION.md).

## Quick Start

### 웹 페이지

영어 기본, 한국어 전환(선택은 브라우저에 저장). 화면은 상단 탭의 보기 4개(설정·실행·분석·Python, 주소 `#setup`·`#run`·`#analysis`·`#python`)로 나뉜다.

1. 설정: 왼쪽에 데이터셋(DS1–4 동봉, 또는 `smt2020 convert`로 만든 로컬 `.bin`)과 실행 설정(종료 시각, 웜업(비우면 데이터셋 기간), 복제 수, seed, 부하 계수), 오른쪽에 운영 전략.
   - CQT 디스패칭(논문 QTCR·QTS, 자체 순위 없는 툴그룹에 적용).
   - 툴그룹별 lot 순위: 기준 조합(최대 6개, 순서 변경, `qt_within`은 시간 입력)을 만들어 체크한 툴그룹에 적용. 표는 CQT 구간 툴그룹만·영역·이름으로 거르고, 툴그룹의 현재 순위(데이터셋 또는 자체)를 보여 주며 클릭하면 조합으로 불러온다.
   - 배치 조기 시작(CQT 여유 h), Stopping(CQT 구간 툴그룹별 한도·기본값, [P2] Table 3 프리셋), 엔지니어링 lot(EF·CAtE 구간·CoT 임계 직접 입력과 [P1] 값, 엔지니어링 lot 없는 데이터셋은 안내), super hot 예약.
   - 설정 JSON: 보기·복사·붙여넣어 적용(Python·CLI와 같은 스키마). 적용·실행 전에 코어가 설정을 검증해 오류를 보여 준다.
   - 넓은 화면은 왼쪽 열이 고정되고, 좁은 화면은 1열(전략 뒤에 실행 버튼, 아래에 고정).
2. 진행: 복제는 워커 풀(Web Worker 최대 `navigator.hardwareConcurrency`개)이 하나씩 실행한다. 전체(완료 복제, 경과·남은 시간(진행률 비례 추정))와 복제별(대기, 모의 일자(Drain 잔여 WIP, QTS 사전·본 실행), 완료 시간) 진행 막대. FAB 지도: 따라가는 복제(처음 진행을 보고한 복제, 완료되면 실행 중인 다른 복제, 복제 클릭으로 변경)의 투입·완료 lot·WIP와 영역별 툴그룹 타일(채움 = 작업 중 툴 비율, 위쪽 막대 = 고장·PM 툴 비율, 숫자 = 대기 lot, 툴팁 = 상태별 툴 수). 갱신은 약 250 ms(벽시계)마다. 실행 중 설정 잠금(실행 탭 표시), 취소는 워커 종료. 실행이 끝나면 분석 보기로 넘어간다.
3. 분석: 값은 복제 평균 ± 95% 신뢰구간.
   - 개요(보고 기간별): 실행 설정, 핵심 지표(투입·완료 lot, 평균 WIP, PRL ACT·ONTIME, ERL ACT, CQT %VL), 차트(lot 유형별 ACT, CQT %VL, 구간별 %VL(위반 수 순, 상위 15개 또는 전체), 선택 구간의 스텝별 반송·대기·공정 시간(위반·충족), 일별 추이(%VL·WIP·완료, 신뢰구간 띠), 영역 가동률, 툴그룹 상태별 시간), 표 6종, 복제별 digest·성능. JSON(CLI `--json`과 같은 형식)·CSV로 내려받는다.
   - 상세(복제 1개): 복제 0은 실행 중 기록하고, 다른 복제는 같은 설정으로 재생해 기록한다(digest가 다르면 오류). 구간 × 일(120 d 초과는 주) 위반 히트맵(칸 = 목록 필터), 위반 목록(구간·lot 유형·제품·기간 필터, 초과·시각 정렬, 쪽 단위, CSV), 툴그룹 일별 대기 lot·툴 시간 비율(두 차트 커서·확대 연동), 툴그룹 구간 사건(최대 7 d, 툴별 작업·고장·PM 타임라인과 도착, 목록·CSV), lot 이력(투입부터 위반까지 스텝별 반송·대기·공정, 구간 확대, 가장 오래 기다린 툴그룹의 그때 사건으로 이동). 개요에서 고른 구간이 상세 필터가 된다.
4. Python 패키지: 페이지의 pip 명령·플랫폼별 wheel.

### Python

Python 3.9 이상, Windows x64·Linux x86-64·macOS(Intel, Apple silicon). wheel에 DS1–4 데이터셋 포함.

```bash
pip install smt2020 --no-index --find-links https://code-gihan.github.io/academic-semiconductor-fab/python/
```

```python
import smt2020
from smt2020 import DAY

dataset = smt2020.load_dataset("ds2")  # 동봉 ds1–ds4, 데이터셋 파일 경로 또는 AutoSched 모델 디렉터리
simulation = smt2020.Simulation(dataset, {"horizon": 730 * DAY, "queue_time": "qtcr"})  # 시각 0

simulation.run(until=100 * DAY)  # 100일에 일시정지
simulation.progress()  # pass, passes, now, horizon, released, completed, wip, finished
simulation.lots()  # 재공 lot: id, part, kind, step, tool_group, state, tool, cqt_deadline …
simulation.tools()  # 툴: id, tool_group, state, setup, lots
simulation.tool_groups()  # 툴그룹: queue, 상태별 툴 수

simulation.run(on_progress=lambda progress: progress["wip"] < 2500)  # 매일 호출, False면 일시정지
simulation.run()  # 끝까지: 투입 lot 전량 완료
results = simulation.results()
summary = smt2020.summarize([results])  # 측정값별 평균·95% 신뢰구간
simulation.reset({**simulation.config(), "queue_time": "qts"})  # 다른 전략으로 시각 0부터

# 같은 실행을 기록과 함께(digest 동일): 위반 lot, 툴그룹 일별 상태
recorded = smt2020.Simulation(dataset, {"horizon": 730 * DAY, "queue_time": "qtcr"},
                              {"violations": True, "tool_groups": True})
recorded.run()
violations = recorded.records()["violations"]  # 열: lot, segment, entered, arrived, exit …
segments = dataset.info()["segments"]  # violations["segment"]의 index 대상
```

- 실행 중 GIL을 놓으므로 스레드마다 `Simulation`을 두면 복제가 병렬로 실행된다. Ctrl-C는 실행을 일시정지하고 `KeyboardInterrupt`를 발생시킨다.

```python
from concurrent.futures import ThreadPoolExecutor

def replicate(replication):
    simulation = smt2020.Simulation(dataset, {"horizon": 730 * DAY, "replication": replication})
    simulation.run()
    return simulation.results()

with ThreadPoolExecutor() as pool:
    summary = smt2020.summarize(list(pool.map(replicate, range(10))))
```

### JavaScript

```js
// module worker: run()은 진행하는 동안 스레드를 점유한다.
import init, { Dataset, Simulation, summarize, csv, digest } from "./pkg/fab_wasm.js";

await init();
const dataset = new Dataset(new Uint8Array(await (await fetch("data/ds2.bin")).arrayBuffer()));
const DAY = 86_400_000; // 시간 단위 ms
const simulation = new Simulation(dataset, { horizon: 730 * DAY, queue_time: "qtcr" });
simulation.run(100 * DAY); // 100일에 일시정지
const queues = simulation.toolGroups().map((group) => [group.name, group.queue]);
simulation.run(undefined, (progress) => postMessage(progress)); // 매일 호출, false면 일시정지
const results = simulation.results();
const table = csv(summarize([results])); // 측정값별 평균·95% 신뢰구간의 CSV
const fingerprint = digest(results); // 같으면 결과가 비트 단위로 같다
simulation.free(); // wasm 메모리 해제
```

- 복제는 `replication`만 다르게 워커마다 실행하고 결과 배열을 `summarize`·`csv`에 넘긴다(`www/main.js`·`www/worker.js`).
- 데이터셋 파일: 배포 페이지의 `data/ds1.bin`–`ds4.bin`(다른 출처에서도 fetch 가능, 예: `https://code-gihan.github.io/academic-semiconductor-fab/data/ds2.bin`) 또는 `smt2020 convert` 출력.
- Node.js: `initSync({ module: readFileSync("www/pkg/fab_wasm_bg.wasm") })` 후 같은 API.

### Rust

```rust
use std::sync::Arc;
use smt2020::{Config, DAY, Dataset, Simulation};

let dataset = Arc::new(Dataset::from_bytes(&std::fs::read("www/data/ds2.bin")?)?);
let mut simulation = Simulation::new(dataset, Config::new(730 * DAY))?;
simulation.run(Some(100 * DAY))?; // 100일에 일시정지
let queued = simulation.tool_groups().iter().map(|group| group.queue).sum::<usize>();
simulation.run(None)?; // 끝까지
let summary = smt2020::report::summarize(&[simulation.results()?]);
```

### CLI

```bash
cargo build --release -p smt2020-cli
target/release/smt2020 convert "data/raw/AutoSched/dataset 2/LVHM_Model/LVHM_Model.asd" www/data/ds2.bin
target/release/smt2020 run www/data/ds2.bin --replications 10 --queue-time qtcr --json ds2.json --csv ds2.csv
target/release/smt2020 run www/data/ds2.bin --config config.json
target/release/smt2020 validate "data/raw/AutoSched/dataset 2/LVHM_Model"
```

- `run`: 데이터셋 파일 또는 `.asd` 디렉터리. 설정은 `--config`(JSON, [실행 설정](#실행-설정)) 위에 옵션을 덮어쓴다: `--horizon` 일, `--warm-up` 일, `--seed`, `--load`, `--reserve-super-hot`, `--queue-time none|qtcr|qts`, `--batch-start-within` 시간, `--stopping TG=FRONT/TOTAL`(반복), `--stopping-default FRONT/TOTAL`, `--engineering base|engineering_first|cate:생산h/엔지니어링h|cot:N`. 기준 목록(`ranking`)은 `--config`로. `--replications N`(설정의 replication부터 번호), `--threads`(기본 가용 코어), `--period`(표 기간), `--json`·`--csv` 출력.
- 웹 JSON의 `replications[i].config`를 `--config`로 실행하면 digest가 같다(결정성 확인).

## 참고 문헌·데이터

- [P1] D. Kopp, M. Hassoun, A. Kalir, L. Mönch, "SMT2020—A Semiconductor Manufacturing Testbed," *IEEE Trans. Semicond. Manuf.*, 33(4), 522–531, 2020. doi:10.1109/TSM.2020.3001933
- [P2] D. Kopp, M. Hassoun, A. Kalir, L. Mönch, "Integrating Critical Queue Time Constraints into SMT2020 Simulation Models," *Proc. WSC 2020*, 1813–1824. doi:10.1109/WSC48552.2020.9383889
- 데이터: SMT2020 Testbed Release 1.0(2020-03), https://p2schedgen.fernuni-hagen.de/downloads/simulation (논문의 `index.php?id=simulation` 주소는 이전됨) — `AutoSched/`(AutoSched AP 모델·실행 결과), `General Data/`(xlsx 일반 형식·명세). 출처 [P1] 표기.
- 재배포: 배포처에 이용 조건 문구 없음. 변환 데이터셋 파일(`www/data/ds1–4.bin`)을 출처([P1], 배포처) 표기와 함께 커밋·배포한다.

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

비활성 대체 입력: DS2 `order_medium_SL_50PCTL.txt`·`order_low_SL_25PCTL.txt`(납기만 다름), DS2·4 `order.txt`(주기형, 고정 납기), DS4 `E_order.txt`(납기만 다름). 기본 변환은 활성 파일, 대체 입력은 `convert --orders 파일…`로 지정(초기 WIP를 쓰려면 `WIP.txt`도 지정).

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
- 부하 계수 ℓ(운영 곡선): 투입 시각 t → t/ℓ, 납기 오프셋 유지. 목록형은 목록이 종료 시각까지 이어지는지 검증(마지막 투입 + 평균 간격 ≥ 종료 시각).
- 종료 시각 전에 시작하는 lot만 투입하고, 이후 WIP가 0이 될 때까지 진행(Drain). 마지막 lot 완료 사건에서 실행을 멈추고, 종료 + 365 d 기한 사건까지 남으면 오류.

### 스텝 처리

순서(`CustomActl.txt`): 샘플링 → 반송 → 대기 → setup → load → 공정 → unload → 리워크 판정 → 다음 스텝.

- 샘플링: StepPercent p%로 수행. 미수행 시 즉시 다음 스텝(시간 0).
- 반송: `fromto.txt` 정의 쌍(이전 수행 스텝 위치 → 현 위치)만 적용, Fab→Fab U[5,10] min. 미정의(Delay 관련)·투입 직후 첫 스텝 0.
- load/unload: LTIME/ULTIME 1 min(`Delay_32` 0).
- 공정 시간 p ~ U[PTIME±PTIME2](±5%), lot(배치)·스텝당 1회 추출(가정). per_lot p, per_piece n·p(n = wafer 수), per_batch 배치당 p.
- cascading(STNCAP=2, 45 TG): wafer 단위 PartInterval(Implant 9, Wet_Etch 14), lot 단위 BatchInterval(Planar 6, TF 11, Dielectric 5). 툴당 동시 2 lot, 직렬 2슬롯. 단위는 슬롯1에서 c, 슬롯2에서 p−c(슬롯2 점유 시 대기). lot 첫 단위의 슬롯1 진입 = max(자기 load 종료, 선행 단위의 슬롯1 이탈). lot 완료 = 마지막 단위의 슬롯2 이탈 + unload. 단독 lot의 per_piece 시간 p + (n−1)c. setup·load·unload는 다른 lot 공정과 병렬(SEQ_ADDS_SETUP_DELAYS=N, 가정). c는 setup 미포함.
- 배치(Diffusion 10 TG, per_batch): BATCHMN–BATCHMX wafer(75–100, 100–125, 125–150, 최소 = 최대 − 1 lot). 호환: DS1·2 동일 route·step(crit_sameroutestep), DS3·4 동일 PARTFAM·step 이름(생산·E 혼합, crit_samepartfam + crit_samestepname). 순위 순 후보마다 호환 lot을 순위 순으로 최대까지 채워 최소 이상이면 시작, 없으면 대기. 호환 lot이 더 올 수 없으면(잔여 투입 없음, 앞 스텝·이동 중 lot 없음) 최소 미만도 시작(가정).
- setup: SETUP·WHEN(need = 툴 현재 setup과 다를 때, always = 매번). 시간 = route STIME(상수), 없으면 `setup.txt`(CURSETUP→NEWSETUP 우선, 빈 CURSETUP = 임의→NEWSETUP), SDIST constant/uniform(STIME±STIME2). 미정의 0, 툴 초기 setup 없음(가정). 수행 후 툴 setup = NEWSETUP. rank_RSETUP 순위 시간 = `setup.txt` 평균(route STIME만 있는 setup은 0), wake_LeastSetupTime은 STIME 포함 평균(가정, 기준 결과의 litho·Implant_119·90 setup 비중 근거).
  - 데이터: DE_BE_13(1→2 7, 2→1 12 min)·DE_BE_66(15, 10 min) 순서의존. Implant gas SU128 72, SU132 60, SU91 80 min. Implant_119·90 route STIME. reticle LithoTrack_FE_115 8, FE_95 15 min. E lot 보정(always): FE_115 U[16,32], FE_95 U[30,60], 마지막 3개 litho U[52.5,127.5], Planar U[15,60] min.
- 리워크: REWORK r% 확률로 RWKSTEP로 돌아가 구간 재수행(lot 단위, 데이터상 litho → … → Litho_REG 3스텝).
- LTL 전용: SVESTN=yes·FORSTEP=j 스텝에서 쓴 툴만 스텝 j 수행 가능. 연쇄 적용(예: 13→113→351→362→390).
- CQT: STEP_CQT=e, CQT(h) = 본 스텝 종료 ~ e 시작 허용 대기. 기본 모델은 측정만 하고 강제하지 않음([P1]). DS2 구간 264개([P2] Table 1 대비 제품별 1개 적음, 논문 합 274). 중첩 없음, 연쇄(종료 = 다음 시작) 있음, 시작·종료 스텝 샘플링 100%.

### 디스패칭

- 순위 = `tool.txt` FWLRANK 순: rank_HP(우선순위 높은 순, hot lot이 setup 유발 가능) → rank_RSETUP(필요 setup 시간 짧은 순) → rank_FIFO(대기열 도착 순, DS1·3) 또는 rank_CR(작은 순, DS2·4). DS3·4는 LithoTrack_FE_95·115, Planar 6 TG에 rank_RSETUP 없음. 동률은 lot 번호. 설정의 기준 목록(`ranking`)이 있는 TG는 그 목록([운영 전략](#운영-전략)).
- CR = (납기 − t) / 잔여 공정시간. 잔여 공정시간 = 현재 스텝부터 기대 스텝시간 합(load + 공정 + unload, 샘플링 확률 가중, 반송·리워크 제외). 스텝별 a + b·n 형태 접미합을 사전 계산.
- 자격 필터: LTL 전용 툴, setup run, super hot 예약, Stopping 보류. CAtE·CoT는 순위 맨 앞 유형 키.
- rule_LSSU(Implant_128·132·91, setup 그룹 Implant_Gas, MINRUN 7): setup 변경 후 해당 setup으로 7 lot 처리 전 재변경 금지(run 길이는 변경 시 MINRUN, lot 시작마다 1 감소). 미완 run 중엔 hot lot도 setup을 바꾸지 않는 lot만 자격(AutoSched 문서: run 최소 lot 보장), 없으면 대기(가정). 현 setup lot이 더 올 수 없으면 대기 해제(가정).
- 툴 선택(유휴 툴 복수): wake_LeastSetupTime TG(DS1·2 9개, DS3·4 15개)는 도착 lot의 setup 시간 최소. 그 외·동률은 유휴 최장(가정).
- super hot(우선순위 30, rule_HotLotFIRST): HOTLOT=yes면 현 스텝 공정 시작 시(HOTLOTDELAY%=0) 다음 스텝 TG 툴 1대 예약, 예약 툴은 도착까지 대기, setup은 도착 후. rule_LSSU TG 제외, TG당 예약 1건, 예약 툴 고장·PM 시 다음 가용 툴로 이전(가정). `.asd`는 전부 HOTLOT=no라 기본 비활성, 옵션으로 활성(xlsx 시맨틱).

### 가용성

- UDT(`downcal`·`attach`, 영역 11개): TTF·첫 TTF Exp(10,080 min), TTR Exp(MTTR min: Def_Met·Litho_Met·TF_Met 35.28, Diffusion 151.2, Planar 201.6, Wet_Etch 221.76, Dry_Etch 231.84, TF 453.6, Dielectric·Implant 604.8, Litho 705.59). 달력 기준, 다음 TTF는 수리 종료부터. 공정 중 고장 시 중단 후 수리 뒤 재개(가정).
- SDT(`pmcal`·`attach`, TG별 292개 = MN 105, QT 105, WK 82). 시간형 79(Def_Met·Diffusion·Litho_Met·Litho·TF_Met: MN 30 d, QT 91 d, Litho WK 7 d)은 시작 간격 = MTBPM(소요 포함). 카운터형 213(그 외)은 툴별 처리 wafer 수가 MTBPM에 도달하면 시작, 카운터는 PM 시작 시 0(가정). 소요 U[MTTR±MTTR2] h. 첫 PM은 TG 내 k번째 툴(k = 1..N)에 FOA·k/N(일 또는 wafer). 진행 중 공정 완료 후 시작하고 대기 중 신규 착수 금지(가정). 동시 도래 PM은 순차 수행.
- 고장과 PM은 겹치지 않음: PM 중 도래한 고장은 PM 종료 후 수리 시작, 고장 중 도래한 PM은 수리 후 시작(가정, 기준 결과 PM%·[P1] 가용도 근거). 고장끼리 겹치면 모두 끝나야 가용. `Delay_32`는 정지 없음.

### 통계

- 기간: WarmUp(2018, 종료 시 초기화) + 연도별 누적 Period_1–7(`period.txt`, REPORT = yes만 보고, RESET = yes면 종료 시 초기화). 종료 시각에서 기간을 잘라 보고 후 초기화, 이후 완료분은 Drain 보고. 설정 `warm_up` = w이면 WarmUp [0, w)·Period_1 [w, 종료 시각)으로 대체(짧은 실험용, 확장).
- lot: TH, CT 평균·표준편차·분위수, ONTIME%(완료 ≤ 납기), FF = CT/RPT. 제품 × 유형(PRL·PHL·super hot·ERL·EHL)별.
- RPT = 빈 fab 기대 CT = Σ 샘플링 가중(스텝시간 + 반송) + 리워크 기대분(루프별 q/(1−q)회 재수행, q = 샘플링 × 리워크 확률). [P1] Table II 대비 −0.1 ~ +1.1%(10제품 확인).
- WIP 시간가중 평균.
- 툴·TG·영역: DOWN·PM·SETUP·LOAD·UNLOAD·PROC·IDLE %, UTIL = SETUP + LOAD + UNLOAD + PROC, 가용도 = 100 − DOWN − PM, SDT 비중 = PM/(DOWN + PM)(`stnfam.rep` 정의와 동일). cascading 툴의 두 job이 겹치면 DOWN > PM > SETUP > PROC > LOAD > UNLOAD > IDLE 순 하나로 집계(가정, 기준 결과 근거).
- CQT([P2]): 대기 = 시작 스텝 종료 ~ 종료 스텝 작업 시작(setup·load 전). %VL(위반 / 구간 완료), %VL1h·2h·4h, AVL·AONT(h, 구간 완료 전체 평균 위반·여유 시간). Litho(LithoTrack_FE_95·115 포함 구간)·Rest·Total, 그리고 구간 정의별과 그 스텝별 반송·대기열·공정 시간(위반·충족 완료별, 확장).
- 일별(확장): 투입·완료 lot, 시간가중 WIP, 그날 끝난 CQT 구간의 완료·위반.
- 스텝 추적은 저장하지 않고 실행 중 집계(4년 lot-step 3,000만 건 이상 → 저장 시 GB 규모). 분위수용으로 lot별 CT만 보관.

## 운영 전략

| 전략 | 출처 | 내용 |
|---|---|---|
| BASE | P1·P2 | 데이터 순위(HP → RSETUP → FIFO/CR) |
| QTCR | P2 식 (1) | 순위 HP → RSETUP → QTCR → FIFO/CR. d^Q = C_s + CQT. t ≤ d^Q면 (d^Q − t)/Σ_{k=i..n} p_k, 아니면 (d^Q − t)·Σ_{k=i..n} p_k. 작을수록 우선, 구간 밖 lot = +∞ |
| QTS | P2 식 (2)–(6) | QTCR 자리에 d_i. TW_k = (FF_k − 1)p_k, TT = Σ_{k=s+1..n−1} FF_k·p_k + TW_n, Ratio_k = FF_k·p_k/TT(k < n), TW_n/TT(k = n), FCQT_k = CQT·Ratio_k, d_i = C_s + Σ_{k=s+1..i} FCQT_k − p_i(i < n), d_n = C_s + CQT. FF_k = 평균 스텝 CT/p_k(스텝 CT = 이전 수행 스텝 종료 ~ 본 스텝 종료, 종료 시각까지 기간, 미측정 1). 설정에 FF가 없으면 같은 설정에서 QT 규칙·QT 기준·QT 배치 시작·Stopping을 뺀 사전 실행으로 산출(가정) |
| Stopping | P2 §3.2·Table 3 | TG별 임계 ①TG 앞 CQT lot(대기·공정 중) ②① + 구간 안에서 그 TG에 아직 도달하지 않은 CQT lot(이동 중 포함, TG당 lot 1회). 구간의 TG(시작 다음 ~ 종료 스텝) 중 하나라도 도달하면 구간 시작 스텝에서 보류, 임계가 해제되는 사건(도달 → 미도달)에서 재평가. 직전 구간 종료 = 현 구간 시작이면 무시. BASE·QTCR·QTS와 결합. 임계(①/②) LithoTrack_FE_95: none 1000/1000, high 90/130, medium 60/95, small 50/85. FE_115: 1000/1000, 90/220, 70/150, 55/125. 그 외 1000/1000. 임계 > 0. 배치·LSSU TG 임계가 최소 배치·run을 채울 lot까지 보류하면 교착 → 미완료 오류 |
| EF | P1 §V | 우선순위 EHL 25, PHL 20, ERL 15, PRL 10(전 TG) |
| CAtE | P1 §V | LithoTrack_FE_95·115만. 생산·엔지니어링 구간 (lp, le) h 교대, t=0 생산 구간부터(가정). DS3 (151.2, 16.8)·(75.6, 8.4)·(21.6, 2.4), DS4 (134.6, 33.4)·(67.2, 16.8)·(19.2, 4.8). 구간 유형 lot만, 없으면 다른 유형 |
| CoT | P1 §V | LithoTrack_FE_95·115만. 대기 EL ≥ 한계(100·50·25·10)면 그 수만큼 EL 우선. 그 외 PL 우선, PL 없으면 EL(가정) |
| 기준 목록 | 확장 | TG별 기준 1–6개(`ranking`, 서로 다름, 앞이 우선, 동률은 lot 번호, CAtE·CoT 유형 키 뒤). 기준(작을수록 우선): `priority`·`least_setup`·`fifo`·`critical_ratio`(= rank_HP·RSETUP·FIFO·CR), `due_date`(납기), `shortest_step`(현 스텝 기대 시간), `least_remaining`(잔여 기대 작업), `qtcr`·`qts`(위 식), `qt_deadline`(C_s + CQT), `{"qt_within": h}`(QT 여유 ≤ h인 lot 먼저, 나머지 동률). QT 기준은 구간 밖 lot을 맨 뒤로. 목록 없는 TG = 데이터 순위 + `queue_time` |
| QT 배치 시작 | 확장 | `batch_start_within` = h: 최소 미만 배치도 후보 lot 하나의 QT 여유가 h 이하가 되면 시작. 보류 시 그 시각에 깨우기 사건 예약 |

- QTCR·QTS의 p_k = CR과 같은 기대 스텝시간.
- QT 여유 = C_s + CQT − t − 종료 스텝 시작까지의 기대 작업(Σ_{k=i..n−1} p_k, 종료 스텝 대기 중 0). 음수면 이미 늦음.
- [P2] complex CQT(441 구간)는 추가 구간의 CQT 값이 미공개라 재현 불가, default만 재현.

## API

코어는 Rust 라이브러리 `smt2020` 하나다. JavaScript(`fab_wasm`)·Python(`smt2020`)은 같은 기능과 직렬화 스키마를 언어 관례대로 노출하는 포장이고(시간 ms, `DAY`·`HOUR`·`MINUTE`·`SECOND` 상수), CLI(`convert`, `run`, `validate`)는 그 위의 실행 도구다.

| 기능 | Rust `smt2020` | JS `fab_wasm` | Python `smt2020` |
|---|---|---|---|
| 데이터셋 | `Dataset::from_bytes`·`to_bytes`, `asd::{load, load_with_orders, orders}` | `new Dataset(bytes)` | `Dataset(bytes)`, `load_dataset(source)` |
| 데이터셋 정보 | `Dataset::info()` | `dataset.info()` | `dataset.info()` |
| 생성(시각 0) | `Simulation::new(Arc<Dataset>, Config)`, `with_recording(…, Recording)` | `new Simulation(dataset, config, recording?)` | `Simulation(dataset, config, recording=None)` |
| 실행·일시정지 | `run(until)`, `run_observed(until, 관찰자)` | `run(until?, onProgress?)` | `run(until=None, on_progress=None)` |
| 상태 | `progress()`, `lots()`, `tools()`, `tool_groups()` | `progress()`, `lots()`, `tools()`, `toolGroups()` | `progress()`, `lots()`, `tools()`, `tool_groups()` |
| 설정·재시작 | `config()`, `reset(Config)` | `config()`, `reset(config?)` | `config()`, `reset(config=None)` |
| 기록·재생 | `recording()`, `records()`, `flow_factors()` | `recording()`, `records()`, `flowFactors()` | `recording()`, `records()`, `flow_factors()` |
| 결과 | `results()`, `Results::digest` | `results()`, `digest(results)` | `results()`, `digest(results)` |
| 복제 요약 | `report::{metrics, summarize, daily, csv}` | `summarize(results[])`, `daily(results[])`, `csv(summaries)` | `summarize(results)`, `daily(results)`, `csv(summaries)` |

### 실행 흐름

- `Simulation` = 설정 1개의 실행 1회. 생성할 때 설정을 검증하고(잘못되면 오류) 시각 0에서 시작한다.
- `run(until)`: `until`(ms, 그 시각의 사건 포함)까지 또는 끝까지 진행하고 진행 상태를 돌려준다. 관찰자는 모의 1일마다 진행 상태를 받고, `false`(Rust `Break`)를 돌려주면 그 시각에 일시정지한다. 관찰자의 예외(JS throw, Python 예외·Ctrl-C)도 일시정지 후 그대로 전달된다. 다시 `run`하면 이어서 진행하고, 일시정지는 결과를 바꾸지 않는다(같은 digest).
- 종료 조건: `horizon` 전에 시작하는 lot만 투입하고, 투입 lot이 전부 완료되면 끝난다(`finished`, 이후 `run`은 그대로). 종료 시각 + 365일에 lot이 남으면 실행 실패(이후 `run`도 같은 오류). 그 밖의 조건(시각, WIP, 완료 수 등)은 `until`·관찰자로 일시정지해 판단한다.
- 상태: 진행·일시정지 중 언제든 읽는다. 아래 [진행·상태·결과](#진행상태결과).
- `results()`: 끝난 실행의 결과(전에는 오류). `reset(config)`: 같은 데이터셋으로 시각 0부터(설정을 생략하면 같은 설정, 거부되면 그대로 유지).
- 운영 전략은 설정(`queue_time`, `ranking`, `batch_start_within`, `stopping`, `engineering`, `reserve_super_hot`)으로 정하고 `reset`으로 바꾼다. 실행 중 변경은 없다(대기열 항목이 도착 시 순위 입력을 고정).
- QTS(규칙 또는 `qts` 기준) + `flow_factors` 없음: 같은 설정에서 QT 규칙·QT 기준·QT 배치 시작·Stopping을 뺀 1차 실행이 FF를 측정한 뒤 본 실행(진행 `pass` 0/2 → 1/2). `until`·결과는 본 실행 기준이고, 설정 검증은 생성 시 함께 한다.
- 기록: `with_recording`(JS·Python은 생성자의 세 번째 인자)이 위반 구간 완료, TG 일별 상태, 사건 창을 기록한다([기록](#기록)). 결과는 바뀌지 않는다(digest 동일). 재생 = 같은 설정·복제를 기록과 함께 다시 실행(결정적이라 같은 실행), QTS는 `flow_factors()`를 설정에 넣어 1차 실행을 생략한다.
- 병렬: JS는 Web Worker마다, Python은 스레드마다(실행 중 GIL 해제), Rust는 스레드마다(`Simulation: Send`) `Simulation`을 둔다. 데이터셋은 공유한다.

### 실행 설정

JSON·JS 객체·Python dict·Rust `Config` 공통. `horizon` 외 필드는 생략하면 기본값, 미지 필드는 오류, 시간 필드는 ms 수(소수는 ms 반올림, `until`도 같음).

| 필드 | 기본값 | 내용 |
|---|---|---|
| `horizon` | 필수 | 종료 시각. 이전에 시작하는 lot만 투입하고 이후 Drain |
| `warm_up` | null | 웜업. 보고 기간을 WarmUp [0, warm_up)·Period_1 [warm_up, horizon)로 대체 |
| `seed`, `replication` | 1, 0 | 난수 스트림. 같은 값이면 전략이 달라도 공통 난수 |
| `load` | 1 | 부하 계수(투입 시각 ÷ load, 납기 오프셋 유지) |
| `reserve_super_hot` | false | super hot lot 다음 툴 예약 |
| `queue_time` | `"none"` | `"none"`·`"qtcr"`·`"qts"`. 기준 목록 없는 TG의 FIFO/CR 앞 |
| `flow_factors` | null | QTS FF(route × 스텝, null = 미측정 = 1). 없으면 1차 실행으로 산출 |
| `ranking` | `{}` | TG별 기준 목록: `{"Diffusion_FE_120": [{"qt_within": 7200000}, "priority", "fifo"]}` |
| `batch_start_within` | null | QT 배치 시작 임계(ms) |
| `stopping` | null | `{"limits": {"LithoTrack_FE_95": {"front": 50, "total": 85}}, "default": {"front": 1000, "total": 1000}}` |
| `engineering` | `"base"` | `"base"`·`"engineering_first"`·`{"cate": {"production": ms, "engineering": ms}}`·`{"cot": {"trigger": 100}}` |

### 진행·상태·결과

- 데이터셋 정보(`info()`): `areas`, `tool_groups[]` {`name`, `area`, `tools`, `batching`, `setup_runs`, `stepper`, `ranks`(데이터 순위 기준)}, `parts[]` {`name`, `family`, `engineering`, `route`}, `routes[]` {`name`, `steps[]` {`name`, `tool_group`}}, `segments[]`(CQT 구간, route·시작 스텝 순 = 구간 index) {`route`, `entry`, `exit`, `limit`, `litho`, `tool_groups`}, `periods[]` {`name`, `start`, `report`, `reset`}. 숫자 참조는 각 목록의 index.
- 진행(`Progress`, 관찰자에게 1일 1회, `progress()`·`run`의 반환): `pass`·`passes`(QTS 1차 실행이면 0/2·1/2), `now`(그 pass의 시각), `horizon`, `released`·`completed`(투입·완료 lot), `wip`, `cqt_completed`·`cqt_violated`(CQT 구간 완료·위반 누적), `finished`.
- lot(`lots()`, 재공 lot을 id 순으로): `id`(투입 순번, 0부터), `part`, `kind`, `priority`(디스패칭 우선순위, EF 반영), `wafers`, `release`, `due`, `step`·`step_name`·`tool_group`(이동 중이면 향하는, 대기·공정 중이면 그 스텝), `state`(`moving`·`queued`·`processing`), `tool`(공정 중인 툴 id), `cqt_exit`·`cqt_deadline`(진행 중인 CQT 구간의 종료 스텝과 그 스텝의 한도 내 최종 시작 시각).
- 툴(`tools()`, id = 데이터셋의 툴그룹 순서대로 매긴 번호): `id`, `tool_group`, `state`(`down`·`pm`·`setup`·`process`·`load`·`unload`·`idle`, cascading job이 겹치면 앞선 상태), `setup`(현재 setup), `lots`(공정 중인 lot id, cascading은 job 2개).
- 툴그룹(`tool_groups()`, 데이터셋 순): `name`, `area`, `tools`, `queue`(대기 lot 수), 상태별 툴 수 `down`·`pm`·`setup`·`process`·`load`·`unload`·`idle`.
- 결과(`Results`): `seed`·`replication`, `periods[]`(보고 기간, 마지막은 Drain: `name`, `start`, `end`, `lots[]` {`part`, `kind`(PRL·PHL·SHL·ERL·EHL), `started`, `completed`, `on_time`, `cycle_time_mean`·`cycle_time_std`(ms, 완료 없으면 null), `flow_factor_mean`}, `flow_factors[]` {`kind`, `percentiles`(0·5·25·50·75·95·100%)}, `wip`, `tool_groups[]` {`name`, `area`, `tools`, `time` {`down`, `pm`, `setup`, `process`, `load`, `unload`, `idle`}(ms)}, `cqt_litho`·`cqt_rest` {`completed`, `violated`, `violated_1h`·`2h`·`4h`, `violation`·`slack`(ms)}, `cqt_segments[]`(데이터셋 구간 순) {`route`, `entry`, `exit`, `litho`, `cqt`(같은 형식), `steps[]` {`step`, `met`·`violated` {`visits`, `transport`, `queue`, `process`}(ms 합)}}), `days[]`(일별: `started`, `completed`, `wip`, `cqt`), `released`, `completed`, `end`, `events`, `step_flow_factors`(QTS `flow_factors` 입력).
- CQT 대기 분해: 대기(시작 스텝 종료 ~ 종료 스텝 작업 시작) = 구간 안 스텝마다 반송(직전 종료 ~ 도착) + 대기열(도착 ~ 작업 시작) + 공정(작업 시작 ~ 종료, setup·load·unload·고장 정지 포함) + 종료 스텝의 반송·대기열. 위반·충족 완료로 나눠 합한다.
- digest: 결과 postcard 인코딩의 FNV-1a 64비트 해시. 같으면 결과가 비트 단위로 같다.
- 측정값(`summarize`·`csv`): 범위 fab·kind·lot·tool_group·area·cqt·cqt_segment(`route:entry-exit`)·cqt_step(`route:entry-exit:step`)별 측정(`ct_mean_d`, `on_time_pct`, `ff_p50`, `util_pct`, `vl_pct`, `avl_h`, `queue_vl_h` 등, 접미사 = 단위)의 n·평균·표본 표준편차·95% CI 반폭(Student t). `daily`: 일별 fab(started·completed·wip)·cqt(completed·vl_pct·avl_h)의 같은 통계. 목록·정의는 [SIMULATION.md](SIMULATION.md) 9.3–9.4. CSV 열 `period,scope,item,kind,measure,n,mean,std,ci95`.
- CLI·웹 JSON: `{data, threads, replications: [{config, digest, seconds, results}], summary}`(CLI는 `peak_heap_bytes`, 웹은 복제별 `memory_bytes` 추가).

### 기록

설정 `{"violations": bool, "tool_groups": bool, "events": {"from": ms, "until": ms, "tool_groups": [이름], "lots": [투입 순번]}}`(생략 = 끔, 빈 목록 = 전부). `records()`는 같은 길이 열의 표 3개(Python은 `pandas.DataFrame(records["violations"])`로 바로 쓴다, 숫자 참조는 `info()` index).

| 표 | 행 | 열 |
|---|---|---|
| `violations` | 한도를 넘긴 구간 완료 | `lot`, `part`, `kind`, `segment`, `release`, `entered`(시작 스텝 종료), `arrived`(종료 스텝 도착), `exit`(종료 스텝 작업 시작) |
| `tool_groups` | 날 × TG | `day`, `tool_group`, `queue`(시간가중 대기 lot), `down`·`pm`·`setup`·`process`·`load`·`unload`·`idle`(툴 시간 합, ms) |
| `events` | 창·필터를 통과한 사건 | `time`, `kind`(release·arrive·start·end·complete·down·up·pm_start·pm_end), `lot`, `part`, `tool`, `tool_group`, `step`(`part`의 route 스텝; 없으면 null) |

- TG 필터는 TG 없는 사건(투입·완료)을, lot 필터는 lot 없는 사건(고장·PM)을 거른다. 창 끝 ≤ 시작, 미지 TG는 오류.
- 규모(DS2 730 d): 위반 약 17.7만 행, TG 일별 약 8.1만 행(JSON 19.5 MB), 기록 시간 1–2% 증가. 사건은 전 TG 하루 약 4.2만 행.

### 데이터셋 파일

`SMT2020\0`(8 B) + 형식 버전(u32 LE, 현재 1) + postcard(`Dataset`). 다른 형식 버전은 오류이며 `convert`로 다시 만든다. 크기: DS1 0.07 MB, DS2 2.98 MB, DS3 0.37 MB, DS4 3.61 MB(변환 1초 미만), 압축 없음.

## 실험·검증

| 실험 | 데이터 | 조건 | 비교 대상 |
|---|---|---|---|
| 기준 실행 | DS1–4 | 1,460 d(2018–2021), 웜업 1년, seed 1 복제 3회 / AutoSched 1회 | `.rep` 마지막 기간(Period_3 = 2019–2021 누적): perf(LOTCOMPS, WIPLOTAVG), order(CYCLEAVG·STD, ONTIME%), stnfam(DOWN·PM·SETUP·PROC·UTIL %), stngrp(영역 가용도·UTIL %) |
| 운영 곡선 | DS1·2 | 8년(2,920 d, 웜업 1년), 1회, 부하 50–100%(용량 10,200·10,250 WSPW) | [P1] Fig. 2·3(일반 lot FF 분위수): 10,000 WSPW에서 중앙값 1.85·1.95, 5–95% 1.73–2.01·1.79–2.13, 최대 2.37·2.46 |
| 엔지니어링 전략 | DS3·4 | 2년(웜업 1년), 20회. BASE, EF, CAtE 3종, CoT 4종 | [P1] Fig. 4·5(유형별 ACT) |
| CQT | DS2 | 2년(웜업 1년), 10회, 초기 WIP. BASE·QTCR·QTS(+Stopping) | [P2] Table 5(default) |

- AutoSched XTHEOR는 내부 이론 CT 기준이라 FF와 직접 비교하지 않음(CT·TH·ONTIME·가동률로 비교).
- 난수 생성기가 달라(AutoSched CMRG) 경로 일치는 불가. 복제 평균과 95% 신뢰구간으로 비교한다. 전략 비교는 같은 seed·복제 번호(공통 난수).
- 명령(`B=target/release/smt2020`):

```bash
for m in "dataset 1/HVLM_Model" "dataset 2/LVHM_Model" "dataset 3/HVLM_E_Model" "dataset 4/LVHM_E_Model"; do
  $B validate "data/raw/AutoSched/$m" --csv "validate-${m%%/*}.csv"
done
for e in base engineering_first cate:151.2/16.8 cate:75.6/8.4 cate:21.6/2.4 cot:100 cot:50 cot:25 cot:10; do
  $B run www/data/ds3.bin --replications 20 --engineering $e --csv "ds3-${e//[:\/]/_}.csv"
done   # DS4: cate:134.6/33.4 cate:67.2/16.8 cate:19.2/4.8
for q in none qtcr qts; do
  $B run www/data/ds2.bin --replications 10 --queue-time $q --csv "ds2-$q.csv"
  $B run www/data/ds2.bin --replications 10 --queue-time $q --stopping LithoTrack_FE_95=50/85 --stopping LithoTrack_FE_115=55/125 --csv "ds2-$q-small.csv"
  $B run www/data/ds2.bin --replications 10 --queue-time $q --stopping LithoTrack_FE_95=5/10 --stopping LithoTrack_FE_115=5/10 --csv "ds2-$q-5_10.csv"
done
$B convert "data/raw/AutoSched/dataset 2/LVHM_Model/LVHM_Model.asd" data/ds2-periodic.bin --orders order.txt WIP.txt
for L in 50 60 70 80 85 90 92 95 97.5 98 99 100; do   # 부하 L% = 계수 L·1.02(DS1), L·1.025(DS2) / 100
  $B run www/data/ds1.bin --horizon 2920 --load $(python -c "print($L*102/10000)") --csv "oc-ds1-$L.csv"
  $B run data/ds2-periodic.bin --horizon 2920 --load $(python -c "print($L*1025/100000)") --csv "oc-ds2-$L.csv"
done
```

### 검증 결과

`validate` 출력(1,460 d, Period_3 누적 = 2019–2021). 본 모델 seed 1 복제 3회 평균 / AutoSched 1회:

| | DS1 | DS2 | DS3 | DS4 |
|---|---|---|---|---|
| 완료 lot | 62,572 / 62,647 | 62,606 / 62,369 | 68,757 / 68,856 | 75,103 / 75,160 |
| CT PRL | +3.5%(제품별 +3.3 ~ +3.6) | +0.3%(−0.6 ~ +0.8) | +4.1%(+4.0 ~ +4.2) | +0.9%(−0.2 ~ +1.8) |
| CT ERL | – | – | +4.3% | +0.5% |
| CT PHL·EHL | +3.6% | +3.9% | +4.0%·+4.2% | +5.2%·+5.0% |
| CT super hot | +3.4% | +4.0% | +4.3% | +5.6% |
| 평균 WIP(lot) | 2,345 / 2,265 | 2,148 / 2,140 | 2,520 / 2,420 | 2,761 / 2,734 |
| SETUP% LithoTrack_FE_95·115 | 10.3·5.9 / 10.8·6.2 | 11.6·5.9 / 11.8·6.0 | 14.3·7.7 / 14.7·7.9 | 19.6·10.0 / 19.7·10.1 |
| SETUP% Implant_128 | 17.1 / 15.5 | 17.3 / 16.6 | 19.6 / 17.4 | 20.7 / 17.9 |
| UTIL% Planar_BE_75 | 72.6 / 69.8 | 73.1 / 71.5 | 74.4 / 70.4 | 74.1 / 69.8 |

- 계획 lot 전량 완료(1,460 d: 85,767·85,671·94,360·102,931 lot).
- TG 상태 비율 차(106 TG 평균 %p, DS1–4): DOWN −0.03 ~ 0.00, PM −0.04 ~ −0.02(최대 |차| 0.45), SETUP +0.02 ~ +0.42, PROC −0.32 ~ −0.07, UTIL −0.09 ~ +0.11. 영역 가용도 차 ≤ 0.24%p(stngrp). order별 CT 표준편차 차 ≤ 0.13·0.36·0.38·0.35 d. 복제 간 PRL CT 변동 약 ±2%.
- 잔여 편차(AutoSched 내부 동작 미문서, 근거 없는 보정 없음):
  - hot lot CT +3.4 ~ +5.6%.
  - HV/LM(DS1·3) PRL CT +3.5 ~ +4.1%. 고정 납기라 DS1·3 ONTIME% 하락(PRL 59–75% / 89–92%, 복제 간 편차 큼).
  - 대형 cascading TG 가동률 과다(Planar_BE_75 +1.6 ~ +4.3%p, 영역 Planar +1.0 ~ +3.2%p). 유휴 툴 우선 배정으로 cascading이 기준보다 적은 것으로 추정(job을 막 시작한 툴 우선 배정 시 전체 −1.4%p로 반대 편차).
  - LSSU Implant(128·132·91) SETUP% +0.7 ~ +2.8%p. route STIME setup의 Implant_90·119 PROC% −2.3 ~ −4.2%p(UTIL −2.9 ~ +0.1%p).
  - E lot 보정 setup이 있는 Planar(DS3·4) SETUP% 과다·PROC% 과소(DS4 Planar_FE_77 13.7 / 5.9, PROC 49.6 / 55.8): cascading 겹침 구간 SETUP > PROC 집계(가정) 영향으로 추정.
  - CR 데이터셋 part_6·9 ONTIME% 99.7–99.8 / 기준 71–82%([P2] Table 4도 71%).

### 엔지니어링 전략

DS3·4, 2년, 20회, Period_1(2019) ACT(d). 본 모델 평균 / [P1] Fig. 4·5 판독값(±0.5 d). 95% CI 반폭은 ERL·EHL ≤ 1.2 d, PRL ≤ 0.6 d, PHL ≤ 0.1 d.

| 전략 | DS3 PRL | DS3 ERL | DS3 EHL | DS4 PRL | DS4 ERL | DS4 EHL |
|---|---|---|---|---|---|---|
| BASE | 39.8 / 38.5 | 47.3 / 45.8 | 29.7 / 28.5 | 39.3 / 39.0 | 49.4 / 49.2 | 31.3 / 30.0 |
| EF | 42.7 / 40.0 | 30.3 / 29.2 | 29.7 / 28.4 | 47.7 / 40.0 | 33.3 / 31.2 | 31.8 / 29.8 |
| CAtE 168 h | 39.9 / 38.6 | 64.5 / 64.0 | 37.2 / 36.3 | 39.3 / 38.9 | 50.0 / 49.8 | 33.7 / 32.4 |
| CAtE 84 h | 39.9 / 38.7 | 61.6 / 60.6 | 37.3 / 36.0 | 39.3 / 39.0 | 49.9 / 49.7 | 34.0 / 32.8 |
| CAtE 24 h | 39.5 / 38.3 | 53.3 / 52.0 | 34.9 / 33.7 | 39.4 / 39.0 | 49.7 / 49.4 | 34.5 / 33.2 |
| CoT 100 | 39.6 / 39.3 | 58.9 / 67.4 | 35.3 / 42.6 | 39.2 / 39.6 | 49.8 / 50.7 | 33.5 / 36.1 |
| CoT 50 | 39.5 / 38.8 | 52.8 / 55.4 | 33.5 / 36.0 | 39.3 / 39.2 | 49.6 / 49.6 | 32.9 / 33.0 |
| CoT 25 | 39.6 / 38.6 | 49.0 / 49.1 | 32.0 / 32.2 | 39.4 / 39.2 | 49.5 / 49.4 | 32.2 / 31.4 |
| CoT 10 | 39.8 / 38.4 | 46.8 / 45.4 | 30.7 / 30.0 | 39.5 / 39.1 | 49.6 / 49.3 | 31.7 / 30.5 |

- PHL: DS3 26.0 ~ 26.1 / 약 25, DS4 25.2 ~ 26.1 / 약 24(기준 실행 hot lot 편차와 같은 수준).
- [P1] §V 경향과 일치: EF는 EL 최선·PL 최악, 긴 생산 구간·큰 트리거는 EL 악화, CR(DS4)에서는 전략 간 차이 축소.
- 차이: DS3 CoT 100의 EL ACT가 [P1]보다 낮음(ERL 58.9 / 67.4), [P1]의 DS3 CoT 100 PL 악화는 재현되지 않음(PRL 39.6 / BASE 39.8). DS4 EF의 PRL ACT가 [P1]보다 높음(47.7 / 40.0, PRL ONTIME 0%): EL 우선이 스테퍼 보정 setup을 늘리는 것으로 추정.

### CQT

DS2, 2년, 10회, Period_1(2019). 본 모델 평균 / [P2] Table 5(default, None 시나리오). 95% CI 반폭: ACT ≤ 0.3 d, %VL ≤ 0.7%p, AVL ≤ 0.08 h.

| 규칙 | PRL ACT(d) | PRL ONTIME(%) | %VL Total·Litho·Rest | %VL1h·2h·4h Total | AVL(h) Total | AONT(h) Total |
|---|---|---|---|---|---|---|
| BASE | 37.9 / 37.7 | 99.3 / 92.7 | 15.3·12.5·15.5 / 17.5·18.6·17.4 | 13.6·12.3·10.3 / 15.6·14.1·11.7 | 1.78 / 1.92 | 2.36 / 2.31 |
| QTCR | 37.7 / 37.5 | 94.7 / 93.4 | 9.4·2.1·10.0 / 9.4·1.2·10.6 | 7.8·6.6·4.9 / 7.7·6.5·4.8 | 0.73 / 0.71 | 2.54 / 2.56 |
| QTS | 37.9 / 37.7 | 93.3 / 91.3 | 9.5·2.0·10.1 / 9.3·1.1·10.5 | 7.9·6.7·5.1 / 7.7·6.5·4.9 | 0.75 / 0.73 | 2.54 / 2.56 |

- ONTIME 차는 part_6·9(기준 실행 잔여 편차) 영향.
- Stopping: [P2] Table 3 small(50/85·55/125)은 default 구간에서 발동하지 않는다(BASE·QTCR·QTS 모두 Stopping 없는 실행과 digest 동일, [P2]는 complex 설정에 적용). 스테퍼 5/10이면 %VL Total BASE 15.3 → 10.2%(Litho 12.5 → 3.5), QTCR 9.4 → 7.2%이나 PRL ACT 37.9 → 92.1 d, QTCR 37.7 → 83.4 d(용량 낭비, [P2] §2.1), 전 lot 완료.

### 운영 곡선

8년(2,920 d), 1회, Period_7(2019–2025 누적) 일반 lot(PRL) FF 분위수 P0·P5·P25·P50·P75·P95·P100. 부하 = 투입 / 용량(10,200·10,250 WSPW), 계획 10,000 WSPW = DS1 98%·DS2 97.6%. DS2는 목록형 투입이 2025-12-31에 끝나 용량 근처 8년을 덮지 못하므로 비활성 주기형 `order.txt`(고정 납기 오프셋) + `WIP.txt`로 실행(가정).

| 부하(%) | DS1 | DS2 |
|---|---|---|
| 50 | 1.02·1.09·1.13·1.16·1.19·1.26·1.65 | 1.04·1.29·1.40·1.47·1.55·1.69·2.26 |
| 60 | 1.03·1.10·1.14·1.17·1.21·1.28·1.62 | 1.07·1.29·1.38·1.44·1.50·1.62·2.13 |
| 70 | 1.06·1.13·1.17·1.21·1.26·1.33·1.64 | 1.10·1.32·1.39·1.45·1.51·1.60·2.04 |
| 80 | 1.08·1.19·1.25·1.30·1.35·1.43·1.79 | 1.14·1.37·1.45·1.52·1.59·1.70·2.08 |
| 85 | 1.13·1.24·1.31·1.36·1.41·1.49·1.82 | 1.23·1.41·1.52·1.60·1.68·1.81·2.11 |
| 90 | 1.13·1.34·1.41·1.48·1.54·1.63·2.00 | 1.31·1.51·1.64·1.72·1.80·1.93·2.18 |
| 92 | 1.21·1.38·1.47·1.54·1.60·1.69·2.06 | 1.37·1.59·1.72·1.80·1.88·2.01·2.16 |
| 95 | 1.34·1.50·1.59·1.66·1.72·1.81·2.20 | 1.47·1.70·1.82·1.89·1.96·2.06·2.52 |
| 97.5 | 1.47·1.68·1.78·1.86·1.92·2.02·2.38 | 1.63·1.87·1.96·2.01·2.06·2.11·2.25 |
| 98 | 1.51·1.74·1.85·1.93·2.00·2.10·2.48 | 1.59·1.90·1.98·2.03·2.07·2.11·2.27 |
| 99 | 1.68·1.87·1.98·2.06·2.13·2.22·2.59 | 1.80·1.96·2.04·2.07·2.10·2.13·2.39 |
| 100 | 1.90·2.10·2.21·2.28·2.35·2.44·2.87 | 1.93·2.00·2.06·2.09·2.11·2.16·2.49 |

- 10,000 WSPW(DS1 98%·DS2 97.5%) / [P1] 본문: 중앙값 1.93·2.01 / 1.85·1.95, P5–P95 1.74–2.10·1.87–2.11 / 1.73–2.01·1.79–2.13, 최대 2.48·2.25 / 2.37·2.46.
- DS1은 부하 전 구간에서 [P1] Fig. 2와 같은 형태(100%: P50 2.28 / 약 2.25, P100 2.87 / 약 2.76), 98% 부근은 기준 실행의 HV/LM CT 편차(+3.5%)만큼 높다.
- DS2는 50–70%에서 상위 분위수(P95·P100)가 부하와 함께 감소하는 [P1] Fig. 3의 현상(저부하 배치 대기)을 재현한다. 용량 근처 분포는 [P1]보다 낮고 좁다(100%: P50 2.09 / 약 2.44). 납기 형태의 영향: 97.5%에서 목록형 투입(lot별 납기 z~U[0.8, 1.2])은 중앙값 2.01 동일, P5–P95 1.62–2.41로 [P1]보다 넓다.

## 성능 측정

같은 PC(Intel i7-10700, 8코어 16스레드), 단일 스레드·복제 1회, 1,460 d(Drain 포함), seed 1·복제 0. 네이티브는 `smt2020 run --threads 1`(최대 메모리 = CLI 계수 할당자의 최대 힙), wasm은 웹 페이지(Chromium 152, Web Worker 1개, 최대 메모리 = 워커 wasm 선형 메모리).

| | DS1 | DS2 | DS3 | DS4 |
|---|---|---|---|---|
| 사건 | 70.5 M | 60.8 M | 79.3 M | 77.3 M |
| 네이티브: 시간·사건/s | 25.5 s · 2.77 M | 23.8 s · 2.55 M | 30.2 s · 2.63 M | 33.1 s · 2.33 M |
| wasm: 시간·사건/s | 31.3 s · 2.25 M | 28.4 s · 2.15 M | 36.6 s · 2.17 M | 39.6 s · 1.95 M |
| wasm / 네이티브 시간 | 1.23 | 1.19 | 1.21 | 1.20 |
| 최대 메모리 네이티브 / wasm | 9.9 / 10 MB | 35.0 / 25 MB | 13.6 / 12 MB | 37.3 / 27 MB |
| 결과 digest(네이티브 = wasm) | 822207a22054b640 | 89543570bb565f50 | ded1f0a167494bab | 27db95cd7bbcd187 |

- 결정성: 네 데이터셋 모두 네이티브·wasm(브라우저·Node.js)·Python wheel 결과가 비트 단위로 같다(digest 일치). Python은 실행 중 GIL을 놓아 DS1–4를 스레드 4개로 동시에 실행하면 36 s(각각 단독 26–34 s).
- 2년(730 d, 웹 기본값) 네이티브: 12.9·12.0·15.1·16.8 s. 병렬: DS3 2년 20회 16스레드 57 s.
- 이전 구현 대비(같은 1,460 d): 34.5·32.7·40.7·47.5 s → 25.4·23.5·29.7·33.0 s(26–31% 단축, 결과 동일). 대기열 항목(도착 시 순위 입력 고정), 선택당 공통 입력 1회 계산, LTO·단일 코드 생성 단위. Stopping 재평가를 임계 해제 사건으로 바꿔 DS4 180 d 스테퍼 5/10 실행 77.8 → 6.3 s.
- 구현 단계 10–11의 비용(같은 세션 대조): 순위 기준 목록은 같은 속도(키 계산 인라인, 호출이면 약 50% 느림). 구간·스텝 분해·일별 결과·기록 지점은 DS2 730 d 약 2%(기록 꺼짐; 사건 항목은 기록할 때만 만든다, lot 시각표는 lot 구조체 밖). 기록 켬(위반·TG 일별)은 추가 1–2%. 결과 구조가 늘어 최대 힙 +2–3 MB, digest는 새 값(기존 측정값은 CSV 바이트 동일).
- 참고 기준(하드웨어 상이): AutoSched AP 1,460 d 1회 — DS1 36:02(lot-step 34.69 M), DS2 31:30(30.01 M), DS3 40:48(39.04 M), DS4 39:58(38.15 M).

## 구현 구조

```text
crates/des-core/  DES 코어 lib(모델 독립): 시각, 미래 사건 목록, 스케줄러, 사건 루프, 종료 시각, 관측 사건
crates/smt2020/   SMT2020 도메인 lib(des-core 참조): 데이터 모델·.asd 로더·데이터셋 파일, Simulation(단계 실행·상태), 전략, 통계, 측정값·복제 요약. 바인딩 의존 없음
crates/cli/       네이티브 CLI(패키지 smt2020-cli, 실행 파일 smt2020): convert, run, validate
crates/wasm/      JS 포장, wasm-bindgen cdylib(패키지 fab-wasm): Dataset, Simulation, summarize, csv, digest. tests/(Node API 테스트)
crates/python/    Python 포장, PyO3 cdylib(패키지 smt2020-python, maturin wheel smt2020): 같은 API + load_dataset(DS1–4 동봉), smt2020.pyi(타입), tests/(unittest)
www/              index.html, style.css, main.js(실행 조율·wheel 목록), views.js(보기), setup.js·strategy.js(시나리오·전략 편집기), pool.js·worker.js(워커 풀·실행·재생), progress.js(진행·FAB 지도), results.js(분석 개요), details.js(분석 상세), charts.js(ECharts 차트), labels.js(구간·스텝·시각 표기), files.js(내려받기), motion.js(애니메이션), i18n.js(문구·숫자 형식), locales/(en·ko 문구), vendor/(anime.js 4.5.0 MIT, Apache ECharts 6.1.0 Apache-2.0), data/(DS1–4 데이터셋 파일), pkg/·python/(빌드 산출)
data/raw/         SMT2020 배포본 SMT_2020 - Final 폴더 내용(AutoSched/, General Data/). 커밋 제외
```

- DES 코어(`des-core`): 사건 스케줄링 관점, 다음 사건 시각으로 시계 진행.
  - `Model`: 상태 + 초기화 루틴 `init`(t=0, 1회) + 사건 루틴 `handle`.
  - `Scheduler`: 시계 `now`, `schedule_at`·`schedule_in`, `stop`(현재 사건 후 실행 종료). 과거 시각 예약은 panic(인과성 위반).
  - `Simulation`: `run(until)`(`until` 이하 사건 처리 후 시계 = `until`, 정지·사건 소진 시 먼저 종료, 다음 실행이 이어서 진행), `run_observed(until, interval, 관찰자)`(`interval` 격자의 관측 사건열을 시각 순으로 병합, 일시정지 후에도 같은 격자, 관찰자가 중단 가능). `events_processed` = 모델 사건 수.
  - 미래 사건 목록: (시각, 예약 순번) 최소 힙(`BinaryHeap`, 동시각 FIFO, 페이로드 비교 없음).
- 데이터(`smt2020::data`·`asd`): `options.def`의 활성 파일(또는 지정 order 파일)만 읽어 `Dataset` 생성(이름 → 인덱스, 시간 ms, 날짜는 SIM_START 기준).
  - 지원 범위 밖 값·조합(예: STNCAP 1·2 외, MINRUN 외 setup 기준, SEQ_ADDS_SETUP_DELAYS Y)은 무시하지 않고 `파일:행` 오류.
  - 검증: 이름 참조(툴그룹·스텝·setup·캘린더·부품·위치), per_batch ⇔ 배치 TG(0 < BATCHMN ≤ BATCHMX), cascading 간격 ⇔ STNCAP 2(0 < c ≤ 최소 공정시간), LTL·CQT 대상은 뒤 스텝, 리워크 대상은 앞 스텝(확률 < 100%), CQT 시작·종료 스텝과 배치·setup run 스텝은 샘플링 100%, 배치·setup run 스텝은 리워크 루프 밖(대기 lot의 도착 보장), fromto 쌍·순위 중복 없음(FIFO·CR 동시 불가), 투입 ≥ SIM_START, 기간 오름차순.
- 시뮬레이션(`smt2020::sim`): `Simulation`이 데이터셋(`Arc`, 시뮬레이션 간 공유)·설정·pass·des-core 실행기를 소유하고 [실행 흐름](#실행-흐름)을 구현한다. 종료는 마지막 lot 완료 사건의 `stop`, 기한(종료 + 365 d)도 사건이다(상태 반복 확인 없음). 상태 조회는 모델을 읽기만 한다(집계 갱신 없음, 결과 불변).
  - 모듈: `fab`(모델·사건 처리·대기열 항목), `dispatch`(툴·lot 선택, 배치 구성, Stopping 보류 표시), `tool`(job 단계·cascading·정지·상태 집계), `status`(lot·툴·툴그룹 상태), `routes`(기대 스텝시간·잔여 작업·RPT·CQT 구간 TG 사전 계산), `plan`(투입 계획), `stats`(통계·결과·digest), `strategy`(전략·Stopping 해제 판정), `rng`.
- 측정값(`smt2020::report`): 결과 → 측정값(논문 단위), 복제 요약(평균·표본 표준편차·Student t 95% CI), CSV.
- 도메인 엔진: 엔티티 `Vec` + 인덱스 id(lot·툴·TG·스텝, route는 평탄 배열), `Event`는 작은 enum. 고장 중단 시 툴 epoch를 올려 기존 사건을 무효화(lazy deletion)하고 재스케줄.
- 대기열: TG별 `Vec` 항목. 도착 시 순위 입력(우선순위, 도착 시각, 납기, 잔여 작업, setup, LTL 전용 툴, 배치 키, CQT 긴급도 입력)을 고정해 디스패칭이 lot·route 자료 대신 연속 메모리를 순회한다. 유휴 툴은 TG별 FIFO.
- 디스패칭은 사건(도착, job 종료, 수리, PM 종료, 예약 해제, Stopping 임계 해제)에서만 실행한다.
- 전략: `enum` + `match`(고정 집합, 동적 디스패치 없음).
- 데이터셋 파일: postcard + 매직·형식 버전. 주기형 투입은 규칙만, 목록형·WIP는 lot 레코드(DS2·4 약 20만 lot). 브라우저는 xlsx를 읽지 않는다.
- 바인딩: 코어 메서드를 그대로 위임하고 값은 serde 스키마로 변환한다(JS: serde-wasm-bindgen JSON 호환 객체, `i64`는 경계에서 f64(2^53 ms까지 정확). 설정은 JSON 값을 거쳐 읽어 미지 필드를 검출(구조체 역직렬화는 알려진 속성만 읽음). Python: pythonize dict·list). 관찰자 반환값 `false`/`False`만 일시정지, 예외는 일시정지 후 전달. Python은 실행 중 GIL을 놓고 1일마다 다시 잡아 관찰자·Ctrl-C를 처리한다.
- 웹 모듈: `main.js`(실행 조율), `views.js`(해시 라우터, `hashchange`만), `setup.js`(시나리오 = 데이터셋·설정·복제 수, 데이터셋 파일 1회 fetch·디코딩해 `info()`, 설정 JSON, 실행 전 `new Simulation`으로 코어 검증), `strategy.js`(전략 편집기, 설정 객체를 직접 고침), `pool.js`·`worker.js`(작업 큐·워커), `progress.js`(진행·FAB 지도), `results.js`(개요), `details.js`(기록·재생·상세), `charts.js`(차트), `i18n.js`(`data-i18n` 문구, `data-i18n-attr` 속성), `motion.js`.
- 웹 실행: 워커 풀(`pool.js`, 최대 `navigator.hardwareConcurrency`개)이 작업(복제 실행·재생)을 차례로 맡긴다. 워커는 디코딩한 데이터셋을 보관해 같은 데이터셋의 바이트는 처음 한 번만 받는다. SharedArrayBuffer·wasm 스레드 미사용(GitHub Pages는 COOP/COEP 헤더 설정 불가). 워커의 관찰자는 직전 보고 후 250 ms(벽시계)가 지난 첫 1일 관측에서 일시정지하고, 워커는 진행·`toolGroups()`를 `postMessage`한 뒤 이어 실행한다(일시정지는 결과 불변, 타이머 없음). 메인 스레드는 복제별 상태를 갱신하고 프레임마다 최대 1회(`requestAnimationFrame`) 그린다. 취소는 작업 묶음 단위 `worker.terminate()`.
- 기록·재생: 복제 0은 위반·툴그룹 일별 기록과 함께 실행한다(결과 불변). 다른 복제는 같은 설정 + 측정된 QTS 흐름 계수로 1 pass 재생해 기록하고 digest를 비교한다. 구간 사건·lot 이력은 창 끝(`until`)까지만 재생한다. 기록은 최근 복제 3개까지 보관.
- 애니메이션: anime.js 4.5.0(MIT, `www/vendor/`에 동봉, 외부 CDN 미사용)으로 등장·수치 증가·설명 펼침. `prefers-reduced-motion`이면 생략(최종 화면 동일).
- 차트: Apache ECharts 6.1.0(`dist/echarts.esm.min.js`, Apache-2.0, ZRender BSD-3 포함, 라이선스 `www/vendor/echarts.LICENSE.txt`). 실행 시작 때 미리 불러온다(설정·실행 보기는 쓰지 않음). 색은 CSS 토큰, 상자 폭·색 구성이 바뀌면 새 인스턴스로 다시 그리고(애니메이션 없음), 상자가 페이지에서 빠지면 dispose한다(`ResizeObserver`). 확대(dataZoom), 커서 연동(`connect`), 범례 토글, 터치 툴팁.
- 다국어: 언어별 문구 파일(`www/locales/*.js`, `en.js`와 같은 키, `{이름}` 자리 표시, 빠진 키는 영어). 정적 요소는 `data-i18n` 키, 동적 문구는 `t()`, 숫자는 `Intl.NumberFormat`. 상태 문구·결과는 언어 전환 시 다시 그린다. 언어 추가 = 문구 파일 + `www/i18n.js`의 `LANGUAGES`·`MESSAGES` 등록.
- 빌드: release 프로필 `lto = true`, `codegen-units = 1`, `panic = "abort"`(네이티브 약 7% 단축, 결과 동일).
- 의존성: rand_xoshiro·libm·serde·postcard(`smt2020`), clap·serde_json(`smt2020-cli`), wasm-bindgen·js-sys·serde-wasm-bindgen·serde_json(`fab-wasm`), pyo3(abi3-py39)·pythonize(`smt2020-python`).

## 구현 단계

1. DES 코어(`des-core`) — 완료
2. 데이터 모델·`.asd` 로더(`smt2020`) — 완료
3. lot 흐름·툴 처리: 난수 스트림, 투입·반송·대기, 디스패칭(HP·RSETUP·FIFO·CR), 공정(lot·wafer·batch), cascading, 배치, setup·rule_LSSU·wake, LTL, 리워크, 샘플링, super hot 예약 — 완료
4. 가용성: UDT·PM — 완료
5. 통계: lot·툴·CQT 지표, 기간 — 완료
6. 운영 전략: QTCR·QTS·Stopping·EF·CAtE·CoT — 완료
7. `cli`: convert(.bin)·run·validate, 데이터셋 파일, 측정값·복제 요약, 사건 구동 종료·관측 — 완료
8. `wasm` API·웹 UI·결정성 확인·성능 측정 — 완료
9. 재사용 코어: `Simulation`(단계 실행·일시정지·상태 조회·재시작), JS·Python 포장(같은 API), wheel 배포 — 완료
10. 운영 전략 실험: 툴그룹별 순위 기준(`ranking`, 기준 11종), QT 배치 시작(깨우기 사건), 웜업 설정, 데이터셋 정보(`info()`), 웹 보기 분리(설정·실행·결과·Python)와 전략 편집기·설정 JSON — 완료
11. 분석 데이터: 구간별·스텝별 CQT 대기 분해, 일별 결과·요약, 진행의 CQT 누적, 기록(위반·TG 일별·사건 창)과 재생용 QTS FF — 완료
12. 분석 보기(드릴다운·시계열·히트맵), 13. 시나리오 비교(쌍대 CI·스윕), 14. 공유·게시 — 진행 예정

## 로컬 빌드·테스트

```bash
cargo test   # 기본 멤버(crates/python 제외)
cargo test -p smt2020 --release -- --ignored   # 실데이터 로드·데이터셋 파일 왕복, 동봉 데이터셋 파일 = 원천 변환, 4개 데이터셋 2년 계획 완료, 전략 완료(data/raw 필요)
cargo build --release -p smt2020-cli
# 데이터셋 파일(커밋 대상) 재생성: 로더·형식 변경 시
for n in 1 2 3 4; do target/release/smt2020 convert "$(ls -d "data/raw/AutoSched/dataset $n"/*/*.asd)" www/data/ds$n.bin; done
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129   # crates/wasm/Cargo.toml의 wasm-bindgen 버전과 같아야 함
cargo build --release --target wasm32-unknown-unknown -p fab-wasm
wasm-bindgen --target web --no-typescript --out-dir www/pkg target/wasm32-unknown-unknown/release/fab_wasm.wasm
node --test crates/wasm/tests/api.test.mjs   # JS API(www/pkg 필요)
pip install maturin
maturin build --release -m crates/python/Cargo.toml --out dist   # 현재 플랫폼 wheel
pip install --no-index --find-links dist smt2020
python -m unittest discover -s crates/python/tests   # Python API
python -m http.server -d www
```

`file://`로 열면 ES 모듈이 막히므로 정적 서버로 연다.

## 배포

`main`에 push하면 `.github/workflows/pages.yml`이 실행된다.

1. wheel: Linux(manylinux x86-64)·Windows(x64)·macOS(universal2)에서 maturin으로 빌드하고, 각 플랫폼에서 설치해 Python API 테스트를 돌린다.
2. 페이지: Rust 테스트(동봉 데이터셋 파일을 현재 빌드로 디코딩 포함), wasm 빌드, JS API 테스트 후 wheel을 `www/python/`에 모아 목록(`index.html`, 페이지의 내려받기·pip `--find-links` 공용)을 만들고 `www/`(데이터셋 파일 포함)를 GitHub Pages로 배포한다.
