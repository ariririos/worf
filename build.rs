// Credit to Polochon-street/bliss-rs

use std::env;
use std::process::Command;

fn main() {
    if cfg!(target_os = "linux") {
        let mut npm = Command::new("npm");
        npm.arg("run").arg("build");
        npm.current_dir("public");
        let status = npm.output().expect("Failed to run npm build");
        println!("cargo:warning=Running `npm run build`...");
        println!("cargo:warning=stdout: {}", String::from_utf8_lossy(&status.stdout));
        println!("cargo:warning=stderr: {}", String::from_utf8_lossy(&status.stderr));
        println!("cargo:warning=npm run build exited with status: {}", status.status);
    } else {
        println!("Run `npm run build` yourself!");
    }
    for (name, value) in env::vars() {
        if name.starts_with("DEP_FFMPEG_") {
            if value == "true" {
                println!(
                    r#"cargo:rustc-cfg=feature="{}""#,
                    name["DEP_FFMPEG_".len()..name.len()].to_lowercase()
                );
            }
            println!(
                r#"cargo:rustc-check-cfg=cfg(feature, values("{}"))"#,
                name["DEP_FFMPEG_".len()..name.len()].to_lowercase()
            );
        }
    }
}
