use crate::scaler::ScalerHandle;

use anyhow::Result;
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tracing::*;

pub struct Proxy {
    backend_host: String,
    backend_port: u16,
    listener: TcpListener,
    scaler: ScalerHandle,
}

impl Proxy {
    pub async fn try_new(
        listen_host: &str,
        listen_port: u16,
        backend_host: &str,
        backend_port: u16,
        scaler: ScalerHandle,
    ) -> Result<Self> {
        let listener = TcpListener::bind((listen_host, listen_port)).await?;
        info!("Listening for TCP connections on {listen_host}:{listen_port}, proxying connections to {backend_host}:{backend_port}.");
        Ok(Proxy {
            backend_host: backend_host.to_owned(),
            backend_port,
            listener,
            scaler,
        })
    }

    pub async fn run(self) {
        while let Ok((ingress, _)) = self.listener.accept().await {
            let scaler = self.scaler.clone();
            let backend = (self.backend_host.to_owned(), self.backend_port);
            tokio::spawn(async move {
                // only connect if backend is up
                if let Err(e) = scaler.ensure_up().await {
                    error!("Failed to ensure a serving backend, dropping connection: {e}");
                } else {
                    proxy_tcp_stream(ingress, backend).await;
                }
            });
        }
    }
}

/// Proxy a TCP Stream to a backend address
async fn proxy_tcp_stream<A: ToSocketAddrs>(mut ingress: TcpStream, backend: A) {
    let mut egress = match TcpStream::connect(backend).await {
        Ok(egress) => {
            trace!("Successfully connected to backend. Proxying connections.");
            egress
        }
        Err(e) => {
            error!("Error while connecting to backend: {e}");
            return;
        }
    };

    match tokio::io::copy_bidirectional(&mut ingress, &mut egress).await {
        Ok((bytes_to_backend, bytes_from_backend)) => {
            trace!("Connection ended gracefully ({bytes_to_backend} bytes from client, {bytes_from_backend} bytes from server)");
        }
        Err(e) => {
            error!("Error while proxying: {e}");
        }
    }
}
