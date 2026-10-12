//! The "MCP servers" settings section: the engine's servers with their state, test /
//! reconnect / sign-in / remove, per-tool switches, an add form and a previewed import.
//! Everything shown comes from the engine's redacted view; nothing here holds a secret.

use gpui::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, ParentElement, Role, Styled, div,
    prelude::*, px,
};
use pipkin_core::{
    Command, McpAuthStatus, McpKind, McpScope, McpServer, McpServerInput, McpServerState,
};

use super::controls::{Btn, BtnKind, chip, separator};
use super::workspace::Workspace;
use crate::theme::ActiveTheme;

impl Workspace {
    fn mcp_dispatch(&self, command: Command, cx: &mut gpui::App) {
        self.dispatch(command, cx);
    }

    /// Send the add form; the engine validates and the snapshot that follows lists it.
    pub(super) fn submit_mcp_server(&mut self, cx: &mut Context<Self>) {
        let input = McpServerInput {
            scope: self.mcp_scope,
            name: self.mcp_name.read(cx).text().trim().to_owned(),
            kind: self.mcp_kind,
            target: self.mcp_target.read(cx).text(),
            values: McpServerInput::parse_values(&self.mcp_values.read(cx).text()),
        };
        let valid = input.problem().is_none();
        self.mcp_dispatch(Command::McpAddServer(input), cx);
        if valid {
            self.mcp_name.update(cx, |e, cx| e.set_text("", cx));
            self.mcp_target.update(cx, |e, cx| e.set_text("", cx));
            self.mcp_values.update(cx, |e, cx| e.set_text("", cx));
        }
    }

    pub(super) fn submit_mcp_answer(&mut self, cx: &mut Context<Self>) {
        let response = self.mcp_answer.read(cx).text();
        if response.trim().is_empty() {
            return;
        }
        let pending = self.state(cx).mcp.servers.iter().find_map(|s| {
            s.auth
                .prompt
                .as_ref()
                .map(|p| (s.name.clone(), p.id.clone()))
        });
        let Some((server, prompt)) = pending else {
            return;
        };
        self.mcp_dispatch(
            Command::McpAnswerSignIn {
                server,
                prompt,
                response: pipkin_core::onboarding::SignInAnswer::new(response),
            },
            cx,
        );
        self.mcp_answer.update(cx, |e, cx| e.set_text("", cx));
    }

