//! Loopback OAuth redirect handler (reference `auth/oauth/callback-server.ts`).
//!
//! Listens on `host:port` (port `0` = any free port), answers only `GET <path>`, checks `state`
//! when one is expected, and hands the query to `complete` before the browser page is sent so
//! the page can show exchange failures. Pages: 404 unknown route, 400 state mismatch / missing
//! code / provider error, 409 already handled, 502 exchange failure, 200 success.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::{CANCELLED, Interaction, Prompt};
use crate::provider::BoxFuture;

pub type Query = HashMap<String, String>;
pub type Complete<T> = Arc<dyn Fn(Query) -> BoxFuture<'static, Result<T, String>> + Send + Sync>;
/// Rejects a callback with a 400 page while continuing to wait (e.g. a missing field).
pub type Validate = Arc<dyn Fn(&Query) -> Result<(), String> + Send + Sync>;

pub struct CallbackOptions {
    /// Shown on the browser page, e.g. `OpenAI`.
    pub provider_name: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    /// Host in the redirect URI when it differs from `host` (e.g. `localhost`).
    pub redirect_host: Option<String>,
    pub state: Option<String>,
    pub timeout: Option<Duration>,
    pub validate: Option<Validate>,
}

struct Shared<T> {
    claimed: bool,
    settled: bool,
    tx: Option<oneshot::Sender<Result<T, String>>>,
}

impl<T> Shared<T> {
    fn finish(&mut self, result: Result<T, String>) {
        if self.settled {
            return;
        }
        self.settled = true;
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(result);
        }
    }
}

