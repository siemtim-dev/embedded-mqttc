use std::{env, fs::File, io::Write, path::PathBuf};

const ENV_VARS: &[&'static str] = &[
    "WIFI_SSID",
    "WIFI_PASSWORD",

    "MQTT_HOST",
    "MQTT_PORT",
    "MQTT_CLIENT_ID",
    "MQTT_USERNAME",
    "MQTT_PASSWORD",

    "MQTT_SUBSCIBE_TO",
    "MQTT_PUBLISH_TO",
];

fn load_env() {
    // Lade .env-Datei
    if let Ok(dotenv_path) = dotenvy::dotenv() {
        println!("cargo:rerun-if-changed={}", dotenv_path.display());
    }

    // Jetzt sind alle Variablen aus .env als Umgebungsvariablen gesetzt
    // Wir können sie weiterreichen an den Compiler
    

    for env_var_name in ENV_VARS {
        if let Ok(env_var_value) = env::var(env_var_name) {
        println!("cargo:rustc-env={}={}", env_var_name, env_var_value);
    }
    }
}

fn main() {
    load_env();

    // Put `memory.x` in our output directory and ensure it's
    // on the linker search path.
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());
    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(include_bytes!("memory.x"))
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    // By default, Cargo will re-run a build script whenever
    // any file in the project changes. By specifying `memory.x`
    // here, we ensure the build script is only re-run when
    // `memory.x` is changed.
    println!("cargo:rerun-if-changed=memory.x");

    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tlink-rp.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");

}