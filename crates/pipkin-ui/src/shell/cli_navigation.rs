//! Keyboard-only movement in CLI dialogs. Never dispatches a decision, probe or command.
use super::workspace::{Overlay, Workspace};
use gpui::{App, Context, KeyDownEvent, ScrollHandle, Window, px};

pub(super) fn scroll_keys(
    event: &KeyDownEvent,
    scroll: &ScrollHandle,
    edges: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let m = event.keystroke.modifiers;
    if m.control || m.alt || m.platform {
        return;
    }
    let mut p = scroll.offset();
    let page = (scroll.bounds().size.height * 0.8).max(px(32.));
    match event.keystroke.key.as_str() {
        "pageup" => p.y += page,
        "pagedown" => p.y -= page,
        "home" if edges => p.y = px(0.),
        "end" if edges => p.y = -scroll.max_offset().y,
        "up" if edges => p.y += px(24.),
        "down" if edges => p.y -= px(24.),
        "left" if edges => p.x += px(48.),
        "right" if edges => p.x -= px(48.),
        _ => return,
    }
    p.y = p.y.clamp(-scroll.max_offset().y, px(0.));
    p.x = p.x.clamp(-scroll.max_offset().x, px(0.));
    scroll.set_offset(p);
    window.refresh();
    cx.stop_propagation();
}
impl Workspace {
    /// Restore only a lost modal focus after asynchronous expiry/removal. Never steal
    /// focus that the user moved, switch overlays, or focus an actionable control.
    pub(super) fn retain_cli_navigation(&self, window: &mut Window, cx: &mut Context<Self>) {
        let overlay = self.overlay;
        if !matches!(overlay, Overlay::LocalCli | Overlay::CliApprovals)
            || !self.menu_focus.contains_focused(window, cx)
        {
            return;
        }
        let Some(focused) = window.focused(cx) else {
            return;
        };
        let this = cx.entity().downgrade();
        // The pinned GPUI runs frame callbacks before drawing. Cross a frame before
        // examining the rendered focus tree (not the tree we are about to replace).
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |window, cx| {
                let _ = this.update(cx, |w, cx| {
                    if w.overlay == overlay
                        && window.focused(cx).as_ref() == Some(&focused)
                        && !w.menu_focus.contains_focused(window, cx)
                    {
                        window.focus(&w.menu_focus, cx);
                    }
                });
            });
        });
    }

    pub(super) fn cli_dialog_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.overlay, Overlay::LocalCli | Overlay::CliApprovals) {
            return;
        }
        if self.overlay == Overlay::LocalCli
            && (self.local_cli_advanced || !self.state(cx).local_cli.guided)
            && (self.local_cli_executable.read(cx).is_composing()
                || self.local_cli_project.read(cx).is_composing())
        {
            // A propagated Tab can become literal text input and commit/replace the
            // marked range. Do not navigate or mutate composition here.
            if event.keystroke.key == "tab" {
                cx.stop_propagation();
            }
            return;
        }
        let m = event.keystroke.modifiers;
        if self.overlay == Overlay::CliApprovals
            && event.keystroke.key == "c"
            && m.control
            && m.shift
            && !m.alt
            && !m.platform
        {
            self.copy_current_cli_review(cx);
            window.prevent_default();
            cx.stop_propagation();
            return;
        }
        if event.keystroke.key == "tab" && !m.control && !m.alt && !m.platform {
            // Native tab order includes the workspace behind this modal. Skip it without
            // delivering input there. A bound protects against malformed/empty focus trees.
            for _ in 0..512 {
                if m.shift {
                    window.focus_prev(cx);
                } else {
                    window.focus_next(cx);
                }
                if self.menu_focus.contains_focused(window, cx) {
                    cx.stop_propagation();
                    return;
                }
            }
            window.focus(&self.menu_focus, cx);
            cx.stop_propagation();
        } else {
            let scroll = if self.overlay == Overlay::CliApprovals
                && self.cli_review_focus.contains_focused(window, cx)
            {
                &self.cli_review_scroll
            } else {
                &self.cli_dialog_scroll
            };
            scroll_keys(
                event,
                scroll,
                self.overlay == Overlay::CliApprovals,
                window,
                cx,
            );
        }
    }
}
