//! Native ports of the reference example plugins
//! (`coding-agent/examples/plugins/{pi-example-questions,pi-example-plugin}`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use pipkin_engine_protocol::{Answer, NoticeLevel, ProtoError};
use tokio_util::sync::CancellationToken;

use super::questions::option;
use super::{Admission, BoxFuture, Extension, ExtensionHost, Question};

/// `pi-example-questions` (`src/session.ts`): on activation, asks a choice, a yes/no and a line
/// of text, reports each answer as a notice, then sets a status line. Unanswered questions
/// time out instead of blocking.
pub struct QuestionsExample;

pub const QUESTIONS_EXAMPLE_ID: &str = "pi-example-questions";

impl Extension for QuestionsExample {
    fn id(&self) -> &str {
        QUESTIONS_EXAMPLE_ID
    }

    fn activate(&self, host: Arc<dyn ExtensionHost>) -> BoxFuture<'static, ()> {
        Box::pin(ask_three_questions(host))
    }
}

async fn ask_three_questions(ui: Arc<dyn ExtensionHost>) {
    // The reference passes BACKGROUND_CONTEXT: nothing but the runner's shutdown stops it.
    let never = CancellationToken::new();
    let flavor = ui
        .ask(
            Question::select(
                "Pick a flavor",
                vec![
                    option("mint", "Mint", Some("cool and sharp")),
                    option("vanilla", "Vanilla", Some("plain and sweet")),
                    option("pepper", "Pepper", None),
                ],
            )
            .with_message("The example extension is asking."),
            None,
            never.clone(),
        )
        .await
        .and_then(text_of);
    ui.notify(
        NoticeLevel::Info,
        &match &flavor {
            None => "No flavor was picked.".to_string(),
            Some(flavor) => format!("You picked {flavor}."),
        },
    );
    let keep_going = ui
        .ask(
            Question::confirm("Keep going?").with_message("Answer yes or no."),
            None,
            never.clone(),
        )
        .await
        .and_then(|answer| match answer {
            Answer::Confirm { value } => Some(value),
            _ => None,
        });
    ui.notify(
        NoticeLevel::Info,
        &match keep_going {
            None => "No answer to keep going.".to_string(),
            Some(value) => format!("Keep going: {value}."),
        },
    );
    let name = ui
        .ask(
            Question::input("What should it be called?")
                .with_placeholder("a name")
                .with_default("pipsqueak"),
            None,
            never,
        )
        .await
        .and_then(text_of);
    ui.notify(
        NoticeLevel::Info,
        &match &name {
            None => "No name was given.".to_string(),
            Some(name) => format!("Called {name}."),
        },
    );
    let status = format!("flavor {}", flavor.as_deref().unwrap_or("none"));
    ui.set_status("example-questions", Some(&status));
}

fn text_of(answer: Answer) -> Option<String> {
    match answer {
        Answer::Choice { value } | Answer::Text { value } => Some(value),
        Answer::Confirm { .. } => None,
    }
}

/// `pi-example-plugin`'s Session facet (`src/session.ts`): a greeting service that counts the
/// facet's activations. The reference publishes it as the remote service
/// `pi.example-plugin.greeting` for its TUI facet; native extensions cannot publish services to
/// presentations, so here it is a plain method on the extension.
#[derive(Default)]
pub struct GreetingExample {
    activations: AtomicU64,
}

pub const GREETING_EXAMPLE_ID: &str = "pi-example-plugin";

/// `ExampleFacetReply` (`src/contract.ts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GreetingReply {
    pub message: String,
    pub worker_activations: u64,
}

impl GreetingExample {
    pub fn new() -> GreetingExample {
        GreetingExample::default()
    }

    /// `ExampleFacetService.greet`.
    pub fn greet(&self, name: &str) -> GreetingReply {
        GreetingReply {
            message: format!("Hello {name} from the bundled Session worker facet!!!"),
            worker_activations: self.activations.load(Ordering::SeqCst),
        }
    }

    /// The session side of the reference TUI facet's `/hello [name]` command (`src/tui.ts`):
    /// greet, then prompt the agent with `pong: <name>`. Returns the line the TUI shows with
    /// `PresentationUI.showStatus` (a presentation-only service with no native equivalent) and
    /// the prompt's admission.
    pub async fn hello(
        &self,
        host: &dyn ExtensionHost,
        args: &str,
    ) -> (String, Result<Admission, ProtoError>) {
        let message = if args.is_empty() {
            "from the TUI"
        } else {
            args
        };
        let reply = self.greet(message);
        let shown = format!(
            "We got this: {} Worker activations: {}.",
            reply.message, reply.worker_activations
        );
        let admitted = host.prompt(format!("pong: {message}"), None).await;
        (shown, admitted)
    }
}

impl Extension for GreetingExample {
    fn id(&self) -> &str {
        GREETING_EXAMPLE_ID
    }

    fn activate(&self, _host: Arc<dyn ExtensionHost>) -> BoxFuture<'static, ()> {
        self.activations.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}
