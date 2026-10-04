use wasm_bindgen::prelude::*;

/// Smoke test for the JS <-> Rust round trip; replace with the simulator API.
#[wasm_bindgen]
pub fn greet(name: &str) -> String {
    format!("Hello, {name}! This string was built by Rust/wasm.")
}
