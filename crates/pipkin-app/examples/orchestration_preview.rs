//! Isolated visual fixture; no storage, engine, provider or network connection.
//! Run: cargo run -p pipkin-app --example orchestration_preview [-- --dense]
use gpui::AppContext;
use pipkin_core::{dashboard as d, *};
use pipkin_ui::model::Model;

fn main() {
    gpui_platform::application().with_assets(pipkin_ui::assets::Assets).run(|cx| {
        let mut state = AppState::new(Bootstrap { projects: vec![], models: vec![], conversations: vec![], now: 0 }, Prefs::default());
        state.prefs.window_size = Some(if std::env::args().any(|arg| arg == "--small") { (900., 760.) } else { (2400., 1450.) });
        if std::env::args().any(|arg| arg == "--light") { state.prefs.theme = Theme::Light; }
        state.dashboard.open = true;
        state.dashboard.available = true;
        state.dashboard.freshness = d::Freshness::Live;
        let mut threads: Vec<(&str, String, &str)> = [
            ("/demo/pipkin", "Refine orchestration overview", "running"),
            ("/demo/pipkin", "Fix composer focus", "ready"),
            ("/demo/pipkin", "Update shortcuts", "ready"),
            ("/demo/website", "Approve homepage copy", "waiting"),
            ("/demo/website", "Polish landing page", "running"),
            ("/demo/api-service", "Review webhook retries", "ready"),
            ("/demo/api-service", "Add request tracing", "running"),
            ("/demo/worker", "Choose retry policy", "waiting"),
            ("/demo/worker", "Trace queue latency", "running"),
        ].into_iter().map(|(path, title, status)| (path, title.to_owned(), status)).collect();
        if std::env::args().any(|arg| arg == "--dense") {
            threads = [("/demo/alpha", 1), ("/demo/beta", 1), ("/demo/docs", 9), ("/demo/service", 33), ("/demo/workspace", 89)]
                .into_iter().flat_map(|(path, count)| (0..count).map(move |i| (path, format!("Example thread {}", i + 1), "ready"))).collect();
        }
        let thread_count = threads.len() as u32;
        for (i, (path, title, status)) in threads.iter().enumerate() {
            let id = ConversationId(i as u64 + 1);
            let mut conversation = ConversationState::new(id, ProjectId(1), title.clone(), 0);
            conversation.opened = true;

            if *status == "waiting" {
                conversation.ui_requests.push(UiRequest {
                    id: format!("question-{i}"), kind: UiRequestKind::Select,
                    title: "Which retry policy should I use?".into(),
                    message: Some("Demo · simulated agent. Choose a policy to preview the response controls.".into()),
                    items: vec![UiRequestItem { value: "backoff".into(), label: "Exponential backoff".into(), description: Some("Three retries with jitter".into()) },
                        UiRequestItem { value: "fixed".into(), label: "Fixed delay".into(), description: Some("Wait five seconds between retries".into()) }],
                    placeholder: None, default_value: None, deadline: None, form: None, source: None,
                });
            }
            let generation = conversation.generation;
            state.conversations.push(conversation);
            state.apply_event(BackendEvent { conversation: id, generation, op: None, kind: EventKind::ConversationLifecycleReady(true) });
            let worker: d::ActivityRow = serde_json::from_value(serde_json::json!({
                "session": format!("preview-{i}"), "root": format!("preview-{i}"), "revision":1,
                "kind":"root", "cwd": path, "title": title, "model":"Demo model",
                "state": if *status == "waiting" { serde_json::json!({"state":"waiting", "reason":"question"}) } else { serde_json::json!({"state":status}) },
                "createdAtMs":i, "updatedAtMs":0, "sortAtMs":i
            })).unwrap();
            state.dashboard.rows.push(d::Row { worker, conversation: Some(id), root: Some(id) });
        }
        state.dashboard.counts = Some(d::ActivityCounts {
            attention: threads.iter().filter(|(_, _, status)| *status == "waiting").count() as u32,
            running: threads.iter().filter(|(_, _, status)| *status == "running").count() as u32,
            total: thread_count, ..Default::default()
        });
        state.selected = Some(ConversationId(1));
        let model = cx.new(|_| Model::new(state));
        model.update(cx, |m, _| m.set_effect_handler(Box::new(|effect, cx| {
            if let Effect::Backend(BackendRequest::Dashboard(d::Request::Detail { generation, session, .. })) = effect {
                let entity = cx.entity();
                cx.defer(move |cx| entity.update(cx, |m, cx| m.mutate(|s| s.dashboard_event(d::Event::Detail {
                    generation, session, before: None, oldest: None, has_older: false,
                    items: vec![TranscriptItem { id: ItemId(1), at: 0, native_only: false, kind: ItemKind::Assistant {
                        text: "Demo · simulated agent. The worker currently retries failed jobs immediately. Choose a retry policy or send the next prompt. No work is executed in this visual fixture.".into(), streaming: false } }],
                }), cx)));
            }
        })));
        if std::env::args().any(|arg| arg == "--decision") {
            model.update(cx, |m,cx|m.dispatch(Command::Dashboard(d::Command::Select("preview-7".into())),cx));
        }
        pipkin_ui::shell::open_main_window(cx, model, "/tmp/pipkin-orchestration-preview".into());
    });
}
