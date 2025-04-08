use super::CronJob;
use crate::{await_cancellable, config::DownloaderConfig};
use anyhow::bail;
use fs_extra::dir::CopyOptions;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;

pub struct UpdateGameData {
    downloader_path: PathBuf,
    output_path: PathBuf,
    config: DownloaderConfig,
}

impl UpdateGameData {
    pub fn new(config: DownloaderConfig) -> std::io::Result<Self> {
        let downloader_path = std::env::current_exe()?
            .with_file_name(format!("downloader{}", std::env::consts::EXE_SUFFIX,));
        if !downloader_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Downloader not found: {downloader_path:?}"),
            ));
        }
        let output_path = Path::new(&config.storage_dir).to_path_buf();
        std::fs::create_dir_all(&output_path)?;
        Ok(Self {
            downloader_path,
            output_path,
            config,
        })
    }
}

impl CronJob for UpdateGameData {
    const NAME: &'static str = "update_game_data";
    const PERIOD: Duration = Duration::from_secs(10 * 60);
    const TIMEOUT: Duration = Duration::from_secs(7 * 60);

    async fn run(&self, stop_signal: CancellationToken) -> anyhow::Result<()> {
        let mut cmd = Command::new(self.downloader_path.as_os_str());

        let latest_version_file = self.output_path.join("latest-ver.txt");
        let latest_version = std::fs::read_to_string(&latest_version_file).ok();
        let latest_path = self.output_path.join("latest");
        log::info!("Latest version: {:?}", latest_version);
        let _deleter = FolderDeleter::new(latest_path.clone());

        cmd.args(["--verbose", "true"])
            .args(["download"])
            .args(["-s", &self.config.slug])
            .args(["-f", &self.config.file_regex])
            .args(["-p", &self.config.parallelism.to_string()])
            .args([OsStr::new("-o"), latest_path.as_os_str()])
            .args([
                "-c",
                &format!("{}/{}", self.config.clut_path, self.config.slug),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        log::info!("Running: {:?}", cmd.as_std());

        let mut cmd = cmd.spawn()?;
        let mut stdout = cmd.stdout.take().unwrap();
        let mut stderr = cmd.stderr.take().unwrap();
        let status = await_cancellable!(cmd.wait(), stop_signal, {
            cmd.kill().await?;
        });
        drop(cmd);

        if !status.success() {
            let mut stdout_buf = String::new();
            let mut stderr_buf = String::new();

            if let Err(e) = stdout.read_to_string(&mut stdout_buf).await {
                log::error!("Failed to read stdout: {}", e);
                stdout_buf = "<failed to read stdout>".to_string();
            }

            if let Err(e) = stderr.read_to_string(&mut stderr_buf).await {
                log::error!("Failed to read stderr: {}", e);
                stderr_buf = "<failed to read stderr>".to_string();
            }

            bail!(
                "non-zero exit code: {}\nstdout:\n{}\nstderr:\n{}",
                status,
                stdout_buf,
                stderr_buf
            );
        }

        let mut is_updated = None;
        let mut installed_version = None;

        let mut out = BufReader::new(stdout).lines();
        loop {
            let line = match out.next_line().await? {
                None => break,
                Some(line) => line,
            };

            if let Some(line) = line.strip_prefix("[ERROR] ") {
                log::error!("{}", line);
                continue;
            }
            if let Some(line) = line.strip_prefix("[WARN] ") {
                log::warn!("{}", line);
                continue;
            }
            if let Some(line) = line.strip_prefix("[INFO] ") {
                log::info!("{}", line);
                continue;
            }
            if let Some(line) = line.strip_prefix("[VERBOSE] ") {
                log::trace!("{}", line);
                continue;
            }
            if let Some(line) = line.strip_prefix("[DEBUG] ") {
                log::debug!("{}", line);
                continue;
            }
            if let Some(line) = line.strip_prefix("[OUTPUT] ") {
                match line.split_once(" => ") {
                    Some(("GITHUB_OUTPUT", value)) => match value.split_once("=") {
                        Some(("updated", value)) => {
                            is_updated = Some(value.parse::<bool>()?);
                        }
                        Some(("version", value)) => {
                            installed_version = Some(value.to_owned());
                        }
                        _ => log::info!("{}", line),
                    },
                    _ => log::info!("{}", line),
                }
                continue;
            }
            log::error!("Unknown output: {}", line);
        }

        if is_updated.is_none() || installed_version.is_none() {
            bail!("Failed to parse output");
        }

        let is_updated = is_updated.unwrap();
        let installed_version = installed_version.unwrap();

        if is_updated {
            log::info!("Game data updated to {}", installed_version);
            let dest_path = self.output_path.join(&installed_version);
            if dest_path.exists() {
                log::info!("Removing old version: {}", installed_version);
                std::fs::remove_dir_all(&dest_path)?;
            }
            std::fs::create_dir_all(&dest_path)?;
            log::info!("Copying from {:?} to {:?}", latest_path, dest_path);
            fs_extra::dir::copy(
                &latest_path,
                &dest_path,
                &CopyOptions::new().content_only(true).overwrite(true),
            )?;
            log::info!("Copied to {}", installed_version);
            std::fs::write(&latest_version_file, &installed_version)?;
        } else {
            log::info!("Game data not updated; already at {}", installed_version);
        }

        _deleter.disable();
        Ok(())
    }
}

struct FolderDeleter(PathBuf);

impl FolderDeleter {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn disable(self) {
        std::mem::forget(self);
    }
}

impl Drop for FolderDeleter {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            log::error!("Failed to remove folder: {}", e);
        }
    }
}
