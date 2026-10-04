# academic-semiconductor-fab

반도체 FAB 시뮬레이터의 GitHub Pages 사이트. Rust → wasm(wasm-bindgen) + 정적 HTML.

https://code-gihan.github.io/academic-semiconductor-fab/

## 로컬 빌드

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129   # Cargo.toml의 wasm-bindgen 버전과 같아야 함
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen --target web --no-typescript --out-dir www/pkg target/wasm32-unknown-unknown/release/fab_wasm.wasm
```

`www/`를 정적 서버로 열어 확인한다(예: `python -m http.server -d www`). `file://`로 열면 ES 모듈이 막힌다.

## 배포

`main`에 push하면 `.github/workflows/pages.yml`이 빌드해 GitHub Pages로 배포한다.
