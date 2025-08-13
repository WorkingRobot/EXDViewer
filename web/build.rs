use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use shadow_rs::ShadowBuilder;

fn main() {
    ShadowBuilder::builder().build().unwrap();

    if std::env::var_os("CARGO_FEATURE_TRUNK_ASSETS").is_some() {
        build_frontend();
    }
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
    println!("Running: {command:?}");
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
