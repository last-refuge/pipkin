use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Role, StatefulInteractiveElement,
    Styled, Window, div, prelude::*, px,
};
use pipkin_core::*;

use super::controls::*;
use super::workspace::{Overlay, Workspace};
use crate::theme::ActiveTheme;

pub fn relative_time(now: i64, at: i64) -> String {
    let d = (now - at).max(0);
    match d {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", d / 60),
        3600..=86399 => format!("{}h", d / 3600),
        86400..=604799 => format!("{}d", d / 86400),
        _ => format!("{}w", d / 604800),
    }
}

impl Workspace {
    pub(super) fn render_nav(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = cx.theme().clone();
        let c = &t.colors;
        let (project_name, rows, selected, search_empty, now, any_in_project, query, found) = {
            let s = self.state(cx);
            // Saved messages that match, with where they are and how much was searched.
            let found = (s.mode == Mode::Real
                && s.history_search.query == s.search.trim()
                && !s.history_search.query.is_empty())
            .then(|| {
                let r = &s.history_search;
                let hits: Vec<(usize, String, String)> = r
                    .hits
                    .iter()
                    .enumerate()
                    .map(|(i, h)| {
                        (
                            i,
                            s.conversation(h.conversation)
                                .map_or_else(|| "Conversation".into(), |c| c.title.clone()),
                            h.snippet.clone(),
                        )
                    })
                    .collect();
                let coverage = format!(
                    "Searched the saved copies of {} conversation{} ({} messages) on this computer. Earlier history that has not been loaded here is not included.{}",
                    r.conversations_searched,
                    if r.conversations_searched == 1 { "" } else { "s" },
                    r.messages_searched,
                    if r.truncated { " Showing the first results only." } else { "" },
                );
                (hits, coverage)
            });
            let rows: Vec<_> = s
                .visible_conversations()
                .iter()
                .map(|cv| (cv.id, cv.title.clone(), cv.updated_at, cv.activity()))
                .collect();
            let any = s
                .conversations
                .iter()
                .any(|cv| Some(cv.project) == s.current_project().map(|p| p.id));
            (
                s.current_project()
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "No project".into()),
                rows,
                s.selected,
                s.search.trim().is_empty(),
                s.now(),
                any,
                s.search.clone(),
                found,
            )
        };
        let this = cx.entity();
        let list: gpui::AnyElement = if rows.is_empty() {
            let (title, hint) = if !any_in_project {
                (
                    "No conversations yet".to_string(),
                    "Start one with New conversation.".to_string(),
                )
            } else if search_empty {
                ("Nothing here".to_string(), String::new())
            } else {
                (
                    format!("No conversations match “{query}”"),
                    "Try a different search.".to_string(),
                )
            };
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .px(px(12.0))
                .py(px(20.0))
                .child(div().text_color(c.text_muted).child(title))
                .child(
                    div()
                        .text_size(t.small_size())
                        .text_color(c.text_faint)
                        .child(hint),
                )
                .into_any_element()
        } else {
            div()
                .id("conversation-list")
                .role(Role::List)
                .aria_label("Conversations")
                .flex()
                .flex_col()
                .gap(px(2.0))
                .px(px(8.0))
                .overflow_y_scroll()
                .track_scroll(&self.nav_scroll)
                .flex_1()
                .min_h_0()
                .children(rows.into_iter().map(|(id, title, at, activity)| {
                    let is_sel = selected == Some(id);
                    let this = this.clone();
                    let hover = c.bg_hover;
                    let ring = c.accent;
                    div()
                        .id(("conv", id.0 as usize))
                        .role(Role::ListItem)
                        .aria_label(
                            activity.map_or_else(|| title.clone(), |a| format!("{title}, {a}")),
                        )
                        .aria_selected(is_sel)
                        .relative()
                        .flex()
                        .flex_col()
                        .flex_none()
                        .gap(px(2.0))
                        .px(px(12.0))
                        .py(px(9.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .tab_stop(true)
                        .when(is_sel, |d| d.bg(c.bg_selected))
                        .hover(move |s| s.bg(hover))
                        .focus_visible(move |s| s.border_1().border_color(ring))
                        .on_click(move |_, window, cx| {
                            this.update(cx, |this, cx| {
                                this.close_panel(window, cx);
                                this.dispatch(Command::SelectConversation(id), cx);
                            });
                        })
                        // The brand's amber marker on the open conversation.
                        .when(is_sel, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left_0()
                                    .w(px(4.0))
                                    .rounded_l(px(6.0))
                                    .bg(c.accent_fill),
                            )
                        })
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .text_color(if is_sel { c.text } else { c.text_muted })
                                .child(title),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_size(t.small_size())
                                .text_color(c.text_faint)
                                .child(
                                    format!("{} ago", relative_time(now, at))
                                        .replace("now ago", "just now"),
                                )
                                .children(activity.map(|a| {
                                    if a == "Done!" {
                                        chip(a, c.success, c.diff_add_bg, cx)
                                    } else {
                                        chip(a, c.accent, c.accent_bg, cx)
                                    }
                                })),
                        )
                }))
                .into_any_element()
        };

        let found_el = found.map(|(hits, coverage)| {
            let none = hits.is_empty();
            let family = t.ui_font();
            div()
                .id("search-results")
                .role(Role::List)
                .aria_label("Messages that match")
                .flex()
                .flex_col()
                .flex_none()
                .gap(px(2.0))
                .px(px(8.0))
                .pb(px(6.0))
                .max_h(px(300.0))
                .overflow_y_scroll()
                .child(
                    div()
                        .px(px(4.0))
                        .text_size(t.small_size())
                        .text_color(c.text_faint)
                        .child(if none {
                            "No messages match"
                        } else {
                            "Messages"
                        }),
                )
                .children(hits.into_iter().map(|(i, title, snippet)| {
                    let this = this.clone();
                    menu_row(("hit", i), false, cx)
                        .role(Role::ListItem)
                        .aria_label(format!(
                            "{title}: {}",
                            snippet.replace(['\u{2}', '\u{3}'], "")
                        ))
                        .flex_col()
                        .items_start()
                        .gap(px(2.0))
                        .on_click(move |_, window, cx| {
                            this.update(cx, |this, cx| {
                                this.close_panel(window, cx);
                                this.dispatch(Command::OpenSearchHit(i), cx);
                            });
                        })
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .text_size(t.small_size())
                                .text_color(c.text_faint)
                                .child(title),
                        )
                        .child(div().w_full().text_size(t.small_size()).child(marked_text(
                            &snippet,
                            c.text_muted,
                            c.accent,
                            family.clone(),
                        )))
                }))
                .child(
                    div()
                        .px(px(4.0))
                        .pt(px(4.0))
                        .text_size(t.small_size())
                        .text_color(c.text_faint)
                        .child(coverage),
                )
        });
        let proj = {
            let this = cx.entity();
            Btn::new("project-switcher")
                .icon("folder")
                .label(project_name)
                .trailing_icon("chevron-down")
                .aria("Switch project")
                .on_click(move |window, cx| {
                    this.update(cx, |this, cx| {
                        this.open_overlay(Overlay::Project, window, cx)
                    })
                })
        };
        let new_enabled = self.state(cx).availability().new_conversation;
        let new_btn = {
            let this = cx.entity();
            Btn::new("new-conversation")
                .icon("plus")
                .aria("New chat (Ctrl+N)")
                .disabled(!new_enabled)
                .on_click(move |window, cx| {
                    this.update(cx, |this, cx| {
                        this.close_panel(window, cx);
                        this.dispatch(Command::NewConversation, cx);
                        this.focus_composer(window, cx);
                    })
                })
        };
        // The search box shows while it is open or holds a query; the icon opens it, or closes it
        // and clears the query.
        let search_visible = self.search_open || !search_empty;
        let search_btn = {
            let this = cx.entity();
            Btn::new("toggle-search")
                .icon("search")
                .aria("Search conversations")
                .selected(search_visible)
                .on_click(move |window, cx| {
                    this.update(cx, |this, cx| {
                        if search_visible {
                            this.search_open = false;
                            this.nav_search.update(cx, |e, cx| e.set_text("", cx));
                            this.dispatch(Command::SetSearch(String::new()), cx);
                        } else {
                            this.search_open = true;
                            // The box is drawn on the next frame; focus it then.
                            cx.on_next_frame(window, |this, window, cx| {
                                let handle = this.nav_search.read(cx).focus_handle(cx);
                                window.focus(&handle, cx);
                            });
                        }
                        cx.notify();
                    })
                })
        };
        let notice = self.state(cx).notice.clone();
        let notice_el = notice.map(|message| {
            let this = cx.entity();
            div()
                .id("notice")
                .role(Role::Alert)
                .aria_label("Notice")
                .flex()
                .items_start()
                .gap(px(6.0))
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .bg(c.bg_active)
                .text_size(t.small_size())
                .text_color(c.danger)
                .child(div().flex_1().min_w_0().child(message))
                .child(
                    Btn::new("dismiss-notice")
                        .icon("x")
                        .aria("Dismiss notice")
                        .compact()
                        .on_click(move |_, cx| {
                            this.update(cx, |this, cx| this.dispatch(Command::DismissNotice, cx))
                        }),
                )
        });
        let util_row = |id: &'static str,
                        glyph: &'static str,
                        label: &'static str,
                        hint: Option<&'static str>,
                        cx: &mut Context<Self>| {
            let hover = c.bg_hover;
            let ring = c.accent;
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .flex()
                .items_center()
                .gap(px(9.0))
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .tab_stop(true)
                .text_size(t.small_size())
                .text_color(c.text_muted)
                .hover(move |s| s.bg(hover))
                .focus_visible(move |s| s.border_1().border_color(ring))
                .child(icon(glyph, px(15.0), c.text_muted))
                .child(div().flex_1().child(label))
                .children(hint.map(|h| kbd(h, cx)))
        };
        let usage_btn = {
            let this = cx.entity();
            util_row("open-usage", "zap", "Usage", None, cx).on_click(move |_, window, cx| {
                this.update(cx, |this, cx| {
                    this.open_overlay(Overlay::Session, window, cx)
                })
            })
        };
        let prefs_btn = {
            let this = cx.entity();
            util_row("open-prefs", "settings", "Settings", Some("Ctrl ,"), cx).on_click(
                move |_, window, cx| {
                    this.update(cx, |this, cx| this.open_overlay(Overlay::Prefs, window, cx))
                },
            )
        };
        let palette_btn = {
            let this = cx.entity();
            util_row("open-palette", "command", "Commands", Some("Ctrl K"), cx).on_click(
                move |_, window, cx| {
                    this.update(cx, |this, cx| {
                        this.open_overlay(Overlay::Palette, window, cx)
                    })
                },
            )
        };

        div()
            .id("navigation")
            .role(Role::Navigation)
            .aria_label("Navigation")
            .flex()
            .flex_col()
            .size_full()
            .bg(c.bg_pane)
            .gap(px(8.0))
            .pb(px(12.0))
            // The logo is centred in a box as tall as the conversation header beside it, so the
            // rule under it continues the header's, and "New chat" starts right below that line.
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .h(px(68.0 * t.scale.max(1.0)))
                    .mb(px(4.0))
                    .border_b_1()
                    .border_color(c.border)
                    .child(wordmark(px(24.0), cx)),
            )
            .child(
                div()
                    .px(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(proj)
                    .children(notice_el),
            )
            // The list's own heading, with its two actions at the right.
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(16.0))
                    .pr(px(12.0))
                    .child(
                        div()
                            .text_size(t.small_size())
                            .text_color(c.text_muted)
                            .child("Conversations"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .child(search_btn)
                            .child(new_btn),
                    ),
            )
            .when(search_visible, |d| {
                d.child(
                    div().px(px(12.0)).child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(10.0))
                            .h(t.control_height())
                            .rounded(px(6.0))
                            .bg(c.bg_input)
                            .border_1()
                            .border_color(c.border)
                            .child(icon("search", px(14.0), c.text_faint))
                            .child(div().flex_1().min_w_0().child(self.nav_search.clone())),
                    ),
                )
            })
            .children(found_el)
            .child(list)
            .child(
                div()
                    .px(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(palette_btn)
                    .child(usage_btn)
                    .child(prefs_btn),
            )
    }
}
