//! Prepared graph projection and viewport-aware layout. No I/O or GPUI dependencies.
use pipkin_core::dashboard::{Row, State, WorkState};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const CARD_W: f32 = 238.;
pub(super) const CARD_H: f32 = 132.;
pub(super) const PROJECT_H: f32 = 54.;
pub(super) const SLOT_W: f32 = 548.;
pub(super) const GROUP_W: f32 = 526.;
pub(super) const THREAD_Y: f32 = 76.;
pub(super) const ROW_PITCH: f32 = 148.;

/// Geometry in graph coordinates, plus the transform that fits the visible nodes.
pub(super) struct GraphLayout {
    pub origins: Vec<(f32, f32)>,
    pub server: (f32, f32),
    pub zoom: f32,
    pub offset: (f32, f32),
}

impl GraphLayout {
    pub fn fit(groups: &[(usize, bool)], viewport: (f32, f32)) -> Self {
        let width = (viewport.0 - 48.).max(1.);
        let height = (viewport.1 - 48.).max(1.);
        let heights: Vec<_> = groups
            .iter()
            .map(|&(visible, more)| {
                if visible == 0 && !more {
                    PROJECT_H + 48.
                } else {
                    THREAD_Y
                        + visible.div_ceil(2) as f32 * ROW_PITCH
                        + if more { 48. } else { 0. }
                        + 32.
                }
            })
            .collect();
        // Try each useful column count instead of choosing from unscaled card widths.
        // The winning packing gives the largest readable cards with all nodes in view.
        let mut best = (1, 0_f32, Vec::new());
        for columns in 1..=groups.len().clamp(1, 64) {
            let bands: Vec<f32> = heights
                .chunks(columns)
                .map(|row| row.iter().copied().fold(0., f32::max))
                .collect();
            let fit = (width / (columns as f32 * SLOT_W + 40.))
                .min(height / (150. + bands.iter().sum::<f32>()));
            if fit > best.1 {
                best = (columns, fit, bands);
            }
        }
        let (columns, fit, bands) = best;
        // Unlike manual zoom, Fit must be able to show very large graphs in full.
        let zoom = fit.clamp(f32::EPSILON, 1.6);
        let logical_width = width / zoom;
        let horizontal_gap = if columns > 1 {
            (logical_width - (columns as f32 * SLOT_W + 40.)).max(0.) / (columns - 1) as f32
        } else {
            0.
        };
        let extent_width =
            columns as f32 * SLOT_W + 40. + horizontal_gap * columns.saturating_sub(1) as f32;
        let vertical_gap =
            (height / zoom - 150. - bands.iter().sum::<f32>()).max(0.) / (bands.len() + 1) as f32;
        let mut origins = Vec::with_capacity(groups.len());
        let mut y = 150. + vertical_gap;
        for (band, &band_height) in bands.iter().enumerate() {
            let count = (groups.len() - band * columns).min(columns);
            let row_offset = (columns - count) as f32 * (SLOT_W + horizontal_gap) / 2.;
            for col in 0..count {
                origins.push((32. + row_offset + col as f32 * (SLOT_W + horizontal_gap), y));
            }
            y += band_height + vertical_gap;
        }
        Self {
            origins,
            server: (extent_width / 2. - 150., 22.),
            zoom,
            offset: ((viewport.0 - extent_width * zoom) / 2., 24.),
        }
    }
}

#[derive(Clone)]
pub(super) struct Project {
    pub path: String,
    pub name: String,
    pub rows: Vec<Row>,
    pub attention: usize,
    pub slot: usize,
}
#[derive(Default)]
pub(super) struct GraphState {
    pub projects: Vec<Project>,
    pub attention: Vec<String>,
    pub collapsed: BTreeSet<String>,
    pub limits: BTreeMap<String, usize>,
    slots: BTreeMap<String, usize>,
    pub list: bool,
    pub attention_only: bool,
    pub zoom: Option<f32>,
    pub rendered_zoom: f32,
    pub viewport: Option<(f32, f32)>,
    pub locate: Option<String>,
    pub pan: (f32, f32),
    pub dragging: Option<(f32, f32)>,
}

