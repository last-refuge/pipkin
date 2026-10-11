use gpui::App;
use gpui_platform::application;

mod adapters;
mod cache;
mod controller;
mod diagnostics;
mod install;
mod launch;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod native_menu;
mod platform;
mod storage;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    // The private engine worker: serve the engine over stdin/stdout and exit, before anything
    // else (no options parsing, no storage, no GPUI) runs in this process.
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if let Some(worker) = adapters::native::supervisor::worker_args(&raw) {
        match worker {
            Ok(args) => std::process::exit(adapters::native::supervisor::run_worker(args)),
            Err(message) => {
                eprintln!("pipkin: {message}");
                std::process::exit(2);
            }
        }
    }
    let options = match controller::Options::from_env_args() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("pipkin: {message}");
            eprintln!(
                "usage: pipkin [--version] [--diagnose [--probe]] [--pi-dir <path>] [--pi-server-id <uuid>] [--data-dir <path>]\n       pipkin --engine native [--engine-host child|inprocess] [--data-dir <path>]\n       pipkin --demo <scenario> [--data-dir <path>] [--speed <factor>]"
            );
            std::process::exit(2);
        }
    };
    match options.action {
        controller::Action::Run => {}
        controller::Action::Version => {
            println!(
                "pipkin {} (source {}, dirty: {}, protocol {}, schema {})",
                install::VERSION,
                install::APP_REVISION,
                install::APP_DIRTY,
                install::PROTOCOL,
                storage::SCHEMA_VERSION
            );
            return;
        }
        controller::Action::Diagnose { probe } => {
            let data_dir = platform::data_dir(options.data_dir.as_deref());
            let (text, bad) = install::diagnose(&data_dir, options.pi_repo.as_deref(), probe);
            print!("{text}");
            std::process::exit(if bad { 1 } else { 0 });
        }
    }
    let data_dir = platform::data_dir(options.data_dir.as_deref());
    let app = application().with_assets(pipkin_ui::assets::Assets);
    #[cfg(target_os = "macos")]
    app.on_reopen(native_menu::show_main_window);
    app.run(move |cx: &mut App| {
        let model = controller::start(cx, options);
        pipkin_ui::shell::open_main_window(cx, model.clone(), data_dir.clone());
        #[cfg(target_os = "macos")]
        native_menu::install(cx, model, data_dir);
    });
}
