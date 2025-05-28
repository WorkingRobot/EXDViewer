use std::{
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::SystemTime,
};

fn main() {
    {
        let profile = std::env::var("PROFILE").unwrap();
        println!("cargo:rustc-env=PROFILE={profile}");
        if profile == "release" {
            println!(
                "cargo::rustc-env=BUILD_TIMESTAMP={}",
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            );
        } else {
            println!("cargo::rustc-env=BUILD_TIMESTAMP=0");
        }
    }

    // Skip downloading the downloader if we're running in rust-analyzer or anywhere unnecessary
    let is_redundant = cfg!(clippy) || cfg!(miri) || cfg!(doc) || cfg!(test) || cfg!(rustfmt);
    let is_rust_analyzer = if cfg!(windows) || cfg!(target_os = "linux") {
        is_under_rust_analyzer()
    } else {
        false
    };

    if !is_redundant && !is_rust_analyzer {
        download_downloader();
        build_frontend();
    }
}

#[cfg(target_os = "linux")]
fn is_under_rust_analyzer() -> bool {
    use procfs::process::Process;

    let mut current = Process::myself().expect("Failed to get current process");
    loop {
        let parent_id = current.stat().expect("Failed to get process stat").ppid;
        current = match Process::new(parent_id) {
            Ok(p) => p,
            Err(_) => break,
        };

        if PathBuf::from(
            current
                .stat()
                .expect("Failed to get parent process stat")
                .comm,
        )
        .components()
        .any(|p| p.as_os_str().eq_ignore_ascii_case("rust-analyzer"))
        {
            return true;
        }
    }

    false
}

#[cfg(windows)]
fn is_under_rust_analyzer() -> bool {
    std::env::var("_NT_SYMBOL_PATH").is_ok_and(|v| v.contains("rust-analyzer"))
}

fn download_downloader() {
    let exe_suffix = match std::env::var("CARGO_CFG_TARGET_OS")
        .expect("Could not get target os")
        .as_str()
    {
        "windows" => ".exe",
        "linux" => "",
        _ => panic!("Unsupported OS"),
    };
    copy_executable_to(
        &get_output_directory().join(format!("downloader{exe_suffix}")),
        || {
            let ret = ureq::get(format!("https://github.com/WorkingRobot/ffxiv-downloader/releases/latest/download/FFXIVDownloader.Command{exe_suffix}")).call().expect("Could not download downloader");
            if ret.status().is_success() {
                // 10 MB limit
                ret.into_body()
                    .read_to_vec()
                    .expect("Could not read downloader")
            } else {
                panic!("Could not download downloader")
            }
        },
    );
}

// Build egui frontend
fn build_frontend() {
    println!("cargo:rerun-if-changed=../viewer");

    let dist_dir = get_output_directory().join("static");
    let mut command = Command::new("trunk");
    command
        .env("CARGO_TARGET_DIR", "../target/frontend")
        .arg("build")
        .args(["--config", "../viewer"])
        .args([OsStr::new("-d"), dist_dir.as_os_str()]);
    if std::env::var("PROFILE").unwrap() == "release" {
        command.arg("--release").args(["-M", "true"]);
    }

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    println!("Running: {:?}", command);
    let child = command
        .spawn()
        .expect("Could not start frontend build process");
    let output = child.wait_with_output().expect("Could not build frontend");
    if output.status.success() {
        println!("Frontend built successfully");
    } else {
        println!("Error code: {}", output.status);
        println!("stdout:\n{}", String::from_utf8_lossy(&output.stdout));
        println!("stderr:\n{}", String::from_utf8_lossy(&output.stderr));
        panic!("Could not build frontend");
    }
}

// Modified from https://github.com/samwoodhams/copy_to_output
pub fn get_output_directory() -> PathBuf {
    let env_target = std::env::var("TARGET").expect("Could not get TARGET");
    let env_out_dir = std::env::var("OUT_DIR").expect("Could not get OUT_DIR");
    let env_profile = std::env::var("PROFILE").expect("Could not get PROFILE");

    let mut out_path = PathBuf::new();
    let mut cargo_target = String::new();

    let target_dir = {
        let mut target_dir = None;
        let mut sub_path = Path::new(&env_out_dir);
        while let Some(parent) = sub_path.parent() {
            if parent.ends_with(&env_profile) {
                target_dir = Some(parent);
                break;
            }
            sub_path = parent;
        }
        target_dir.map(|p| p.to_path_buf())
    };

    if let Some(target_dir) = target_dir {
        cargo_target.push_str(
            target_dir
                .to_str()
                .expect("Could not convert file path to string"),
        );
    }

    out_path.push(&cargo_target);

    if env_out_dir.contains(&format!(
        "{}{}{}",
        cargo_target,
        std::path::MAIN_SEPARATOR,
        env_target
    )) {
        out_path.push(env_target);
    }

    out_path
}

pub fn copy_executable_to(out_path: &Path, data_getter: impl FnOnce() -> Vec<u8>) {
    if !std::fs::exists(out_path).expect("Could not check if path exists") {
        let data = data_getter();
        let mut file = std::fs::File::create(out_path).expect("Could not open path");
        if cfg!(unix) {
            use std::os::unix::prelude::PermissionsExt;
            let mut perms = file.metadata().unwrap().permissions();
            // Make the file executable for those with read perms
            perms.set_mode(perms.mode() | ((perms.mode() & 0o444) >> 2));
            file.set_permissions(perms)
                .expect("Could not set permissions");
        }
        file.write_all(&data).expect("Could not write to file");
    }
}
