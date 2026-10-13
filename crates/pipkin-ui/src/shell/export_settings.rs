//! The Trace export settings page (plan §12, OBS-3): where traces go, how much of them,
//! the collector credential (stored by the engine, never shown back), a payload preview at
//! either content level, and one control that turns export off. Everything is written and
//! validated by the engine; the page projects its health.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Role, Styled, div,
    prelude::*, px,
};
use pipkin_core::diagnostics::{ExportConfig, ExportHealth};
use pipkin_core::{Command, Mode};

use super::controls::{Btn, BtnKind, separator};
use super::workspace::Workspace;
use crate::theme::ActiveTheme;

impl Workspace {
    /// Fill the page's fields from the engine's configuration (when the page opens).
    pub(super) fn prefill_export_page(&mut self, cx: &mut Context<Self>) {
        let Some(health) = self.state(cx).export_health.clone() else {
            return;
        };
        self.export_content = health.content.clone();
        let endpoint = health.endpoint_url.clone().unwrap_or_default();
        let service = health.service_name.clone().unwrap_or_default();
        self.export_endpoint
            .update(cx, |e, cx| e.set_text(&endpoint, cx));
        self.export_service
            .update(cx, |e, cx| e.set_text(&service, cx));
        if self.export_header.read(cx).text().trim().is_empty() {
            self.export_header
                .update(cx, |e, cx| e.set_text("Authorization", cx));
        }
    }

    fn export_config(&self, enabled: bool, cx: &Context<Self>) -> ExportConfig {
        ExportConfig {
            enabled,
            endpoint: self.export_endpoint.read(cx).text().trim().to_owned(),
            content: self.export_content.clone(),
            service_name: self.export_service.read(cx).text().trim().to_owned(),
        }
    }

    pub(super) fn submit_export_credential(&mut self, cx: &mut Context<Self>) {
        let header = self.export_header.read(cx).text();
        let value = self.export_secret.read(cx).text();
        self.dispatch(
            Command::ExportSetCredential {
                header,
                value: pipkin_core::onboarding::SignInAnswer::new(value),
            },
            cx,
        );
        self.export_secret.update(cx, |e, cx| e.set_text("", cx));
    }

