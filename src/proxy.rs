use std::sync::Arc;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Notify,
    time::{self, Duration},
};
use tracing::*;

/// Start the proxy eventloop. Does its own error handling.
/// Arguments:
/// `*_address`: Self explanatory.
/// `unavailable`: Channel to notify when failing to connect to backend.
/// `available`: Once failed connecting to backend, wait on this channel to retry.
pub async fn run_proxy(
    listen_address: String,
    backend_address: String,
    unavailable: Arc<Notify>,
    available: Arc<Notify>,
) {
    // say hello
    info!("Listening on {listen_address}, proxying requests to {backend_address}.");

    // listen on `listen_address`
    let listener = TcpListener::bind(&listen_address)
        .await
        .map_err(|e| error!("Failed to create TcpListener: {e}"))
        .unwrap();
    // accept new connections
    while let Ok((ingress, _)) = listener.accept().await {
        let unavailable = unavailable.clone();
        let available = available.clone();
        let backend_address = backend_address.clone();
        // span a new task to handle the connection
        tokio::spawn(async move {
            // try connecting to backend and proxy. if connection times out or is
            // refused, mark backend as unavailable and wait until available again
            loop {
                if let Ok(Ok(backend)) = time::timeout(
                    Duration::from_secs(1u64),
                    TcpStream::connect(&backend_address),
                )
                .await
                {
                    trace!("Successfully connected to backend. Proxying connections.");
                    proxy_tcp_connection(ingress, backend).await;
                    break;
                }
                debug!("Error connecting to backend. Marking as unavailable and wait until available again.");
                unavailable.notify_one();
                available.notified().await;
            }
        });
    }
}

/// Proxy a TCP connection between two TcpStreams
async fn proxy_tcp_connection(mut ingress: TcpStream, mut backend: TcpStream) {
    match tokio::io::copy_bidirectional(&mut ingress, &mut backend).await {
        Ok((bytes_to_backend, bytes_from_backend)) => {
            trace!("Connection ended gracefully ({bytes_to_backend} bytes from client, {bytes_from_backend} bytes from server)");
        }
        Err(e) => {
            error!("Error while proxying: {e}");
        }
    }
}