pub(super) fn needs_attention(row: &Row, state: &State) -> bool {
    use pipkin_core::dashboard::WaitReason;
    match row.worker.state {
        WorkState::Waiting {
            reason: WaitReason::Question | WaitReason::CliApproval,
        } => true,
        WorkState::Failed { .. } | WorkState::Interrupted => state
            .seen
            .get(&row.worker.session)
            .is_none_or(|rev| *rev < row.worker.revision),
        WorkState::Unknown => true,
        _ => false,
    }
}
impl GraphState {
    pub fn sync(&mut self, state: &State) {
        let mut groups: BTreeMap<String, Vec<Row>> = BTreeMap::new();
        self.attention = state
            .rows
            .iter()
            .filter(|r| needs_attention(r, state))
            .map(|r| r.worker.session.clone())
            .collect();
        self.attention.sort_by_key(|id| {
            state
                .rows
                .iter()
                .find(|r| &r.worker.session == id)
                .map(|r| (r.worker.sort_at_ms, id.clone()))
        });
        for row in &state.rows {
            groups
                .entry(row.worker.cwd.clone())
                .or_default()
                .push(row.clone());
        }
        self.projects = groups
            .into_iter()
            .map(|(path, mut rows)| {
                // Creation order, not status or token timing, determines position.
                rows.sort_by(|a, b| {
                    (a.worker.created_at_ms, &a.worker.session)
                        .cmp(&(b.worker.created_at_ms, &b.worker.session))
                });
                let next = self.slots.len();
                let slot = *self.slots.entry(path.clone()).or_insert(next);
                let name = super::project_name(&path);
                let attention = rows.iter().filter(|r| needs_attention(r, state)).count();
                Project {
                    path,
                    name,
                    rows,
                    attention,
                    slot,
                }
            })
            .collect();
        self.projects.sort_by_key(|p| p.slot);
    }
    pub fn reveal(&mut self, session: &str) {
        self.locate = Some(session.to_owned());
        if let Some(p) = self
            .projects
            .iter()
            .find(|p| p.rows.iter().any(|r| r.worker.session == session))
        {
            self.collapsed.remove(&p.path);
            if let Some(index) = p.rows.iter().position(|r| r.worker.session == session) {
                let limit = self.limits.entry(p.path.clone()).or_insert(4);
                *limit = (*limit).max((index / 4 + 1) * 4);
            }
        }
    }
    pub fn visible_rows<'a>(&self, p: &'a Project) -> Vec<&'a Row> {
        if self.collapsed.contains(&p.path) {
            return vec![];
        }
        p.rows
            .iter()
            .filter(|r| !self.attention_only || self.attention.contains(&r.worker.session))
            .take(*self.limits.get(&p.path).unwrap_or(&4))
            .collect()
    }
    pub fn thread_position(origin: (f32, f32), index: usize) -> (f32, f32) {
        (
            origin.0 + (index % 2) as f32 * 264.,
            origin.1 + THREAD_Y + (index / 2) as f32 * ROW_PITCH,
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use pipkin_core::dashboard::{ActivityRow, ExecutionBackend, WorkerKind};
    #[test]
    fn fit_uses_wide_canvas_and_compacts_short_project_rows() {
        // The reported layout: two one-thread projects and three paged projects.
        let groups = [(1, false), (1, false), (4, true), (4, true), (4, true)];
        let wide = GraphLayout::fit(&groups, (1500., 1000.));
        let tall = GraphLayout::fit(&groups, (700., 1200.));
        assert_eq!(wide.origins[0].1, wide.origins[2].1, "three across");
        assert!(wide.origins[3].1 > wide.origins[2].1);
        assert!(tall.origins[2].1 > tall.origins[0].1);
        assert!(
            wide.zoom > 0.8,
            "cards should not shrink to the old 55% floor"
        );
        let short = GraphLayout::fit(&[(1, false); 5], (700., 1200.));
        assert!(
            short.zoom > tall.zoom,
            "empty reserved rows must not limit fit"
        );
    }

    #[test]
    fn fit_contains_visible_nodes_at_small_large_and_extreme_aspect_ratios() {
        for viewport in [(300., 260.), (1500., 1000.), (4000., 1000.), (700., 1800.)] {
            for groups in [
                vec![(1, false), (1, false), (4, true), (4, true), (4, true)],
                vec![(0, false); 5],
                vec![(12, true); 25],
            ] {
                let layout = GraphLayout::fit(&groups, viewport);
                let screen = |(x, y): (f32, f32)| {
                    (
                        x * layout.zoom + layout.offset.0,
                        y * layout.zoom + layout.offset.1,
                    )
                };
                let (x, y) = screen(layout.server);
                assert!(x >= 0. && y >= 0.);
                assert!(x + 300. * layout.zoom <= viewport.0 + 0.1);
                for (i, &(visible, more)) in groups.iter().enumerate() {
                    let origin = layout.origins[i];
                    let (x, y) = screen(origin);
                    assert!(x >= 0. && y >= 0.);
                    assert!(x + 502. * layout.zoom <= viewport.0 + 0.1);
                    let bottom = if visible == 0 && !more {
                        origin.1 + PROJECT_H
                    } else {
                        origin.1
                            + THREAD_Y
                            + visible.div_ceil(2) as f32 * ROW_PITCH
                            + if more { 48. } else { 0. }
                    };
                    assert!(screen((0., bottom)).1 <= viewport.1 + 0.1);
                }
            }
        }
    }

    pub(crate) fn row(session: &str, path: &str, state: WorkState) -> Row {
        Row {
            conversation: None,
            root: None,
            worker: ActivityRow {
                session: session.into(),
                revision: 1,
                kind: WorkerKind::Root,
                root: session.into(),
                parent: None,
                child: None,
                cwd: path.into(),
                title: Some(session.into()),
                task_excerpt: None,
                profile: None,
                origin: None,
                continues: None,
                execution_backend: ExecutionBackend::NativeAgent,
                local_cli_mode: None,
                model: None,
                state,
                raw_status: None,
                run: None,
                children_active: 0,
                jobs_active: 0,
                has_result: false,
                created_at_ms: 0,
                updated_at_ms: 0,
                sort_at_ms: 0,
            },
        }
    }
    #[test]
    fn selecting_an_early_thread_preserves_an_expanded_group() {
        let mut graph = GraphState::default();
        graph.sync(&State {
            rows: vec![row("one", "/app", WorkState::Ready)],
            ..Default::default()
        });
        graph.limits.insert("/app".into(), 12);
        graph.reveal("one");
        assert_eq!(graph.limits["/app"], 12);
    }

    #[test]
    fn distinct_full_paths_and_stable_slots_survive_updates() {
        let mut state = State {
            rows: vec![
                row("a", "/one/app", WorkState::Running),
                row("b", "/two/app", WorkState::Ready),
            ],
            ..Default::default()
        };
        let mut graph = GraphState::default();
        graph.sync(&state);
        assert_eq!(graph.projects.len(), 2);
        let old: Vec<_> = graph
            .projects
            .iter()
            .map(|p| (p.path.clone(), p.slot))
            .collect();
        state.rows.reverse();
        state.rows[0].worker.state = WorkState::Running;
        state.rows.push(row("c", "/aaa/new", WorkState::Ready));
        graph.sync(&state);
        for (path, slot) in old {
            assert_eq!(
                graph.projects.iter().find(|p| p.path == path).unwrap().slot,
                slot
            );
        }
    }
    #[test]
    fn reveal_expands_hidden_target_and_attention_is_authoritative() {
        let mut state = State::default();
        for i in 0..9 {
            state
                .rows
                .push(row(&format!("task-{i}"), "/app", WorkState::Ready));
        }
        state.rows[8].worker.state = WorkState::Waiting {
            reason: pipkin_core::dashboard::WaitReason::Question,
        };
        let mut graph = GraphState::default();
        graph.sync(&state);
        graph.collapsed.insert("/app".into());
        assert!(graph.visible_rows(&graph.projects[0]).is_empty());
        graph.reveal("task-8");
        assert_eq!(graph.visible_rows(&graph.projects[0]).len(), 9);
        assert_eq!(graph.attention, vec!["task-8"]);
        state.rows[8].worker.state = WorkState::Running;
        graph.sync(&state);
        assert!(graph.attention.is_empty());
    }
    #[test]
    fn input_wait_and_seen_failures_do_not_invent_attention() {
        let mut state = State::default();
        let input = row(
            "input",
            "/app",
            WorkState::Waiting {
                reason: pipkin_core::dashboard::WaitReason::Input,
            },
        );
        assert!(!needs_attention(&input, &state));
        let failed = row("failed", "/app", WorkState::Failed { detail: None });
        assert!(needs_attention(&failed, &state));
        state.seen.insert("failed".into(), 1);
        assert!(!needs_attention(&failed, &state));
    }
}
