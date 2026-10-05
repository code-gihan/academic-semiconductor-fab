// 페이지의 한국어 문구. 키는 en.js와 같고, {name}은 페이지가 넣는 값이다.
export default {
  "meta.title": "SMT2020 FAB 시뮬레이터",
  "meta.description":
    "SMT2020 반도체 FAB 테스트베드를 브라우저에서 실행합니다. Rust 이산 사건 시뮬레이터를 WebAssembly로 컴파일했습니다.",

  "header.title": "SMT2020 FAB 시뮬레이터",
  "header.tagline":
    "SMT2020 반도체 FAB 테스트베드 4종을 시뮬레이션하고 운영 전략을 브라우저에서 바로 비교합니다.",
  "header.language": "언어",
  "header.source": "소스 코드",
  "intro.local":
    "Rust로 작성한 이산 사건 시뮬레이터가 이 브라우저에서 WebAssembly로 실행되며, 복제 1회에 CPU 스레드 1개를 씁니다. 아무것도 업로드하지 않습니다.",

  "dataset.heading": "데이터셋",
  "dataset.ds1.name": "DS1 · HV/LM",
  "dataset.ds1.description": "고물량·소품종: 제품 2종, 툴 1,043대, 주기 투입, FIFO 디스패칭.",
  "dataset.ds2.name": "DS2 · LV/HM",
  "dataset.ds2.description":
    "저물량·다품종: 제품 10종, 툴 913대, lot별 납기가 있는 투입 목록, CR(critical ratio) 디스패칭.",
  "dataset.ds3.name": "DS3 · HV/LM + 엔지니어링",
  "dataset.ds3.description": "DS1에 제품 1종의 엔지니어링 lot(주 40개)을 더함, 툴 1,135대.",
  "dataset.ds4.name": "DS4 · LV/HM + 엔지니어링",
  "dataset.ds4.description": "DS2에 제품 3종의 엔지니어링 lot(주 80개)을 더함, 툴 1,068대.",
  "dataset.file.name": "데이터셋 파일",
  "dataset.file.description": "smt2020 convert로 만든 .bin 파일(예: 모델의 다른 order 파일로 변환).",
  "dataset.file.label": "데이터셋 파일(.bin)",
  "dataset.common":
    "4종 공통: 툴그룹 105개(영역 11개), 주당 웨이퍼 10,000장 투입(25장짜리 생산 lot 400개), 초기 WIP.",

  "strategy.heading": "운영 전략",
  "strategy.queueTime.label": "CQT 디스패칭",
  "strategy.queueTime.none": "없음 (BASE)",
  "strategy.queueTime.qtcr": "QTCR: 대기 시간 임계 비율",
  "strategy.queueTime.qts": "QTS: 대기 시간 스케줄링",
  "strategy.queueTime.hint":
    "CQT(critical queue time)는 두 스텝 사이의 허용 대기 시간입니다. QTCR은 남은 작업에 비해 한도까지 남은 시간이 짧은 lot을 먼저 처리하고, QTS는 사전 실행에서 잰 흐름 계수로 스텝마다 기한을 정하므로 복제 1회가 약 2배 걸립니다.",
  "strategy.stopping.label": "Stopping",
  "strategy.stopping.none": "끔",
  "strategy.stopping.high": "high 한도 (90/130 · 90/220)",
  "strategy.stopping.medium": "medium 한도 (60/95 · 70/150)",
  "strategy.stopping.small": "small 한도 (50/85 · 55/125)",
  "strategy.stopping.hint":
    "CQT 구간의 스테퍼 툴그룹(LithoTrack_FE_95 · FE_115)에 대기·공정 중인 CQT lot(앞 한도), 또는 그리로 이동 중인 lot까지 더한 CQT lot(뒤 한도)이 너무 많으면 구간 시작에서 lot을 보류합니다.",
  "strategy.engineering.label": "엔지니어링 lot",
  "strategy.engineering.none": "BASE (엔지니어링 lot 없음)",
  "strategy.engineering.base": "BASE: 데이터의 우선순위",
  "strategy.engineering.ef": "EF: 엔지니어링 우선",
  "strategy.engineering.cate": "CAtE {cycle} h: 생산 {production} h, 엔지니어링 {engineering} h",
  "strategy.engineering.cateSet":
    "CAtE {cycle} h ({dataset}): 생산 {production} h, 엔지니어링 {engineering} h",
  "strategy.engineering.cot": "CoT {trigger}: 엔지니어링 lot {trigger}개 대기부터 우선",
  "strategy.engineering.hint":
    "생산 lot과 엔지니어링 lot이 FAB을 나눠 쓰는 방식입니다(DS3, DS4). EF는 모든 툴그룹에서 EHL > PHL > ERL > PRL 순으로 처리합니다. 스테퍼에서 CAtE는 생산·엔지니어링 구간을 번갈아 두고, CoT는 엔지니어링 lot이 N개 대기할 때까지 생산을 먼저 처리한 뒤 그 N개를 처리합니다.",
  "strategy.superHot.label": "super hot lot 툴 예약",
  "strategy.superHot.short": "super hot 예약",
  "strategy.superHot.hint":
    "super hot lot이 공정을 시작하면 다음 스텝의 툴 1대가 그 lot을 기다립니다(AutoSched rule_HotLotFIRST). AutoSched 모델에서는 꺼져 있습니다.",

  "settings.heading": "실행 설정",
  "settings.horizon.label": "종료 시각 (일)",
  "settings.horizon.hint":
    "2018-01-01부터 모의할 일수입니다. 첫해는 웜업이고 이후 해마다 보고하며, 종료 시각 전에 투입한 lot은 끝까지 완료합니다.",
  "settings.replications.label": "복제 수",
  "settings.replications.hint":
    "난수를 달리한 독립 실행으로, 이 기기에서 {threads}개씩 동시에 실행합니다. 결과는 평균과 95% 신뢰구간으로 보여 줍니다.",
  "settings.seed.label": "난수 seed",
  "settings.seed.hint":
    "seed와 복제 번호가 같은 실행은 같은 난수를 써서, 전략을 같은 조건에서 비교합니다.",
  "settings.load.label": "부하 계수",
  "settings.load.hint": "투입 속도 배율입니다. 1은 계획 투입량(주당 웨이퍼 10,000장), 0.9는 90%입니다.",

  "run.start": "시뮬레이션 실행",
  "run.cancel": "취소",
  "run.duration": "730일 복제 1회는 최근 데스크톱에서 약 15–25초, QTS는 약 2배 걸립니다.",

  "status.loadingWasm": "시뮬레이터를 불러오는 중…",
  "status.wasmFailed":
    "시뮬레이터를 불러오지 못했습니다({message}). 최신 Chrome, Edge, Firefox, Safari를 사용하세요.",
  "status.loadingDataset": "{dataset} 불러오는 중…",
  "status.starting": "실행을 시작하는 중…",
  "status.done": "{time} 만에 끝났습니다.",
  "status.cancelled": "취소했습니다.",
  "status.error": "실행하지 못했습니다: {message}",
  "error.noFile": "데이터셋 파일을 먼저 선택하세요.",
  "error.fetch": "{file}을(를) 불러오지 못했습니다(HTTP {status}).",
  "error.worker": "Web Worker 오류: {message}",

  "progress.done": "완료 복제 {done}/{count}",
  "progress.day": "{day}/{days}일",
  "progress.drain": "투입한 lot 마무리 중({wip}개 남음)",
  "progress.preRun": "QTS 사전 실행",
  "progress.mainRun": "QTS 본 실행",
  "progress.elapsed": "경과 {time}",
  "progress.remaining": "약 {time} 남음",

  "results.heading": "결과",
  "results.period": "보고 기간",
  "results.periodOption": "{name} · {start}–{end}일",
  "results.periodHint":
    "WarmUp: 첫해. Period_n: 둘째 해부터 n + 1번째 해까지 누적(종료 시각에서 끊음). Drain: 종료 시각 뒤에 완료한 lot.",
  "results.downloadJson": "JSON 내려받기",
  "results.downloadCsv": "CSV 내려받기",
  "results.ci": "값은 복제 평균이며, ± 뒤는 95% 신뢰구간의 반폭(Student t)입니다.",
  "results.kinds":
    "lot 유형: PRL 생산, PHL 생산 hot, SHL super hot, ERL 엔지니어링, EHL 엔지니어링 hot.",
  "results.reproduce.title": "재현성·성능",
  "results.reproduce.text":
    "digest가 같으면 결과가 비트 단위로 같습니다. JSON 파일의 복제별 config를 smt2020 run --config로 실행하면 같은 digest가 나옵니다.",
  "results.digest": "복제 {replication}: {digest} ({time})",
  "results.performance":
    "복제 {count}회 · 워커 {workers}개 · 전체 {time} · 복제당 {perReplication} · 초당 사건 {events}백만 개 · 최대 메모리 {memory} MB",

  "kpi.started": "투입 lot",
  "kpi.completed": "완료 lot",
  "kpi.wip": "평균 WIP (lot)",
  "kpi.prlCt": "PRL 사이클 타임 (일)",
  "kpi.prlOnTime": "PRL 납기 준수 (%)",
  "kpi.erlCt": "ERL 사이클 타임 (일)",
  "kpi.cqt": "CQT 위반 (%)",

  "table.kind.title": "lot 유형",
  "table.kind.item": "유형",
  "table.kind.note":
    "ACT: 투입부터 완료까지 평균 사이클 타임. 납기 준수: 납기 안에 완료한 비율. FF: 흐름 계수(사이클 타임 ÷ 순수 공정 시간).",
  "table.ff.title": "흐름 계수(FF) 분위수",
  "table.ff.item": "유형",
  "table.ff.note": "완료 lot의 흐름 계수 분포: P0 최솟값, P50 중앙값, P100 최댓값.",
  "table.lot.title": "제품 × lot 유형",
  "table.lot.item": "제품, 유형",
  "table.lot.note": "lot 유형 표의 지표를 제품별로 나눈 값.",
  "table.cqt.title": "CQT 구간",
  "table.cqt.item": "구간",
  "table.cqt.note":
    "%VL: 한도를 넘긴 완료 구간 비율. > 1 h·2 h·4 h: 그 시간보다 더 넘긴 비율. AVL·AONT: 완료 구간당 평균 초과·여유 시간. Litho: 스테퍼를 지나는 구간.",
  "table.area.title": "영역",
  "table.area.item": "영역",
  "table.area.note":
    "가용도: 고장·PM이 아닌 시간 비율. SDT 비중: 정지 시간 중 PM 비율. 가동률: setup·load·unload·공정. 최대: 가장 바쁜 툴그룹.",
  "table.toolGroup.title": "툴그룹 (가동률 순)",
  "table.toolGroup.item": "툴그룹",
  "table.toolGroup.note": "툴 시간 중 상태별 비율(%).",

  "col.completed": "완료",
  "col.ctMean": "ACT (일)",
  "col.ctStd": "CT 표준편차 (일)",
  "col.onTime": "납기 준수 (%)",
  "col.ffMean": "평균 FF",
  "col.percentile": "P{p}",
  "col.vl": "%VL",
  "col.vl1h": "> 1 h (%)",
  "col.vl2h": "> 2 h (%)",
  "col.vl4h": "> 4 h (%)",
  "col.avl": "AVL (h)",
  "col.aont": "AONT (h)",
  "col.availability": "가용도 (%)",
  "col.sdtShare": "SDT 비중 (%)",
  "col.util": "가동률 (%)",
  "col.utilMax": "최대 (%)",
  "col.down": "고장",
  "col.pm": "PM",
  "col.setup": "Setup",
  "col.process": "공정",
  "col.load": "Load",
  "col.unload": "Unload",
  "col.idle": "유휴",

  "cqt.litho": "Litho",
  "cqt.rest": "그 외",
  "cqt.total": "전체",

  "common.on": "켬",
  "common.off": "끔",

  "footer.references": "참고 문헌",
  "footer.data":
    "데이터: SMT2020 testbed release 1.0(2020), FernUniversität in Hagen. 데이터셋 파일은 그 AutoSched AP 모델을 변환한 것입니다.",
};
