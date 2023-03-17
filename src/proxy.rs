use std::sync::Arc;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Notify,
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
            loop {
                match TcpStream::connect(&backend_address).await {
                    Ok(backend) => {
                        trace!("Successfully connected to backend. Proxying packets.");
                        proxy_tcp_connection(ingress, backend).await;
                        break;
                    }
                    Err(_) => {
                        unavailable.notify_one();
                        debug!("Got a request to a backend that is unreachable.");
                        available.notified().await;
                    }
                };
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
