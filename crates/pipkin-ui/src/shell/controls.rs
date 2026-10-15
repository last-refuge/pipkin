//! Shared control anatomy: every interactive control uses these so hover, active, selected,
//! disabled and keyboard-focus states stay consistent.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Window, div, prelude::*, px, svg,
};

use crate::theme::ActiveTheme;

pub type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

pub fn handler(f: impl Fn(&mut Window, &mut App) + 'static) -> Handler {
    Rc::new(f)
}

pub fn icon(name: &str, size: Pixels, color: Hsla) -> impl IntoElement {
    svg()
        .path(format!("icons/{name}.svg"))
        .size(size)
        .flex_none()
        .text_color(color)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BtnKind {
    /// Transparent until hovered.
    Ghost,
    /// Quiet filled surface.
    Subtle,
    /// The single accent action on a surface.
    Primary,
    Danger,
}

#[derive(IntoElement)]
pub struct Btn {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<&'static str>,
    trailing_icon: Option<&'static str>,
    kind: BtnKind,
    disabled: bool,
    selected: bool,
    on_click: Option<Handler>,
    aria: Option<SharedString>,
    compact: bool,
}

impl Btn {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Btn {
            id: id.into(),
            label: None,
            icon: None,
            trailing_icon: None,
            kind: BtnKind::Ghost,
            disabled: false,
            selected: false,
            on_click: None,
            aria: None,
            compact: false,
        }
    }
    pub fn label(mut self, l: impl Into<SharedString>) -> Self {
        self.label = Some(l.into());
        self
    }
    pub fn icon(mut self, i: &'static str) -> Self {
        self.icon = Some(i);
        self
    }
    pub fn trailing_icon(mut self, i: &'static str) -> Self {
        self.trailing_icon = Some(i);
        self
    }
    pub fn kind(mut self, k: BtnKind) -> Self {
        self.kind = k;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
    pub fn aria(mut self, a: impl Into<SharedString>) -> Self {
        self.aria = Some(a.into());
        self
    }
    pub fn on_click(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Btn {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let h = if self.compact {
            px(24.0 * t.scale.max(1.0))
        } else {
            t.control_height()
        };
        let (fg, bg) = match (self.kind, self.selected) {
            (BtnKind::Primary, _) => (c.accent_text, c.accent_fill),
            (BtnKind::Danger, _) => (c.danger, c.danger_bg),
            (BtnKind::Subtle, _) => (c.text, c.bg_active),
            (BtnKind::Ghost, true) => (c.text, c.bg_selected),
            (BtnKind::Ghost, false) => (c.text_muted, gpui::transparent_black()),
        };
        let icon_only = self.label.is_none();
        let label = self.label.clone();
        let aria = self.aria.clone().or(self.label.clone());
        let disabled = self.disabled;
        let on_click = self.on_click.clone();
        let on_key = self.on_click.clone();
        let kind = self.kind;
        let hover_bg = if kind == BtnKind::Primary {
            c.accent_fill.opacity(0.85)
        } else {
            c.bg_hover
        };
        let active_bg = if kind == BtnKind::Primary {
            c.accent_fill.opacity(0.7)
        } else {
            c.bg_active
        };
        let ring = c.accent;
        let mut el: Stateful<Div> = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .h(h)
            .min_w(h)
            .when(!icon_only, |d| d.px(px(10.0)))
            .rounded(px(6.0))
            .bg(bg)
            .text_color(fg)
            .text_size(t.ui_size())
            .font_family(t.ui_font())
            .role(gpui::Role::Button)
            .when_some(aria, |d, a| d.aria_label(a))
            .when(disabled, |d| d.opacity(0.4).cursor_not_allowed())
            .when(!disabled, |d| {
                d.cursor_pointer()
                    .tab_index(0)
                    .hover(move |s| s.bg(hover_bg))
                    .active(move |s| s.bg(active_bg))
                    .focus_visible(move |s| s.border_2().border_color(ring))
                    .when_some(on_click, |d, f| {
                        d.on_click(move |_: &ClickEvent, w, cx| f(w, cx))
                    })
                    .when_some(on_key, |d, f| {
                        // GPUI also synthesizes a click on key-up. Handle activation
                        // in capture so its bubble listener cannot arm a second click.
                        d.capture_key_down(move |ev, w, cx| {
                            if matches!(ev.keystroke.key.as_str(), "enter" | "space")
                                && !ev.keystroke.modifiers.modified()
                            {
                                w.prevent_default();
                                if !ev.is_held {
                                    f(w, cx);
                                }
                                cx.stop_propagation();
                            }
                        })
                    })
            });
        if let Some(i) = self.icon {
            el = el.child(icon(i, px(16.0 * t.scale.max(1.0)), fg));
        }
        if let Some(l) = label {
            el = el.child(div().child(l));
        }
        if let Some(i) = self.trailing_icon {
            el = el.child(icon(i, px(14.0), fg));
        }
        el
    }
}

/// Small pill used for attachments, status and shortcuts.
pub fn chip(text: impl Into<SharedString>, fg: Hsla, bg: Hsla, cx: &App) -> Div {
    let t = cx.theme();
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .px(px(8.0))
        .h(px(22.0 * t.scale.max(1.0)))
        .rounded(px(11.0))
        .bg(bg)
        .text_color(fg)
        .text_size(t.small_size())
        .child(text.into())
}

pub fn kbd(text: impl Into<SharedString>, cx: &App) -> Div {
    let t = cx.theme();
    div()
        .px(px(5.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(t.colors.border_strong)
        .text_color(t.colors.text_faint)
        .text_size(t.small_size())
        .font_family(t.mono_font())
        .child(text.into())
}

/// Elevated surface for menus and dialogs: the only elevated look in the app.
pub fn elevated(cx: &App) -> Div {
    let t = cx.theme();
    div()
        .bg(t.colors.bg_elevated)
        .border_1()
        .border_color(t.colors.border_strong)
        .rounded(px(10.0))
        .shadow_lg()
        .text_color(t.colors.text)
        .text_size(t.ui_size())
        .font_family(t.ui_font())
}

pub fn menu_row(id: impl Into<ElementId>, selected: bool, cx: &App) -> Stateful<Div> {
    let t = cx.theme();
    let hover = t.colors.bg_hover;
    let ring = t.colors.accent;
    div()
        .id(id)
        .flex()
        // A row keeps its height: in a scrolling list it must scroll, not squeeze.
        .flex_none()
        .items_center()
        .gap(px(8.0))
        .min_h(t.control_height())
        .px(px(10.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .tab_stop(true)
        .when(selected, |d| d.bg(t.colors.bg_selected))
        .hover(move |s| s.bg(hover))
        .focus_visible(move |s| s.border_1().border_color(ring))
}

pub fn separator(cx: &App) -> Div {
    div().h(px(1.0)).w_full().bg(cx.theme().colors.border)
}

pub fn into_any(e: impl IntoElement) -> AnyElement {
    e.into_any_element()
}

/// A search snippet: the text with the part the search matched (between STX and ETX in the
/// snippet) in bold and the accent colour.
pub fn marked_text(
    snippet: &str,
    base: Hsla,
    mark: Hsla,
    family: SharedString,
) -> gpui::StyledText {
    let mut text = String::new();
    let mut runs: Vec<gpui::TextRun> = Vec::new();
    let mut marked = false;
    for piece in snippet.split_inclusive(['\u{2}', '\u{3}']) {
        let (body, flip) = match piece.chars().last() {
            Some('\u{2}') => (&piece[..piece.len() - 1], Some(true)),
            Some('\u{3}') => (&piece[..piece.len() - 1], Some(false)),
            _ => (piece, None),
        };
        if !body.is_empty() {
            let mut font = gpui::font(family.clone());
            if marked {
                font.weight = gpui::FontWeight::SEMIBOLD;
            }
            runs.push(gpui::TextRun {
                len: body.len(),
                font,
                color: if marked { mark } else { base },
                background_color: None,
                underline: None,
                strikethrough: None,
            });
            text.push_str(body);
        }
        if let Some(on) = flip {
            marked = on;
        }
    }
    if runs.is_empty() {
        runs.push(gpui::TextRun {
            len: 0,
            font: gpui::font(family),
            color: base,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    gpui::StyledText::new(SharedString::from(text)).with_runs(runs)
}

/// The Pipkin wordmark: Poppins Bold, with the brand amber on "in".
pub fn wordmark(size: gpui::Pixels, cx: &App) -> Div {
    let t = cx.theme();
    div()
        .flex()
        .items_baseline()
        .font_family(t.ui_font())
        .font_weight(gpui::FontWeight::BOLD)
        .text_size(size)
        .line_height(size * 1.15)
        .text_color(t.colors.text)
        .child("Pipk")
        .child(div().text_color(t.colors.accent_fill).child("in"))
}

#[cfg(test)]
mod tests;
