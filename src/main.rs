mod cli;
mod proxy;
mod scaler;

use anyhow::Result;
use clap::Parser;
use cli::Cli;
use std::sync::Arc;
use tokio::{
    net::{TcpListener, TcpStream},
    signal,
    sync::Notify,
    time::{self, Duration},
};
use tracing::*;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // get cli params
    let Cli {
        listen_address,
        backend_address,
        target_deploy,
        target_svc,
    } = Cli::parse();

    info!("Listening on {}.", &listen_address);
    info!("Proxying requests to {}.", &backend_address);
    info!("Target deployment is {}.", &target_deploy);
    info!("Target service is {}.", &target_svc);

    // initialise channels for inter-task communication
    let backend_unavailable = Arc::new(Notify::new());
    let backend_available = Arc::new(Notify::new());

    // handle signals to scale deploy
    let scaler_timeout = 10u64;
    tokio::spawn(run_scaler(
        backend_unavailable.clone(),
        backend_available.clone(),
        target_deploy,
        target_svc,
        scaler_timeout,
    ));

    // handle incoming connections
    let listener = TcpListener::bind(&listen_address).await?;
    tokio::spawn(run_proxy(
        listener,
        backend_unavailable,
        backend_available,
        backend_address,
    ));

    // wait for signal to gracefully exit
    graceful_shutdown().await;

    Ok(())
}

async fn run_scaler(
    unavailable: Arc<Notify>,
    available: Arc<Notify>,
    deploy: String,
    svc: String,
    retry_secs: u64,
) {
    loop {
        unavailable.notified().await;
        info!("Got a request to a backend that is unreachable.");

        while let Err(e) = scaler::scale_up(&deploy, &svc).await {
            error!("Failed scale up routine (retry in {retry_secs}s): {e}");
            time::sleep(Duration::from_secs(retry_secs)).await;
        }

        available.notify_waiters();
        info!("Backend is up again.");
    }
}

async fn run_proxy(
    listener: TcpListener,
    unavailable: Arc<Notify>,
    available: Arc<Notify>,
    backend_address: String,
) {
    while let Ok((ingress, _)) = listener.accept().await {
        let unavailable = unavailable.clone();
        let available = available.clone();
        let backend_address = backend_address.clone();
        // span a new task to handle the connection
        tokio::spawn(async move {
            loop {
                match TcpStream::connect(&backend_address).await {
                    Ok(backend) => {
                        trace!("Successfully connected to backend. Proxying packets.");
                        proxy::proxy_tcp_connection(ingress, backend).await;
                        break;
                    }
                    Err(_) => {
                        unavailable.notify_one();
                        available.notified().await;
                    }
                };
            }
        });
    }
}

async fn graceful_shutdown() {
    let ctrl_c = async {
        match signal::ctrl_c().await {
            Ok(_) => info!("Received C-c, shutting down."),
            Err(err) => error!("Unable to listen for shutdown signal: {}", err),
        };
    };

    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(err) => error!("Unable to listen for shutdown signal: {}", err),
        }
    };

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
