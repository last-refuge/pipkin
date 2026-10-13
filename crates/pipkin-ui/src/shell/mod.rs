//! Workspace shell: navigation, header, composer chrome, inspector, overlays.

pub mod actions;
mod center;
mod clipboard_image;
pub mod commands;
pub mod controls;
mod inspector;
mod jobs;
mod mcp_settings;
mod nav;
mod onboarding;
mod overlays;
mod plan;
mod settings;
mod subagents;
mod workspace;

pub use workspace::Workspace;

use gpui::{
    App, AppContext as _, Bounds, Entity, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};

use crate::model::Model;
use crate::theme::Theme;

/// One-time application setup: fonts, key bindings, theme global.
pub fn init(cx: &mut App, model: &Entity<Model>) {
    if let Err(e) = crate::assets::load_fonts(cx) {
        log::error!("failed to load bundled fonts: {e:#}");
    }
    let prefs = model.read(cx).state.prefs.clone();
    cx.set_global(Theme::new(
        prefs.theme,
        prefs.text_size,
        prefs.reduced_motion,
    ));
    crate::text::init(cx);
    crate::transcript::init(cx);
    actions::bind_keys(cx);
}

/// Open the main Pipkin window.
pub fn open_main_window(cx: &mut App, model: Entity<Model>, data_dir: std::path::PathBuf) {
    init(cx, &model);
    reopen_main_window(cx, model, data_dir);
}

/// Recreate a closed Mac window without reinitializing fonts, shortcuts or the model.
pub fn reopen_main_window(cx: &mut App, model: Entity<Model>, data_dir: std::path::PathBuf) {
    // The size it was last left at, else the default.
    let (w, h) = model
        .read(cx)
        .state
        .prefs
        .window_size
        .unwrap_or((1440.0, 960.0));
    let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
    let m = model.clone();
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            app_id: Some("pipkin".into()),
            window_min_size: Some(size(px(480.), px(480.))),
            titlebar: Some(TitlebarOptions {
                title: Some("Pipkin".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        move |window, cx| cx.new(|cx| Workspace::new(m.clone(), data_dir.clone(), window, cx)),
    )
    .expect("open main window");
    cx.activate(true);
}
