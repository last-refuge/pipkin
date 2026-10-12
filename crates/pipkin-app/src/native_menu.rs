//! Native macOS integration. Other platforms keep their existing menus, keys and quit mode.
use gpui::{
    App, Entity, Global, KeyBinding, Menu, MenuItem, OsAction, PromptLevel, SystemMenuType, actions,
};
use pipkin_ui::{model::Model, shell::actions as shell, text::actions as text};
use std::path::PathBuf;

actions!(
    pipkin_native,
    [
        About,
        Hide,
        HideOthers,
        ShowAll,
        ShowMainWindow,
        CloseWindow,
        Minimize,
        Zoom,
        Help
    ]
);

struct MainWindow {
    // Keep the live model (and dirty drafts) alive after the last Mac window closes.
    model: Entity<Model>,
    data_dir: PathBuf,
}
impl Global for MainWindow {}

pub fn show_main_window(cx: &mut App) {
    if let Some(window) = cx.windows().first().copied() {
        let _ = window.update(cx, |_, window, _| window.activate_window());
    } else if let Some(state) = cx.try_global::<MainWindow>() {
        let model = state.model.clone();
        let data_dir = state.data_dir.clone();
        pipkin_ui::shell::reopen_main_window(cx, model, data_dir);
    }
    cx.activate(true);
    refresh_menus(cx);
}

fn refresh_menus(cx: &mut App) {
    cx.set_menus(menus_for_window(!cx.windows().is_empty()));
}

fn menus_for_window(has_window: bool) -> Vec<Menu> {
    let mut menus = menus();
    for menu in &mut menus {
        for item in &mut menu.items {
            if let MenuItem::Action {
                action, disabled, ..
            } = item
                && (menu.name.as_ref() == "Edit"
                    || action.as_any().is::<CloseWindow>()
                    || action.as_any().is::<Minimize>()
                    || action.as_any().is::<Zoom>())
            {
                *disabled = !has_window;
            }
        }
    }
    menus
}

fn menus() -> Vec<Menu> {
    vec![
        Menu::new("Pipkin").items([
            MenuItem::action("About Pipkin", About),
            MenuItem::separator(),
            MenuItem::action("Settings…", shell::OpenPreferences),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Pipkin", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Pipkin", shell::Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Conversation", shell::NewConversation),
            MenuItem::action("Open Project…", shell::OpenProject),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", text::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", text::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", text::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", text::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", text::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", text::SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Show Pipkin", ShowMainWindow),
        ]),
        Menu::new("Help").items([MenuItem::action("Pipkin Help", Help)]),
    ]
}

pub fn install(cx: &mut App, model: Entity<Model>, data_dir: PathBuf) {
    cx.set_global(MainWindow { model, data_dir });
    // Global Quit also works with no window/focus. It runs the controller's existing orderly
    // on_app_quit hook: flush dirty drafts, stop only our owned engine, drain/join storage.
    cx.on_action(|_: &shell::Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &ShowMainWindow, cx| show_main_window(cx));
    // Composer consumes these actions itself. A transcript uses its own selection model;
    // delegate only if that focused view can handle it, using its existing selection semantics.
    cx.on_action(|_: &text::Copy, cx| {
        let action = pipkin_ui::transcript::view::CopySelection;
        if cx.is_action_available(&action) {
            cx.dispatch_action(&action);
        }
    });
    cx.on_action(|_: &text::SelectAll, cx| {
        let action = pipkin_ui::transcript::view::SelectAllText;
        if cx.is_action_available(&action) {
            cx.dispatch_action(&action);
        }
    });
    cx.on_action(|_: &Help, cx| {
        cx.open_url("https://github.com/last-refuge/pipkin/blob/main/docs/getting-started.md")
    });
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
    });
    cx.on_action(|_: &Minimize, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(|_: &Zoom, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.zoom_window());
        }
    });
    cx.on_action(|_: &shell::OpenPreferences, cx| {
        show_main_window(cx);
        if let Some(window) = cx.windows().first().copied() {
            let _ = window.update(cx, |_, window, _| {
                window.on_next_frame(|window, cx| {
                    window.dispatch_action(Box::new(shell::OpenPreferences), cx);
                });
            });
        }
    });
    cx.on_action(|_: &About, cx| {
        show_main_window(cx);
        if let Some(window) = cx.windows().first().copied() {
            let _ = window.update(cx, |_, window, cx| {
                let detail = format!("Version {}\nNative coding with the built-in Rust engine.\n© Pipkin contributors · MIT license", crate::install::VERSION);
                let response = window.prompt(PromptLevel::Info, "About Pipkin", Some(&detail), &["OK"], cx);
                cx.spawn(async move |_| { let _ = response.await; }).detach();
            });
        }
    });
    cx.bind_keys(bindings());
    cx.on_window_closed(|cx, _| refresh_menus(cx)).detach();
    refresh_menus(cx);
}

fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-q", shell::Quit, None),
        KeyBinding::new("cmd-,", shell::OpenPreferences, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-n", shell::NewConversation, None),
        KeyBinding::new("cmd-shift-o", shell::OpenProject, None),
        KeyBinding::new("cmd-k", shell::OpenPalette, None),
        KeyBinding::new("cmd-c", text::Copy, Some("Composer")),
        KeyBinding::new("cmd-x", text::Cut, Some("Composer")),
        KeyBinding::new("cmd-v", text::Paste, Some("Composer")),
        KeyBinding::new("cmd-a", text::SelectAll, Some("Composer")),
        KeyBinding::new("cmd-z", text::Undo, Some("Composer")),
        KeyBinding::new("cmd-shift-z", text::Redo, Some("Composer")),
        KeyBinding::new(
            "cmd-c",
            pipkin_ui::transcript::view::CopySelection,
            Some("Transcript"),
        ),
        KeyBinding::new(
            "cmd-a",
            pipkin_ui::transcript::view::SelectAllText,
            Some("Transcript"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_window_keeps_quit_and_reopen_available_but_disables_window_and_edit_commands() {
        for has_window in [false, true] {
            let menus = menus_for_window(has_window);
            for menu in menus {
                for item in menu.items {
                    if let MenuItem::Action {
                        action, disabled, ..
                    } = item
                    {
                        if action.as_any().is::<shell::Quit>()
                            || action.as_any().is::<ShowMainWindow>()
                        {
                            assert!(!disabled);
                        } else if menu.name.as_ref() == "Edit"
                            || action.as_any().is::<CloseWindow>()
                            || action.as_any().is::<Minimize>()
                            || action.as_any().is::<Zoom>()
                        {
                            assert_eq!(disabled, !has_window);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn command_q_dispatches_the_same_orderly_quit_as_the_menu() {
        let quit = bindings()
            .into_iter()
            .find(|b| b.action().as_any().is::<shell::Quit>())
            .unwrap();
        assert_eq!(
            quit.keystrokes(),
            KeyBinding::new("cmd-q", shell::Quit, None).keystrokes()
        );
    }

    #[test]
    fn mac_menu_has_standard_order_services_and_real_quit_action() {
        let menus = menus();
        assert_eq!(
            menus.iter().map(|m| m.name.as_ref()).collect::<Vec<_>>(),
            ["Pipkin", "File", "Edit", "Window", "Help"]
        );
        assert!(menus[0].items.iter().any(
            |i| matches!(i, MenuItem::SystemMenu(m) if m.menu_type == SystemMenuType::Services)
        ));
        match menus[0].items.last().unwrap() {
            MenuItem::Action {
                name,
                action,
                disabled,
                ..
            } => {
                assert_eq!(name.as_ref(), "Quit Pipkin");
                assert!(action.as_any().is::<shell::Quit>());
                assert!(!disabled);
            }
            _ => panic!("application menu must end with Quit"),
        }
    }
}
