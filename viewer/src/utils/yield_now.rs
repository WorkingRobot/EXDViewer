// From futures-lite 2.6.0

use std::{
    pin::Pin,
    task::{Context, Poll},
};

pub fn yield_now() -> YieldNow {
    YieldNow(false)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn yield_to_ui() {
    yield_now().await;
}

#[cfg(target_arch = "wasm32")]
pub async fn yield_to_ui() {
    use eframe::wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::window;

    let promise = web_sys::js_sys::Promise::new(&mut |resolve, _| {
        let closure = Closure::once_into_js(move || {
            resolve.call0(&JsValue::NULL).unwrap();
        });
        window()
            .unwrap()
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                0,
            )
            .unwrap();
    });

    let _ = JsFuture::from(promise).await;
}

/// Future for the [`yield_now()`] function.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.0 {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}
