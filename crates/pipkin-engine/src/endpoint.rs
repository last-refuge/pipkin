//! Explicit local attachment to a separately owned engine. No discovery and no TCP listener.
//! A private directory and kernel peer credentials authenticate the local OS user on both
//! ends. All clients use the existing versioned frame contract and share one store writer.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{Server, connect};

/// Foreground host; SIGINT/SIGTERM belong to this process's owner, never an attached client.
pub fn run(config: crate::config::EngineConfig, directory: &std::path::Path) -> i32 {
    #[cfg(unix)]
    {
        use std::sync::Arc;
        let engine = match crate::host::EngineHandle::start(config) {
            Ok(engine) => Arc::new(engine),
            Err(error) => {
                eprintln!("pipkin engine: {error}");
                return 1;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        engine.shared().spawn(async move {
            let result = async {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
                let mut interrupt =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
                let _ = ready_tx.send(());
                tokio::select! { _ = term.recv() => {}, _ = interrupt.recv() => {} }
                Ok::<_, std::io::Error>(())
            }
            .await;
            let _ = tx.send(result);
        });
        // Install handlers before an endpoint can report readiness to a client.
        if ready_rx.recv().is_err() {
            eprintln!("pipkin engine: signal listener failed");
            return 1;
        }
        let server = match Server::start(engine.clone(), directory) {
            Ok(server) => server,
            Err(error) => {
                eprintln!("pipkin engine endpoint: {error}");
                return 1;
            }
        };
        let result = rx.recv();
        drop(server);
        engine.shutdown(std::time::Duration::from_secs(10));
        match result {
            Ok(Ok(())) => 0,
            _ => {
                eprintln!("pipkin engine: signal listener failed");
                1
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (config, directory);
        eprintln!("pipkin engine: local socket attachment is supported on Unix hosts only");
        2
    }
}
