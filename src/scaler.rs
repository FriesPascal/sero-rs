use crate::endpoint_watcher::EndpointWatcherHandle;

use anyhow::{Context, Result};
use k8s_openapi::api::{apps::v1::Deployment, autoscaling::v1::Scale};
use kube::{
    api::{Api, Patch, PatchParams, ScaleSpec},
    Client,
};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::*;

struct Scaler {
    receiver: mpsc::Receiver<ScalerMessage>,
    deploy_name: String,
    client: Arc<Client>,
    endpoints: EndpointWatcherHandle,
}

impl Scaler {
    fn new(
        receiver: mpsc::Receiver<ScalerMessage>,
        deploy_name: &str,
        client: Arc<Client>,
        endpoints: EndpointWatcherHandle,
    ) -> Self {
        Scaler {
            receiver,
            deploy_name: deploy_name.to_owned(),
            client,
            endpoints,
        }
    }

    async fn get_replicas(&self) -> Result<i32> {
        let deploy: Api<Deployment> = Api::default_namespaced((*self.client).clone());
        let replicas = deploy
            .get_scale(&self.deploy_name)
            .await?
            .spec
            .context("Deployment has no ScaleSpec.")?
            .replicas
            .unwrap_or_default();
        Ok(replicas)
    }

    async fn set_replicas(&self, replicas: i32) -> Result<()> {
        let deploy: Api<Deployment> = Api::default_namespaced((*self.client).clone());
        let scale = Scale {
            spec: Some(ScaleSpec {
                replicas: Some(replicas),
            }),
            ..Default::default()
        };
        // do a forced server side apply where "sero" is the fieldManager
        // TODO: maybe first try without `force`, then warn and retry with `force`?
        let patch = Patch::Apply(&scale);
        let params = PatchParams::apply("scaler.sero.rs").force();
        info!(
            "Scaling deployment/{} to {replicas} replicas.",
            self.deploy_name
        );
        deploy
            .patch_scale(&self.deploy_name, &params, &patch)
            .await?;

        Ok(())
    }

    async fn scale_up(&self) -> Result<()> {
        let current_replicas = self.get_replicas().await?;
        if current_replicas < 1 {
            self.set_replicas(1).await?;
        }
        Ok(())
    }

    async fn scale_down(&self) -> Result<()> {
        let current_replicas = self.get_replicas().await?;
        if current_replicas >= 0 {
            self.set_replicas(0).await?;
        }
        Ok(())
    }

    async fn handle_message(&mut self, msg: ScalerMessage) -> Result<()> {
        use ScalerMessage::*;
        match msg {
            ScaleUp => self.scale_up().await,
            ScaleDown => self.scale_up().await,
            EnsureUp(sender) => {
                // first make sure that the backend is serving
                while !self.endpoints.backend_is_serving() {
                    self.scale_up().await?;
                    self.endpoints.changed().await;
                }
                while self.endpoints.sero_is_serving() {
                    // TODO: drain sero endpointslices
                    self.endpoints.changed().await;
                }
                sender
                    .send(())
                    .ok()
                    .context("Could not answer to EnsureUp message because sender end was dropped.")
            }
            EnsureDown(sender) => {
                // TODO: first, make sure that sero is serving (add self to endpointslice)
                // then, scale down backend
                while self.endpoints.backend_is_serving() {
                    self.scale_down().await?;
                    self.endpoints.changed().await;
                }
                sender.send(()).ok().context(
                    "Could not answer to EnsureDown message because sender end was dropped.",
                )
            }
        }
    }

    async fn run(mut self) {
        while let Some(msg) = self.receiver.recv().await {
            if let Err(e) = self.handle_message(msg).await {
                error!("Error while handling ScalerMessage: {e}");
            };
        }
    }
}

#[allow(dead_code)]
#[derive(Debug)]
enum ScalerMessage {
    ScaleUp,
    ScaleDown,
    EnsureUp(oneshot::Sender<()>),
    EnsureDown(oneshot::Sender<()>),
}

#[derive(Clone)]
pub struct ScalerHandle {
    sender: mpsc::Sender<ScalerMessage>,
}

#[allow(dead_code)]
impl ScalerHandle {
    pub fn new(
        max_concurrency: usize,
        deploy_name: &str,
        client: Arc<Client>,
        endpoints: EndpointWatcherHandle,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(max_concurrency);
        let scaler = Scaler::new(receiver, deploy_name, client, endpoints);
        tokio::spawn(scaler.run());

        ScalerHandle { sender }
    }

    pub fn scale_up(&self) -> Result<()> {
        self.sender.try_send(ScalerMessage::ScaleUp)?;
        Ok(())
    }

    pub fn scale_down(&self) -> Result<()> {
        self.sender.try_send(ScalerMessage::ScaleDown)?;
        Ok(())
    }

    pub async fn ensure_up(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender.try_send(ScalerMessage::EnsureUp(tx))?;
        rx.await?;
        Ok(())
    }
}
