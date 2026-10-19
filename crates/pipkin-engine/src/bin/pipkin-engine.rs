//! Headless engine host: serves the engine protocol over stdin/stdout frames.
//!
//! `pipkin-engine --data-dir <dir> --config-dir <dir> [--default-model provider/id] [--listen <private-dir>]`
//!
//! The desktop normally runs the same code as a private worker mode of its own executable
//! (`pipkin --engine-worker`); this binary exists for headless qualification and tests and
//! needs no GUI libraries, Node or the legacy engine.

use std::path::PathBuf;

fn main() {
    if let Some(code) =
        pipkin_engine::execution::claude_cli::orchestration_helper::maybe_run_from_env()
    {
        std::process::exit(code);
    }
    if let Some(code) =
        pipkin_engine::execution::claude_cli::permission_helper::maybe_run_from_env()
    {
        std::process::exit(code);
    }
    env_logger_init();
    let mut data: Option<PathBuf> = None;
    let mut config: Option<PathBuf> = None;
    let mut model: Option<String> = None;
    let mut listen: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => data = args.next().map(PathBuf::from),
            "--config-dir" => config = args.next().map(PathBuf::from),
            "--default-model" => model = args.next(),
            "--listen" => {
                let Some(path) = args
                    .next()
                    .filter(|v| !v.is_empty() && !v.starts_with("--"))
                else {
                    eprintln!("pipkin-engine: --listen needs a private endpoint directory");
                    std::process::exit(2);
                };
                if listen.replace(PathBuf::from(path)).is_some() {
                    eprintln!("pipkin-engine: --listen may only be specified once");
                    std::process::exit(2);
                }
            }
            "--version" => {
                println!(
                    "pipkin-engine {} (protocol {}, schema {})",
                    env!("CARGO_PKG_VERSION"),
                    pipkin_engine_protocol::PROTOCOL_VERSION,
                    pipkin_engine::store::SCHEMA_VERSION
                );
                return;
            }
            other => {
                eprintln!("pipkin-engine: unknown argument {other:?}");
                std::process::exit(2);
            }
        }
    }
    let (Some(data), Some(config)) = (data, config) else {
        eprintln!(
            "usage: pipkin-engine --data-dir <dir> --config-dir <dir> [--default-model p/id] [--listen <private-dir>]"
        );
        std::process::exit(2);
    };
    let mut cfg = pipkin_engine::config::EngineConfig::new(data, config);
    cfg.default_model = model;
    std::process::exit(match listen {
        Some(directory) => pipkin_engine::endpoint::run(cfg, &directory),
        None => pipkin_engine::host::run_stdio(cfg),
    });
}

fn env_logger_init() {
    // Log level from PIPKIN_ENGINE_LOG (error|warn|info|debug); logs go to stderr only.
    struct Stderr(log::LevelFilter);
    impl log::Log for Stderr {
        fn enabled(&self, m: &log::Metadata) -> bool {
            m.level() <= self.0
        }
        fn log(&self, r: &log::Record) {
            if self.enabled(r.metadata()) {
                eprintln!("[{} {}] {}", r.level(), r.target(), r.args());
            }
        }
        fn flush(&self) {}
    }
    let level = match std::env::var("PIPKIN_ENGINE_LOG").as_deref() {
        Ok("debug") => log::LevelFilter::Debug,
        Ok("info") => log::LevelFilter::Info,
        Ok("error") => log::LevelFilter::Error,
        _ => log::LevelFilter::Warn,
    };
    let logger: &'static Stderr = Box::leak(Box::new(Stderr(level)));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(level);
    }
}
