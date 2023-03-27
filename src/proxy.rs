use crate::scaler::ScalerHandle;

use anyhow::Result;
use tokio::net::{TcpListener, TcpStream};
use tracing::*;

pub struct Proxy {
    backend_address: String,
    listener: TcpListener,
    scaler: ScalerHandle,
}

impl Proxy {
    pub async fn try_new(
        listen_address: &str,
        backend_address: &str,
        scaler: ScalerHandle,
    ) -> Result<Self> {
        let listener = TcpListener::bind(listen_address).await?;
        info!("Listening for TCP connections on {listen_address}, proxying connections to {backend_address}.");
        Ok(Proxy {
            backend_address: backend_address.to_owned(),
            listener,
            scaler,
        })
    }

    pub async fn run(self) {
        while let Ok((ingress, _)) = self.listener.accept().await {
            let scaler = self.scaler.clone();
            let backend_address = self.backend_address.to_owned();
            tokio::spawn(async move {
                // only connect if backend is up
                if let Err(e) = scaler.ensure_up().await {
                    error!("Failed to ensure a serving backend, dropping connection: {e}");
                } else {
                    proxy_tcp_stream(ingress, backend_address).await;
                }
            });
        }
    }
}

/// Proxy a TCP Stream to a backend address
async fn proxy_tcp_stream(mut ingress: TcpStream, backend_address: String) {
    let mut egress = match TcpStream::connect(backend_address).await {
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
