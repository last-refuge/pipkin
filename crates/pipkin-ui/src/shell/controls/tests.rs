use super::*;
use gpui::{
    Context, KeyDownEvent, KeyUpEvent, Keystroke, PlatformInput, Render, TestAppContext,
    VisualTestContext,
};
use std::{cell::Cell, rc::Rc};

struct Host(Rc<Cell<usize>>);
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let count = self.0.clone();
        Btn::new("one-activation")
            .label("Activate once")
            .on_click(move |w, _| {
                count.set(count.get() + 1);
                w.refresh();
            })
    }
}

#[gpui::test]
fn button_native_keydown_and_keyup_activate_once_not_on_hold_or_modifier(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(crate::theme::Theme::new(
            pipkin_core::Theme::Dark,
            pipkin_core::TextSize::Normal,
            true,
        ))
    });
    let count = Rc::new(Cell::new(0));
    let captured = count.clone();
    let window = cx.add_window(|_, _| Host(captured));
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        window.focus_next(cx);
    });
    for (n, key) in ["space", "enter"].into_iter().enumerate() {
        cx.simulate_keystrokes(key);
        assert_eq!(count.get(), n + 1);
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            window.dispatch_event(
                PlatformInput::KeyDown(KeyDownEvent {
                    keystroke: Keystroke::parse(key).unwrap(),
                    is_held: true,
                    prefer_character_input: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::KeyUp(KeyUpEvent {
                    keystroke: Keystroke::parse(key).unwrap(),
                }),
                cx,
            );
        });
        assert_eq!(
            count.get(),
            n + 1,
            "hold and release must not duplicate activation"
        );
    }
    cx.simulate_keystrokes("ctrl-space shift-enter");
    assert_eq!(
        count.get(),
        2,
        "modified activation keys are not plain clicks"
    );
}
