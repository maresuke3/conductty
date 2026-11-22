use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico_path = manifest_dir.join("../../assets/app-icon.ico");
    println!("cargo:rerun-if-changed={}", ico_path.display());

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") || !ico_path.is_file() {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc_path = out_dir.join("app-icon.rc");
    let ico_escaped = ico_path.to_string_lossy().replace('\\', "/");
    if std::fs::write(&rc_path, format!("1 ICON \"{ico_escaped}\"\n")).is_err() {
        return;
    }

    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "gnu" {
        let obj_path = out_dir.join("app-icon.o");
        for tool in ["x86_64-w64-mingw32-windres", "windres"] {
            if let Ok(status) = Command::new(tool)
                .args([
                    rc_path.to_str().unwrap(),
                    "-O",
                    "coff",
                    "-o",
                    obj_path.to_str().unwrap(),
                ])
                .status()
            {
                if status.success() {
                    println!("cargo:rustc-link-arg-bin=conductty={}", obj_path.display());
                    break;
                }
            }
        }
    } else if target_env == "msvc" {
        let res_path = out_dir.join("app-icon.res");
        if let Ok(status) = Command::new("rc.exe")
            .args([
                "/nologo",
                &format!("/fo{}", res_path.display()),
                rc_path.to_str().unwrap(),
            ])
            .status()
        {
            if status.success() {
                println!("cargo:rustc-link-arg-bin=conductty={}", res_path.display());
            }
        }
    }
}
