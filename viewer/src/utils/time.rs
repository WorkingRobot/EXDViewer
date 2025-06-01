#[cfg(target_arch = "wasm32")]
pub fn now() -> f64 {
    use web_sys::window;

    window().unwrap().performance().unwrap().now()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn now() -> f64 {
    use std::time::SystemTime;

    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}