    fn mcp_server_row(&self, index: usize, server: &McpServer, cx: &Context<Self>) -> Div {
        let t = cx.theme().clone();
        let c = t.colors.clone();
        let this = cx.entity();
        let name = server.name.clone();
        let expanded = self.state(cx).mcp.expanded.as_deref() == Some(server.name.as_str());
        let (state_fg, state_bg) = match server.state {
            McpServerState::Ready => (c.success, c.bg_active),
            McpServerState::Connecting => (c.text, c.bg_active),
            McpServerState::AuthRequired => (c.warning, c.bg_active),
            McpServerState::Failed => (c.danger, c.danger_bg),
            McpServerState::Stopped => (c.text_muted, c.bg_active),
        };
        let mut header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(c.text)
                    .child(server.name.clone()),
            )
            .child(chip(
                match server.kind {
                    McpKind::Stdio => "command",
                    McpKind::Http => "HTTP",
                },
                c.text_muted,
                c.bg_active,
                cx,
            ))
            .child(chip(
                match server.scope {
                    McpScope::User => "user",
                    McpScope::Project => "project",
                },
                c.text_muted,
                c.bg_active,
                cx,
            ))
            .child(chip(server.state.label(), state_fg, state_bg, cx));
        if !server.enabled {
            header = header.child(chip("off", c.text_faint, c.bg_active, cx));
        }
        if let Some(n) = server.tools {
            header = header.child(chip(format!("{n} tool(s)"), c.text_muted, c.bg_active, cx));
        }
        if let Some(n) = server.resources {
            header = header.child(chip(
                format!("{n} resource(s)"),
                c.text_muted,
                c.bg_active,
                cx,
            ));
        }
        if let Some(n) = server.prompts {
            header = header.child(chip(
                format!("{n} prompt(s)"),
                c.text_muted,
                c.bg_active,
                cx,
            ));
        }
        for capability in &server.capabilities {
            if matches!(capability.as_str(), "resources" | "prompts")
                && server.state == McpServerState::Ready
            {
                header = header.child(chip(
                    format!("offers {capability}"),
                    c.text_faint,
                    c.bg_active,
                    cx,
                ));
            }
        }
        if server.auth.signed_in {
            header = header.child(chip("signed in", c.success, c.bg_active, cx));
        }
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(16.0))
            .py(px(12.0))
            .child(header)
            .child(
                div()
                    .text_size(t.small_size())
                    .font_family(t.mono_font())
                    .text_color(c.text_muted)
                    .child(server.command.clone()),
            );
        if !server.env.is_empty() {
            body = body.child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_faint)
                    .child(server.env.join("  ")),
            );
        }
        if let Some(error) = &server.error {
            body = body.child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.danger)
                    .child(error.clone()),
            );
        }
        // Sign-in progress for HTTP servers.
        if server.auth.supported {
            let auth = &server.auth;
            if !auth.message.is_empty() && auth.status != McpAuthStatus::Idle {
                body = body.child(
                    div()
                        .text_size(t.small_size())
                        .text_color(if auth.status == McpAuthStatus::Failed {
                            c.warning
                        } else {
                            c.text_muted
                        })
                        .child(auth.message.clone()),
                );
            }
            let mut actions = div().flex().flex_wrap().gap(px(4.0));
            if let Some(url) = auth.url.clone() {
                actions = actions.child(
                    Btn::new(("mcp-open-auth", index))
                        .label("Open sign-in page")
                        .kind(BtnKind::Primary)
                        .compact()
                        .on_click(move |_, cx| cx.open_url(&url)),
                );
            }
            if let Some(prompt) = &auth.prompt {
                if prompt.kind == "secret" {
                    body = body.child(div().text_size(t.small_size()).text_color(c.warning).child(
                        "This server asked for a private field that Pipkin cannot safely show here. Cancel and try again later.",
                    ));
                } else {
                    let this = this.clone();
                    body = body
                        .child(
                            div()
                                .text_size(t.small_size())
                                .text_color(c.text)
                                .child(prompt.message.clone()),
                        )
                        .child(div().w_full().child(self.mcp_answer.clone()))
                        .child(
                            Btn::new(("mcp-answer", index))
                                .label("Continue sign-in")
                                .kind(BtnKind::Subtle)
                                .compact()
                                .on_click(move |_, cx| {
                                    this.update(cx, |w, cx| w.submit_mcp_answer(cx));
                                }),
                        );
                }
            }
            let in_progress = matches!(
                auth.status,
                McpAuthStatus::Connecting | McpAuthStatus::Waiting | McpAuthStatus::Prompt
            );
            if in_progress {
                let this = this.clone();
                let n = name.clone();
                actions = actions.child(
                    Btn::new(("mcp-cancel-auth", index))
                        .label("Cancel sign-in")
                        .kind(BtnKind::Ghost)
                        .compact()
                        .on_click(move |_, cx| {
                            this.update(cx, |w, cx| {
                                w.mcp_dispatch(Command::McpCancelSignIn(n.clone()), cx)
                            });
                        }),
                );
            }
            body = body.child(actions);
        }
        // Actions.
        let mut actions = div().flex().flex_wrap().gap(px(4.0)).mt(px(4.0));
        let button = |id: &'static str, label: &'static str, command: Command, kind: BtnKind| {
            let this = this.clone();
            Btn::new((id, index))
                .label(label)
                .kind(kind)
                .compact()
                .aria(format!("{label} {name}"))
                .on_click(move |_, cx| {
                    this.update(cx, |w, cx| w.mcp_dispatch(command.clone(), cx));
                })
        };
        actions = actions
            .child(button(
                "mcp-test",
                "Test",
                Command::McpTestServer(name.clone()),
                BtnKind::Subtle,
            ))
            .child(button(
                "mcp-reconnect",
                "Reconnect",
                Command::McpReconnectServer(name.clone()),
                BtnKind::Ghost,
            ));
        if server.auth.supported {
            if server.auth.signed_in {
                actions = actions.child(button(
                    "mcp-signout",
                    "Sign out",
                    Command::McpSignOut(name.clone()),
                    BtnKind::Ghost,
                ));
            } else {
                actions = actions.child(button(
                    "mcp-signin",
                    "Sign in",
                    Command::McpSignIn(name.clone()),
                    if server.auth.required {
                        BtnKind::Primary
                    } else {
                        BtnKind::Ghost
                    },
                ));
            }
        }
        if !server.catalog.is_empty() {
            actions = actions.child(button(
                "mcp-tools",
                if expanded { "Hide tools" } else { "Tools" },
                Command::McpToggleTools(name.clone()),
                BtnKind::Ghost,
            ));
        }
        actions = actions.child(button(
            "mcp-remove",
            "Remove",
            Command::McpRemoveServer(name.clone()),
            BtnKind::Ghost,
        ));
        body = body.child(actions);
        if expanded {
            let mut list = div().flex().flex_col().gap(px(2.0)).mt(px(4.0));
            for (ti, tool) in server.catalog.iter().enumerate() {
                let this = this.clone();
                let (server_name, tool_name, enabled) =
                    (name.clone(), tool.name.clone(), tool.enabled);
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            Btn::new(("mcp-tool", index * 1000 + ti))
                                .label(if enabled { "On" } else { "Off" })
                                .kind(if enabled {
                                    BtnKind::Subtle
                                } else {
                                    BtnKind::Ghost
                                })
                                .selected(enabled)
                                .compact()
                                .aria(format!(
                                    "{} {}/{}",
                                    if enabled { "Switch off" } else { "Switch on" },
                                    server_name,
                                    tool_name
                                ))
                                .on_click(move |_, cx| {
                                    this.update(cx, |w, cx| {
                                        w.mcp_dispatch(
                                            Command::McpSetToolEnabled {
                                                server: server_name.clone(),
                                                tool: tool_name.clone(),
                                                enabled: !enabled,
                                            },
                                            cx,
                                        )
                                    });
                                }),
                        )
                        .child(
                            div()
                                .text_size(t.small_size())
                                .text_color(if enabled { c.text } else { c.text_faint })
                                .font_family(t.mono_font())
                                .child(tool.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(t.small_size())
                                .text_color(c.text_muted)
                                .child(tool.description.clone()),
                        ),
                );
            }
            body = body.child(list);
        }
        body
    }

    pub(super) fn render_mcp_section(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let c = t.colors.clone();
        let this = cx.entity();
        let mcp = self.state(cx).mcp.clone();
        let card = |cx: &Context<Self>| {
            div()
                .rounded(px(8.0))
                .border_1()
                .border_color(cx.theme().colors.border)
                .bg(cx.theme().colors.bg_surface)
        };
        let mut root = div().flex().flex_col().gap(px(16.0));
        if !mcp.available {
            return root
                .child(card(cx).child(
                    div().px(px(16.0)).py(px(12.0)).text_size(t.small_size()).text_color(c.text_muted).child(
                        "The built-in engine manages MCP servers. The demo backend has none to show.",
                    ),
                ))
                .into_any_element();
        }
        // Status lines.
        let mut status = div().flex().flex_col().gap(px(4.0));
        if let Some(busy) = &mcp.busy {
            status = status.child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child(busy.clone()),
            );
        }
        if let Some(notice) = &mcp.notice {
            let this = this.clone();
            status = status.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(t.small_size())
                            .text_color(c.warning)
                            .child(notice.clone()),
                    )
                    .child(
                        Btn::new("mcp-dismiss-notice")
                            .label("Dismiss")
                            .kind(BtnKind::Ghost)
                            .compact()
                            .on_click(move |_, cx| {
                                this.update(cx, |w, cx| {
                                    w.mcp_dispatch(Command::McpDismissNotice, cx)
                                });
                            }),
                    ),
            );
        }
        if let Some(test) = &mcp.test {
            let this = this.clone();
            status = status.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(t.small_size())
                            .text_color(if test.ok { c.success } else { c.danger })
                            .child(format!("{}: {}", test.name, test.message)),
                    )
                    .child(
                        Btn::new("mcp-dismiss-test")
                            .label("Dismiss")
                            .kind(BtnKind::Ghost)
                            .compact()
                            .on_click(move |_, cx| {
                                this.update(cx, |w, cx| {
                                    w.mcp_dispatch(Command::McpDismissNotice, cx)
                                });
                            }),
                    ),
            );
        }
        for warning in &mcp.warnings {
            status = status.child(
                div()
                    .text_size(t.small_size())
                    .text_color(c.warning)
                    .child(warning.clone()),
            );
        }
        root = root.child(status);
        // Servers.
        let mut servers = card(cx)
            .id("mcp-servers")
            .role(Role::List)
            .aria_label("MCP servers");
        if mcp.servers.is_empty() {
            servers = servers.child(
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .text_size(t.small_size())
                    .text_color(c.text_muted)
                    .child("No servers yet. Add one below, or paste a configuration to import."),
            );
        }
        for (index, server) in mcp.servers.iter().enumerate() {
            if index > 0 {
                servers = servers.child(separator(cx));
            }
            servers = servers.child(self.mcp_server_row(index, server, cx));
        }
        root = root.child(servers);
        // Add form.
        let kind_button =
            |id: &'static str, label: &'static str, kind: McpKind, current: McpKind| {
                let this = this.clone();
                Btn::new(id)
                    .label(label)
                    .kind(if kind == current {
                        BtnKind::Subtle
                    } else {
                        BtnKind::Ghost
                    })
                    .selected(kind == current)
                    .compact()
                    .on_click(move |_, cx| {
                        this.update(cx, |w, cx| {
                            w.mcp_kind = kind;
                            w.mcp_target.update(cx, |e, _| {
                                e.set_placeholder(match kind {
                                    McpKind::Stdio => {
                                        "npx -y @modelcontextprotocol/server-filesystem /path"
                                    }
                                    McpKind::Http => "https://example.com/mcp",
                                })
                            });
                            cx.notify();
                        });
                    })
            };
        let scope_button = |id: &'static str,
                            label: &'static str,
                            scope: McpScope,
                            current: McpScope,
                            enabled: bool| {
            let this = this.clone();
            Btn::new(id)
                .label(label)
                .kind(if scope == current {
                    BtnKind::Subtle
                } else {
                    BtnKind::Ghost
                })
                .selected(scope == current)
                .disabled(!enabled)
                .compact()
                .on_click(move |_, cx| {
                    this.update(cx, |w, cx| {
                        w.mcp_scope = scope;
                        cx.notify();
                    });
                })
        };
        let submit = this.clone();
        let add = card(cx)
            .child(
                div().px(px(16.0)).pt(px(12.0)).font_weight(gpui::FontWeight::MEDIUM).child("Add a server"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .px(px(16.0))
                    .py(px(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(4.0))
                            .child(kind_button("mcp-kind-stdio", "Command", McpKind::Stdio, self.mcp_kind))
                            .child(kind_button("mcp-kind-http", "URL", McpKind::Http, self.mcp_kind))
                            .child(div().w(px(12.0)))
                            .child(scope_button("mcp-scope-user", "For me", McpScope::User, self.mcp_scope, true))
                            .child(scope_button(
                                "mcp-scope-project",
                                "This project",
                                McpScope::Project,
                                self.mcp_scope,
                                mcp.project_available,
                            )),
                    )
                    .child(div().w_full().child(self.mcp_name.clone()))
                    .child(div().w_full().child(self.mcp_target.clone()))
                    .child(div().w_full().child(self.mcp_values.clone()))
                    .child(
                        div().text_size(t.small_size()).text_color(c.text_faint).child(
                            "Values one per line as NAME=value. Use $NAME to read your environment or secret:key for the private secrets file; literal values are stored but never shown again.",
                        ),
                    )
                    .child(
                        div().flex().child(
                            Btn::new("mcp-add")
                                .label("Add server")
                                .kind(BtnKind::Primary)
                                .on_click(move |_, cx| {
                                    submit.update(cx, |w, cx| w.submit_mcp_server(cx));
                                }),
                        ),
                    ),
            );
        root = root.child(add);
        // Import.
        let mut import = card(cx)
            .child(
                div().px(px(16.0)).pt(px(12.0)).font_weight(gpui::FontWeight::MEDIUM).child("Import a configuration"),
            )
            .child(
                div().px(px(16.0)).text_size(t.small_size()).text_color(c.text_muted).child(
                    "Paste an mcpServers block from another tool. Nothing is written or started until you choose what to import.",
                ),
            )
            .child(div().px(px(16.0)).py(px(8.0)).w_full().child(self.mcp_import.clone()));
        let mut import_actions = div()
            .flex()
            .flex_wrap()
            .gap(px(4.0))
            .px(px(16.0))
            .pb(px(12.0));
        {
            let this = this.clone();
            import_actions = import_actions.child(
                Btn::new("mcp-import-preview")
                    .label("Preview")
                    .kind(BtnKind::Subtle)
                    .on_click(move |_, cx| {
                        this.update(cx, |w, cx| {
                            let text = w.mcp_import.read(cx).text();
                            w.mcp_dispatch(Command::McpPreviewImport(text), cx);
                        });
                    }),
            );
        }
        if let Some(preview) = &mcp.import {
            let mut rows = div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .px(px(16.0))
                .pb(px(8.0));
            for (i, entry) in preview.entries.iter().enumerate() {
                let this = this.clone();
                let n = entry.name.clone();
                let mut row = div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        Btn::new(("mcp-import-entry", i))
                            .label(if entry.selected { "Import" } else { "Skip" })
                            .kind(if entry.selected {
                                BtnKind::Subtle
                            } else {
                                BtnKind::Ghost
                            })
                            .selected(entry.selected)
                            .disabled(!entry.valid)
                            .compact()
                            .on_click(move |_, cx| {
                                this.update(cx, |w, cx| {
                                    w.mcp_dispatch(Command::McpToggleImportEntry(n.clone()), cx)
                                });
                            }),
                    )
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(entry.name.clone()),
                    )
                    .child(chip(entry.kind.clone(), c.text_muted, c.bg_active, cx))
                    .child(
                        div()
                            .text_size(t.small_size())
                            .font_family(t.mono_font())
                            .text_color(c.text_muted)
                            .child(entry.summary.clone()),
                    );
                if entry.exists {
                    row = row.child(chip("replaces existing", c.warning, c.bg_active, cx));
                }
                for w in &entry.warnings {
                    row = row.child(
                        div()
                            .text_size(t.small_size())
                            .text_color(c.warning)
                            .child(w.clone()),
                    );
                }
                rows = rows.child(row);
            }
            for w in &preview.warnings {
                rows = rows.child(
                    div()
                        .text_size(t.small_size())
                        .text_color(c.warning)
                        .child(w.clone()),
                );
            }
            import = import.child(rows);
            {
                let this = this.clone();
                import_actions = import_actions.child(
                    Btn::new("mcp-import-user")
                        .label("Import for me")
                        .kind(BtnKind::Primary)
                        .on_click(move |_, cx| {
                            this.update(cx, |w, cx| {
                                w.mcp_dispatch(Command::McpApplyImport(McpScope::User), cx)
                            });
                        }),
                );
            }
            {
                let this = this.clone();
                import_actions = import_actions.child(
                    Btn::new("mcp-import-project")
                        .label("Import into this project")
                        .kind(BtnKind::Subtle)
                        .disabled(!mcp.project_available)
                        .on_click(move |_, cx| {
                            this.update(cx, |w, cx| {
                                w.mcp_dispatch(Command::McpApplyImport(McpScope::Project), cx)
                            });
                        }),
                );
            }
            {
                let this = this.clone();
                import_actions = import_actions.child(
                    Btn::new("mcp-import-dismiss")
                        .label("Dismiss")
                        .kind(BtnKind::Ghost)
                        .on_click(move |_, cx| {
                            this.update(cx, |w, cx| w.mcp_dispatch(Command::McpDismissImport, cx));
                        }),
                );
            }
        }
        import = import.child(import_actions);
        root = root.child(import);
        root.into_any_element()
    }
}