pub struct CallbackServer<T> {
    pub redirect_uri: String,
    rx: tokio::sync::Mutex<Option<oneshot::Receiver<Result<T, String>>>>,
    shared: Arc<Mutex<Shared<T>>>,
    stop_waiting: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl<T> Drop for CallbackServer<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn page(status: u16, title: &str, detail: Option<&str>) -> Vec<u8> {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let mut body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Pipkin sign-in</title></head><body><h1>{}</h1>",
        esc(title)
    );
    if let Some(d) = detail {
        body.push_str(&format!("<p>{}</p>", esc(d)));
    }
    body.push_str("</body></html>");
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Bad Gateway",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: text/html; charset=utf-8\r\ncache-control: no-store\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

async fn read_target(sock: &mut TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
            break;
        }
        let n = tokio::time::timeout(Duration::from_secs(10), sock.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let mut first = head.lines().next()?.split_whitespace();
    Some((first.next()?.to_owned(), first.next()?.to_owned()))
}

impl<T: Send + 'static> CallbackServer<T> {
    /// Bind and start serving. Fails when the address cannot be bound.
    pub async fn start(
        opts: CallbackOptions,
        complete: Complete<T>,
        cancel: &CancellationToken,
    ) -> std::io::Result<CallbackServer<T>> {
        if cancel.is_cancelled() {
            return Err(std::io::Error::other(CANCELLED));
        }
        let listener = TcpListener::bind((opts.host.as_str(), opts.port)).await?;
        let port = listener.local_addr()?.port();
        let (tx, rx) = oneshot::channel();
        let shared = Arc::new(Mutex::new(Shared {
            claimed: false,
            settled: false,
            tx: Some(tx),
        }));
        let redirect_host = opts.redirect_host.clone().unwrap_or(opts.host.clone());
        let redirect_host = if redirect_host.contains(':') {
            format!("[{redirect_host}]")
        } else {
            redirect_host
        };
        let redirect_uri = format!("http://{redirect_host}:{port}{}", opts.path);
        let opts = Arc::new(opts);
        let task_shared = shared.clone();
        let cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let mut conns = JoinSet::new();
            let deadline = opts.timeout.map(|t| tokio::time::Instant::now() + t);
            loop {
                let timed_out = async {
                    match deadline {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    _ = cancel.cancelled() => {
                        lock(&task_shared).finish(Err(CANCELLED.into()));
                        return;
                    }
                    _ = timed_out => {
                        lock(&task_shared).finish(Err(format!("{} sign-in timed out", opts.provider_name)));
                        return;
                    }
                    accepted = listener.accept() => {
                        let Ok((sock, _)) = accepted else { continue };
                        let shared = task_shared.clone();
                        let opts = opts.clone();
                        let complete = complete.clone();
                        conns.spawn(handle(sock, shared, opts, complete));
                    }
                }
            }
        });
        Ok(CallbackServer {
            redirect_uri,
            rx: tokio::sync::Mutex::new(Some(rx)),
            shared,
            stop_waiting: CancellationToken::new(),
            task,
        })
    }

    /// The result of `complete`, an error, or `None` after [`CallbackServer::cancel`].
    pub async fn wait(&self) -> Option<Result<T, String>> {
        let mut guard = self.rx.lock().await;
        let rx = guard.as_mut()?;
        tokio::select! {
            r = rx => Some(r.unwrap_or_else(|_| Err("OAuth callback server closed".into()))),
            _ = self.stop_waiting.cancelled() => {
                if lock(&self.shared).claimed {
                    // A callback is being completed; its result still counts.
                    let r = guard.take()?.await;
                    Some(r.unwrap_or_else(|_| Err("OAuth callback server closed".into())))
                } else {
                    None
                }
            }
        }
    }

    /// Stop waiting for the browser unless a callback is already being completed.
    pub fn cancel(&self) {
        self.stop_waiting.cancel();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

async fn handle<T: Send + 'static>(
    mut sock: TcpStream,
    shared: Arc<Mutex<Shared<T>>>,
    opts: Arc<CallbackOptions>,
    complete: Complete<T>,
) {
    let Some((method, target)) = read_target(&mut sock).await else {
        return;
    };
    let reply = |status: u16, title: &str, detail: Option<&str>| page(status, title, detail);
    let url = url::Url::parse(&format!("http://localhost{target}")).ok();
    let Some(url) = url.filter(|u| method == "GET" && u.path() == opts.path) else {
        let _ = sock
            .write_all(&reply(404, "Callback route not found.", None))
            .await;
        return;
    };
    let query: Query = url.query_pairs().into_owned().collect();
    if let Some(expected) = &opts.state
        && query.get("state") != Some(expected)
    {
        let _ = sock.write_all(&reply(400, "State mismatch.", None)).await;
        return;
    }
    let handled = {
        let s = lock(&shared);
        s.claimed || s.settled
    };
    if handled {
        let _ = sock
            .write_all(&reply(409, "This sign-in has already been handled.", None))
            .await;
        return;
    }
    if let Some(error) = query.get("error") {
        let description = query.get("error_description").unwrap_or(error).clone();
        let _ = sock
            .write_all(&reply(
                400,
                &format!("{} authorization failed.", opts.provider_name),
                Some(&description),
            ))
            .await;
        lock(&shared).finish(Err(format!(
            "{} authorization failed: {description}",
            opts.provider_name
        )));
        return;
    }
    if query.get("code").is_none_or(|c| c.is_empty()) {
        let _ = sock
            .write_all(&reply(400, "Missing authorization code.", None))
            .await;
        return;
    }
    if let Some(validate) = &opts.validate
        && let Err(e) = validate(&query)
    {
        let _ = sock.write_all(&reply(400, &e, None)).await;
        return;
    }
    let claimed = {
        let mut s = lock(&shared);
        let free = !(s.claimed || s.settled);
        if free {
            s.claimed = true;
        }
        free
    };
    if !claimed {
        let _ = sock
            .write_all(&reply(409, "This sign-in has already been handled.", None))
            .await;
        return;
    }
    match complete(query).await {
        Ok(v) => {
            let _ = sock
                .write_all(&reply(
                    200,
                    &format!(
                        "Signed in to {}. You may now close this page.",
                        opts.provider_name
                    ),
                    None,
                ))
                .await;
            lock(&shared).finish(Ok(v));
        }
        Err(e) => {
            let _ = sock
                .write_all(&reply(
                    502,
                    &format!("{} sign-in failed.", opts.provider_name),
                    Some(&e),
                ))
                .await;
            lock(&shared).finish(Err(e));
        }
    }
    let _ = sock.shutdown().await;
}

/// Who finished the browser step.
#[derive(Debug, PartialEq, Eq)]
pub enum CallbackOrManual<T> {
    Callback(T),
    Manual(String),
}

