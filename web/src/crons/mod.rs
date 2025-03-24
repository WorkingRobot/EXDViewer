pub mod update_game_data;
pub use update_game_data::UpdateGameData;

use std::time::Duration;

use tokio_util::sync::CancellationToken;

pub trait CronJob {
    const NAME: &'static str;
    const PERIOD: Duration;
    const TIMEOUT: Duration = Duration::from_secs(30);

    fn run(
        &self,
        stop_signal: CancellationToken,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
}

pub fn create_cron_job<T: CronJob + Send + 'static>(job: T) -> CancellationToken {
    let stop_signal = CancellationToken::new();
    let signal = stop_signal.clone();
    tokio::spawn(async move {
        let new_job = job;
        loop {
            log::info!("Running cron job \"{}\"", T::NAME);
            let new_token = signal.child_token();
            let timer = tokio::time::Instant::now();
            let result = tokio::select! {
                r = new_job.run(new_token.clone()) => r,
                _ = tokio::time::sleep(T::TIMEOUT) => {
                    new_token.cancel();
                    log::error!("Cron job \"{}\" timed out", T::NAME);
                    Ok(())
                }
            };
            log::info!("Cron job \"{}\" took {:?}", T::NAME, timer.elapsed());
            if new_token.is_cancelled() {
                log::warn!("Cron job \"{}\" was cancelled", T::NAME);
            }
            if let Err(e) = result {
                log::error!("Cron job \"{}\" failed: {:?}", T::NAME, e);
            }

            tokio::select! {
                _ = signal.cancelled() => { break; }
                _ = tokio::time::sleep(T::PERIOD) => {}
            }
        }
    });
    stop_signal
}

#[macro_export]
macro_rules! await_cancellable {
    ($expr:expr, $signal:expr) => {
        await_cancellable!($expr, $signal, {})
    };

    ($expr:expr, $signal:expr, $cancelled:tt) => {{
        let result: anyhow::Result<Option<_>> = tokio::select! {
            s = $expr => Ok(Some(s?)),
            _ = $signal.cancelled() => {
                $cancelled;
                Ok(None)
            }
        };
        match result? {
            Some(s) => s,
            None => return Ok(()),
        }
    }};
}
