use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    println!("cargo:rerun-if-env-changed=DEP_OPENSSL_VERSION_NUMBER");
    assert_eq!(
        env::var("DEP_OPENSSL_VERSION_NUMBER").as_deref(),
        Ok("30600040"),
        "Linux requires OpenSSL 3.6.4. Use mise exec -- bash scripts/cargo.sh <command>."
    );
    let target = env::var("TARGET").expect("Cargo target");
    let static_key = format!("{}_OPENSSL_STATIC", target.to_uppercase().replace('-', "_"));
    println!("cargo:rerun-if-env-changed={static_key}");
    assert_eq!(
        env::var(&static_key).as_deref(),
        Ok("1"),
        "Linux requires static OpenSSL. Use mise exec -- bash scripts/cargo.sh <command>."
    );
}