/// Wait for the browser callback, or for the user to paste the code or redirect URL when the
/// browser cannot reach the loopback server. Without a server only the manual prompt is used.
pub async fn wait_for_callback_or_manual<T: Send + 'static>(
    io: &Interaction,
    callback: Option<&CallbackServer<T>>,
    message: &str,
    placeholder: &str,
) -> Result<CallbackOrManual<T>, String> {
    let withdraw = CancellationToken::new();
    let manual = io.prompt_until(Prompt::manual_code(message, placeholder), &withdraw);
    tokio::pin!(manual);
    let result = match callback {
        None => manual.await.map(CallbackOrManual::Manual),
        Some(cb) => {
            tokio::select! {
                r = cb.wait() => match r {
                    Some(Ok(v)) => Ok(CallbackOrManual::Callback(v)),
                    Some(Err(e)) => Err(e),
                    None => (&mut manual).await.map(CallbackOrManual::Manual),
                },
                m = &mut manual => {
                    cb.cancel();
                    match m {
                        Ok(input) => match cb.wait().await {
                            Some(Ok(v)) => Ok(CallbackOrManual::Callback(v)),
                            Some(Err(e)) => Err(e),
                            None => Ok(CallbackOrManual::Manual(input)),
                        },
                        Err(e) => Err(e),
                    }
                }
            }
        }
    };
    withdraw.cancel();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::oauth::{AuthEvent, InteractionSink};

    fn opts(timeout: Option<Duration>) -> CallbackOptions {
        CallbackOptions {
            provider_name: "Test".into(),
            host: "127.0.0.1".into(),
            port: 0,
            path: "/cb".into(),
            redirect_host: None,
            state: Some("s".into()),
            timeout,
            validate: None,
        }
    }

    fn slow_complete() -> Complete<String> {
        Arc::new(|q: Query| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                let code = q.get("code").cloned().unwrap_or_default();
                if code == "bad" {
                    Err("exchange failed".into())
                } else {
                    Ok(code)
                }
            })
        })
    }

    async fn get(url: String) -> u16 {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(url)
            .send()
            .await
            .map(|r| r.status().as_u16())
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn a_second_callback_while_completing_gets_409() {
        let server = CallbackServer::start(opts(None), slow_complete(), &CancellationToken::new())
            .await
            .unwrap();
        let uri = server.redirect_uri.clone();
        let first = tokio::spawn(get(format!("{uri}?code=one&state=s")));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(get(format!("{uri}?code=two&state=s")).await, 409);
        assert_eq!(get(format!("{uri}?error=denied&state=s")).await, 409);
        assert_eq!(first.await.unwrap(), 200);
        assert_eq!(server.wait().await, Some(Ok("one".into())));
    }

    #[tokio::test]
    async fn provider_errors_exchange_failures_cancel_and_timeout() {
        let server = CallbackServer::start(opts(None), slow_complete(), &CancellationToken::new())
            .await
            .unwrap();
        let uri = server.redirect_uri.clone();
        assert_eq!(get(format!("{uri}?state=s")).await, 400);
        assert_eq!(get(format!("{uri}?code=bad&state=s")).await, 502);
        assert_eq!(server.wait().await, Some(Err("exchange failed".into())));

        let server = CallbackServer::start(opts(None), slow_complete(), &CancellationToken::new())
            .await
            .unwrap();
        let uri = server.redirect_uri.clone();
        assert_eq!(
            get(format!(
                "{uri}?error=access_denied&error_description=no&state=s"
            ))
            .await,
            400
        );
        assert_eq!(
            server.wait().await,
            Some(Err("Test authorization failed: no".into()))
        );

        let server = CallbackServer::start(opts(None), slow_complete(), &CancellationToken::new())
            .await
            .unwrap();
        server.cancel();
        assert_eq!(server.wait().await, None);

        let server = CallbackServer::start(
            opts(Some(Duration::from_millis(50))),
            slow_complete(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            server.wait().await,
            Some(Err("Test sign-in timed out".into()))
        );

        let cancel = CancellationToken::new();
        let server = CallbackServer::start(opts(None), slow_complete(), &cancel)
            .await
            .unwrap();
        cancel.cancel();
        assert_eq!(server.wait().await, Some(Err(CANCELLED.into())));
    }

    /// Answers the manual prompt after a delay (or never).
    struct Paste(Option<(Duration, String)>);

    impl InteractionSink for Paste {
        fn prompt(&self, _: Prompt) -> BoxFuture<'static, Result<String, String>> {
            let answer = self.0.clone();
            Box::pin(async move {
                match answer {
                    Some((d, s)) => {
                        tokio::time::sleep(d).await;
                        Ok(s)
                    }
                    None => std::future::pending().await,
                }
            })
        }

        fn notify(&self, _: AuthEvent) {}
    }

    #[tokio::test]
    async fn the_callback_and_the_manual_paste_race() {
        let fast_complete: Complete<String> =
            Arc::new(|q: Query| Box::pin(async move { Ok(q["code"].clone()) }));
        let io = Interaction::new(Arc::new(Paste(None)), CancellationToken::new());
        let server = CallbackServer::start(opts(None), fast_complete.clone(), &io.cancel)
            .await
            .unwrap();
        let uri = server.redirect_uri.clone();
        tokio::spawn(get(format!("{uri}?code=cb&state=s")));
        assert_eq!(
            wait_for_callback_or_manual(&io, Some(&server), "m", "p").await,
            Ok(CallbackOrManual::Callback("cb".into()))
        );
        let io = Interaction::new(
            Arc::new(Paste(Some((Duration::from_millis(10), "pasted".into())))),
            CancellationToken::new(),
        );
        let server = CallbackServer::start(opts(None), fast_complete, &io.cancel)
            .await
            .unwrap();
        assert_eq!(
            wait_for_callback_or_manual(&io, Some(&server), "m", "p").await,
            Ok(CallbackOrManual::Manual("pasted".into()))
        );
        assert_eq!(
            wait_for_callback_or_manual::<String>(&io, None, "m", "p").await,
            Ok(CallbackOrManual::Manual("pasted".into()))
        );
    }
}
