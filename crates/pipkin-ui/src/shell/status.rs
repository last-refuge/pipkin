//! The compact status line (plan §11, STATUS-1): phase, queued inputs, running jobs and
//! subagents, open questions, context occupancy and session usage, derived from core state
//! on each render. Empty segments are omitted; secondary ones collapse at narrow widths.
//! The line is one status region with a full accessible sentence, and the usage chips open
//! the existing Session usage dialog. Nothing animates and nothing is announced on its own.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Role, StatefulInteractiveElement,
    Styled, div, px,
};
use pipkin_core::{ContextSource, StatusLine, StatusPhase};

use super::controls::{Btn, BtnKind, chip, icon};
use super::workspace::{Overlay, Workspace};
use crate::theme::ActiveTheme;

/// `41k`, `1.2M`, `950`.
pub(super) fn compact_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        if m >= 10.0 {
            format!("{m:.0}M")
        } else {
            format!("{m:.1}M")
        }
    } else if n >= 1_000 {
        let k = n as f64 / 1_000.0;
        if k >= 100.0 {
            format!("{k:.0}k")
        } else {
            format!("{k:.1}k")
        }
    } else {
        n.to_string()
    }
}

/// `41k / 200k · 21%` with the source marked; `41k (estimated)` without a capacity.
pub(super) fn context_label(line: &StatusLine) -> Option<String> {
    let context = line.context.as_ref()?;
    let tokens = context.tokens?;
    let source = match context.source {
        ContextSource::Reported => "",
        ContextSource::Estimated => " ~",
        ContextSource::Unavailable => return None,
    };
    let mut label = match (context.capacity, context.percent()) {
        (Some(capacity), Some(percent)) => format!(
            "{}{} / {} · {percent}%",
            compact_tokens(tokens),
            source,
            compact_tokens(capacity)
        ),
        _ => format!("{}{} tokens", compact_tokens(tokens), source),
    };
    if context.stale {
        label.push_str(" · stale");
    }
    match context.compaction.as_str() {
        "background" => label.push_str(" · compacting"),
        "blocking" => label.push_str(" · compacting next"),
        _ => {}
    }
    Some(label)
}

impl Workspace {
    /// The status strip for the composer toolbar; `None` without an open conversation.
    pub(super) fn render_status_line(
        &self,
        narrow: bool,
        cx: &Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let line = self.state(cx).status_line()?;
        let t = cx.theme().clone();
        let c = t.colors.clone();
        let this = cx.entity();
        let mut row = div()
            .id("status-line")
            .role(Role::Status)
            .aria_label(line.accessible())
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.0))
            .min_w_0();
        let (phase_fg, phase_icon) = match line.phase {
            StatusPhase::Idle => (c.text_faint, None),
            StatusPhase::Submitting | StatusPhase::Working => (c.accent, Some("loader-circle")),
            StatusPhase::Stopping => (c.warning, Some("square")),
            StatusPhase::Unknown => (c.warning, Some("circle-help")),
            StatusPhase::Failed => (c.danger, Some("circle-alert")),
            StatusPhase::Waiting => (c.warning, Some("circle-help")),
        };
        if line.phase != StatusPhase::Idle {
            let mut phase = chip(line.phase.label(), phase_fg, c.bg_active, cx);
            if let Some(name) = phase_icon {
                phase = phase.child(icon(name, px(12.0 * t.scale.max(1.0)), phase_fg));
            }
            row = row.child(phase);
        }
        if line.queued > 0 {
            row = row.child(chip(
                format!("{} queued", line.queued),
                c.text_muted,
                c.bg_active,
                cx,
            ));
        }
        if line.open_questions > 0 {
            row = row.child(chip(
                format!(
                    "{} question{}",
                    line.open_questions,
                    if line.open_questions == 1 { "" } else { "s" }
                ),
                c.warning,
                c.bg_active,
                cx,
            ));
        }
        if !narrow {
            if line.active_jobs > 0 {
                row = row.child(chip(
                    format!(
                        "{} job{}",
                        line.active_jobs,
                        if line.active_jobs == 1 { "" } else { "s" }
                    ),
                    c.text_muted,
                    c.bg_active,
                    cx,
                ));
            }
            if line.active_children > 0 {
                row = row.child(chip(
                    format!(
                        "{} agent{}",
                        line.active_children,
                        if line.active_children == 1 { "" } else { "s" }
                    ),
                    c.text_muted,
                    c.bg_active,
                    cx,
                ));
            }
        }
        if let Some(label) = context_label(&line) {
            let over = line
                .context
                .as_ref()
                .and_then(|x| x.percent())
                .is_some_and(|p| p >= 80);
            let open = this.clone();
            row = row.child(
                Btn::new("status-context")
                    .label(label)
                    .kind(BtnKind::Ghost)
                    .compact()
                    .aria("Context usage; opens session usage")
                    .selected(over)
                    .on_click(move |window, cx| {
                        open.update(cx, |w, cx| w.open_overlay(Overlay::Session, window, cx));
                    }),
            );
        }
        if !narrow && let Some(total) = line.total_tokens {
            let mut label = format!("{} tok", compact_tokens(total));
            if let Some(cost) = line.cost_usd {
                label.push_str(&format!(" · {}", super::overlays::format_cost(Some(cost))));
            }
            let open = this.clone();
            row = row.child(
                Btn::new("status-usage")
                    .label(label)
                    .kind(BtnKind::Ghost)
                    .compact()
                    .aria("Session usage")
                    .on_click(move |window, cx| {
                        open.update(cx, |w, cx| w.open_overlay(Overlay::Session, window, cx));
                    }),
            );
        }
        Some(row.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipkin_core::ContextUsage;

    #[test]
    fn labels_are_compact_and_mark_estimates_and_staleness() {
        assert_eq!(compact_tokens(950), "950");
        assert_eq!(compact_tokens(41_250), "41.2k");
        assert_eq!(compact_tokens(200_000), "200k");
        assert_eq!(compact_tokens(1_234_567), "1.2M");
        let mut line = StatusLine {
            model: None,
            effort: None,
            phase: StatusPhase::Idle,
            queued: 0,
            active_jobs: 0,
            active_children: 0,
            open_questions: 0,
            context: None,
            total_tokens: None,
            cost_usd: None,
        };
        assert_eq!(context_label(&line), None);
        line.context = Some(ContextUsage {
            tokens: Some(41_250),
            source: ContextSource::Reported,
            capacity: Some(200_000),
            reserve: None,
            remaining: None,
            model: None,
            stale: false,
            compaction: "idle".into(),
            revision: 1,
        });
        assert_eq!(context_label(&line).as_deref(), Some("41.2k / 200k · 20%"));
        let ctx = line.context.as_mut().unwrap();
        ctx.source = ContextSource::Estimated;
        ctx.stale = true;
        ctx.compaction = "background".into();
        assert_eq!(
            context_label(&line).as_deref(),
            Some("41.2k ~ / 200k · 20% · stale · compacting")
        );
        let ctx = line.context.as_mut().unwrap();
        ctx.capacity = None;
        ctx.stale = false;
        ctx.compaction = "idle".into();
        assert_eq!(context_label(&line).as_deref(), Some("41.2k ~ tokens"));
        let ctx = line.context.as_mut().unwrap();
        ctx.source = ContextSource::Unavailable;
        assert_eq!(context_label(&line), None);
    }
}
