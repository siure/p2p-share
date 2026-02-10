use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/jni_bridge.c");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("android") {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let obj = out_dir.join("jni_bridge.o");

    let compiler = cc::Build::new().get_compiler();
    let mut cmd: Command = compiler.to_command();
    cmd.arg("-c").arg("src/jni_bridge.c").arg("-o").arg(&obj);

    let status = cmd
        .status()
        .expect("failed to run C compiler for JNI bridge");
    assert!(status.success(), "C compiler failed for JNI bridge");

    println!(
        "cargo:warning=android-bindings linked object {}",
        obj.display()
    );
    println!("cargo:rustc-link-arg-cdylib={}", obj.display());
    println!("cargo:rustc-link-arg-cdylib=-Wl,-u,JNI_OnLoad");
}
