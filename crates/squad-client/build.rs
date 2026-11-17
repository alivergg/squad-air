use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=../../assets/icons/squad-air.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR missing"));
    let obj_path = out_dir.join("squad_app_icon.o");

    let windres = [
        "/opt/homebrew/bin/x86_64-w64-mingw32-windres",
        "x86_64-w64-mingw32-windres",
    ]
    .into_iter()
    .find(|bin| {
        Command::new(bin)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
    .expect("x86_64-w64-mingw32-windres not found");

    let status = Command::new(windres)
        .args(["app.rc", "-O", "coff", "-o"])
        .arg(&obj_path)
        .status()
        .expect("failed to execute x86_64-w64-mingw32-windres");

    assert!(status.success(), "windres failed for squad-client app.rc");
    println!("cargo:rustc-link-arg-bins={}", obj_path.display());
}