    pub(super) fn render_export_section(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let c = t.colors.clone();
        let this = cx.entity();
        let real = self.state(cx).mode == Mode::Real && self.state(cx).connection.is_ready();
        let health: Option<ExportHealth> = self.state(cx).export_health.clone();
        let page = self.state(cx).export_page.clone();
        let busy = page.busy.is_some();
        let card = |cx: &Context<Self>| {
            div()
                .rounded(px(8.0))
                .border_1()
                .border_color(cx.theme().colors.border)
                .bg(cx.theme().colors.bg_surface)
        };
        let field = |editor: gpui::Entity<crate::text::ComposerEditor>, cx: &Context<Self>| {
            let c = &cx.theme().colors;
            div()
                .px(px(10.0))
                .py(px(6.0))
                .min_h(px(34.0))
                .flex()
                .items_center()
                .rounded(px(6.0))
                .bg(c.bg_input)
                .border_1()
                .border_color(c.border_strong)
                .child(div().flex_1().min_w_0().child(editor))
        };
        let label = |text: &'static str, t: &crate::theme::Theme| {
            div()
                .text_size(t.small_size())
                .text_color(t.colors.text_muted)
                .child(text)
        };
        let mut root = div().flex().flex_col().gap(px(16.0));
        if self.state(cx).mode == Mode::Demo {
            return root
                .child(
                    card(cx).child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .text_size(t.small_size())
                            .text_color(c.text_muted)
                            .child("Demo · simulated agent: there is no engine to export from."),
                    ),
                )
                .into_any_element();
        }
        // Status.
        let mut status = div().flex().flex_col().gap(px(4.0));
        status = status.child(
            div()
                .id("export-status")
                .role(Role::Status)
                .text_size(t.small_size())
                .text_color(c.text_muted)
                .child(format!(
                    "Export: {}",
                    health
                        .as_ref()
                        .map_or_else(|| "not reported yet".to_owned(), ExportHealth::summary)
                )),
        );
        if let Some(busy) = &page.busy {
            status = status.child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child(busy.clone()),
            );
        }
        if let Some(notice) = &page.notice {
            let this = this.clone();
            status = status.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("export-notice")
                            .role(Role::Alert)
                            .text_size(t.small_size())
                            .text_color(c.warning)
                            .child(notice.clone()),
                    )
                    .child(
                        Btn::new("export-dismiss-notice")
                            .label("Dismiss")
                            .kind(BtnKind::Ghost)
                            .compact()
                            .on_click(move |_, cx| {
                                this.update(cx, |w, cx| {
                                    w.dispatch(Command::ExportDismissNotice, cx)
                                });
                            }),
                    ),
            );
        }
        if health.as_ref().is_some_and(|h| h.headers_in_settings) {
            status = status.child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.warning)
                    .child("The settings file carries export headers with values. Store the credential below and remove them from the file."),
            );
        }
        root = root.child(status);
        // Collector.
        let metadata = self.export_content != "content";
        let (pick_meta, pick_content) = (this.clone(), this.clone());
        let (save, off) = (this.clone(), this.clone());
        let collector = card(cx)
            .id("export-collector")
            .role(Role::Group)
            .aria_label("Collector")
            .flex()
            .flex_col()
            .gap(px(10.0))
            .px(px(16.0))
            .py(px(14.0))
            .child(div().font_weight(gpui::FontWeight::MEDIUM).child("Collector"))
            .child(label("Endpoint URL (OTLP/HTTP traces; Langfuse: its /api/public/otel/v1/traces)", &t))
            .child(field(self.export_endpoint.clone(), cx))
            .child(label("Service name shown on the traces", &t))
            .child(field(self.export_service.clone(), cx))
            .child(label("What leaves the machine", &t))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.0))
                    .child(
                        Btn::new("export-content-metadata")
                            .label("Metadata only")
                            .aria(format!("Metadata only{}", if metadata { ", current choice" } else { "" }))
                            .kind(if metadata { BtnKind::Subtle } else { BtnKind::Ghost })
                            .selected(metadata)
                            .on_click(move |_, cx| {
                                pick_meta.update(cx, |w, cx| {
                                    w.export_content = "metadata".into();
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        Btn::new("export-content-content")
                            .label("Prompts, replies and tool text")
                            .aria(format!("Prompts, replies and tool text{}", if !metadata { ", current choice" } else { "" }))
                            .kind(if !metadata { BtnKind::Subtle } else { BtnKind::Ghost })
                            .selected(!metadata)
                            .on_click(move |_, cx| {
                                pick_content.update(cx, |w, cx| {
                                    w.export_content = "content".into();
                                    cx.notify();
                                });
                            }),
                    ),
            )
            .child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_faint)
                    .child(if metadata {
                        "Ids, timings, token counts and outcomes. No prompts, replies, tool bodies or credentials."
                    } else {
                        "Also the person's prompts, the model's replies and each tool call's arguments and output, cut at 4,096 characters and with the stored credential and token-like text redacted. Images and web page bodies are never sent."
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(
                        Btn::new("export-save")
                            .label("Save and enable")
                            .kind(BtnKind::Primary)
                            .compact()
                            .disabled(!real || busy)
                            .on_click(move |_, cx| {
                                save.update(cx, |w, cx| {
                                    let config = w.export_config(true, cx);
                                    w.dispatch(Command::ExportConfigure(config), cx);
                                });
                            }),
                    )
                    .child(
                        Btn::new("export-disable")
                            .label("Turn off")
                            .kind(BtnKind::Subtle)
                            .compact()
                            .disabled(!real || busy || !health.as_ref().is_some_and(|h| h.enabled))
                            .aria("Turn trace export off")
                            .on_click(move |_, cx| {
                                off.update(cx, |w, cx| w.dispatch(Command::ExportDisable, cx));
                            }),
                    ),
            );
        root = root.child(collector);
        // Credential.
        let (store, clear) = (this.clone(), this.clone());
        let stored = health.as_ref().and_then(|h| h.credential.clone());
        let mut credential = card(cx)
            .id("export-credential")
            .role(Role::Group)
            .aria_label("Collector credential")
            .flex()
            .flex_col()
            .gap(px(10.0))
            .px(px(16.0))
            .py(px(14.0))
            .child(div().font_weight(gpui::FontWeight::MEDIUM).child("Credential"))
            .child(
                div()
                    .id("export-credential-state")
                    .role(Role::Status)
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child(match &stored {
                        Some(name) => format!("Stored as the {name} header. The value is never shown again."),
                        None => "No credential stored. Langfuse: Authorization with Basic <base64 of public-key:secret-key>.".to_owned(),
                    }),
            )
            .child(label("Header name", &t))
            .child(field(self.export_header.clone(), cx))
            .child(label("Value (kept in the engine's private credential file, not in settings)", &t))
            .child(field(self.export_secret.clone(), cx))
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(
                        Btn::new("export-credential-store")
                            .label("Store")
                            .kind(BtnKind::Subtle)
                            .compact()
                            .disabled(!real || busy)
                            .on_click(move |_, cx| {
                                store.update(cx, |w, cx| w.submit_export_credential(cx));
                            }),
                    )
                    .when(stored.is_some(), |d| {
                        d.child(
                            Btn::new("export-credential-clear")
                                .label("Remove")
                                .kind(BtnKind::Ghost)
                                .compact()
                                .disabled(!real || busy)
                                .on_click(move |_, cx| {
                                    clear.update(cx, |w, cx| {
                                        w.dispatch(Command::ExportClearCredential, cx)
                                    });
                                }),
                        )
                    }),
            );
        credential = credential.child(separator(cx));
        root = root.child(credential);
        // Preview.
        let (pv_meta, pv_content) = (this.clone(), this.clone());
        let mut preview = card(cx)
            .id("export-preview")
            .role(Role::Group)
            .aria_label("Payload preview")
            .flex()
            .flex_col()
            .gap(px(10.0))
            .px(px(16.0))
            .py(px(14.0))
            .child(div().font_weight(gpui::FontWeight::MEDIUM).child("What a payload looks like"))
            .child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child("Built from the open conversation's newest records (or a sample). Nothing is sent by a preview."),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(
                        Btn::new("export-preview-metadata")
                            .label("Preview metadata")
                            .kind(BtnKind::Subtle)
                            .compact()
                            .disabled(!real || busy)
                            .on_click(move |_, cx| {
                                pv_meta.update(cx, |w, cx| {
                                    w.dispatch(Command::ExportPreview("metadata".into()), cx)
                                });
                            }),
                    )
                    .child(
                        Btn::new("export-preview-content")
                            .label("Preview with content")
                            .kind(BtnKind::Subtle)
                            .compact()
                            .disabled(!real || busy)
                            .on_click(move |_, cx| {
                                pv_content.update(cx, |w, cx| {
                                    w.dispatch(Command::ExportPreview("content".into()), cx)
                                });
                            }),
                    ),
            );
        if let Some((level, payload)) = &page.preview {
            preview = preview
                .child(label(
                    if level == "content" {
                        "At the content level:"
                    } else {
                        "At the metadata level:"
                    },
                    &t,
                ))
                .child(
                    div()
                        .id("export-preview-body")
                        .role(Role::Document)
                        .max_h(px(240.0))
                        .overflow_y_scroll()
                        .px(px(10.0))
                        .py(px(8.0))
                        .rounded(px(6.0))
                        .bg(c.bg_input)
                        .text_size(t.small_size())
                        .font_family(t.mono_font())
                        .child(payload.clone()),
                );
        }
        root = root.child(preview);
        root.child(
            div()
                .text_size(t.small_size())
                .text_color(c.text_faint)
                .child("Export is off by default. Local diagnostics and costs never depend on it."),
        )
        .into_any_element()
    }
}
