use anyhow::{Context, Result};
use futures::TryStreamExt;
use k8s_openapi::api::{
    apps::v1::Deployment, autoscaling::v1::Scale, discovery::v1::EndpointSlice,
};
use kube::{
    api::{Api, Patch, PatchParams},
    core::params::ListParams,
    runtime::{self, WatchStreamExt},
    Client,
};
use serde_json::json;
use std::sync::Arc;
use tokio::{
    join,
    sync::Notify,
    time::{self, Duration},
};
use tracing::*;

/// Entrypoint for the scaler subroutine. Does its own error handling.
/// Arguments:
/// `unavailable`: Scale up on notifications of this channel.
/// `availabe`: Notify this channel whenever the backend svc has endpoints.
/// `*_name`: Name of the target kubernetes resources.
/// `wait_secs`: Wait this amount of time before scaling down again. Will be replaced
///              by a more sophisticated criterion based on metrics.
/// `retry_secs`: In case of errors, retry after this time.
pub async fn run_scaler(
    unavailable: Arc<Notify>,
    available: Arc<Notify>,
    deploy_name: String,
    svc_name: String,
    wait_secs: u64,
    retry_secs: u64,
) {
    // say hello
    info!("Targetting deployment/{deploy_name} and service/{svc_name}.");

    // make sure we can reach the kube api
    let client = Arc::new(
        Client::try_default()
            .await
            .map_err(|e| {
                error!("Error setting up a kube api client: {e}");
                std::process::exit(255);
            })
            .unwrap(),
    );

    let watch_svc = async {
        while let Err(e) = watch_target_svc(&deploy_name, available.clone(), client.clone()).await {
            error!(
                "Failed watching endpoints for service/{svc_name} (retry in {retry_secs}s): {e}"
            );
            time::sleep(Duration::from_secs(retry_secs)).await;
        }
    };

    let scale_up = async {
        loop {
            // only act when backend is marked as unavailable
            unavailable.notified().await;

            while let Err(e) = scale_up(&deploy_name, client.clone()).await {
                error!("Failed scaling up deployment/{deploy_name} (retry in {retry_secs}s): {e}");
                time::sleep(Duration::from_secs(retry_secs)).await;
            }
        }
    };

    let scale_down = async {
        loop {
            // only act when backend becomes available
            available.notified().await;

            info!("Waiting {wait_secs}s until scaling down.");
            time::sleep(Duration::from_secs(wait_secs)).await;

            while let Err(e) = scale_down(&deploy_name, client.clone()).await {
                error!(
                    "Failed scaling down deployment/{deploy_name} (retry in {retry_secs}s): {e}"
                );
                time::sleep(Duration::from_secs(retry_secs)).await;
            }
        }
    };

    // run all eventloops
    join!(watch_svc, scale_up, scale_down);
}

/// Watch the kube api for EndpointSlices associated to service with name `svc_name`.
/// Notifies all waiters of `barker` whenever there occurs an event stating that there
/// at least one `serving` endpoint.
async fn watch_target_svc(svc_name: &str, barker: Arc<Notify>, client: Arc<Client>) -> Result<()> {
    let ep_slice: Api<EndpointSlice> = Api::default_namespaced((*client).clone());
    let label_selector =
        ListParams::default().labels(&format!("kubernetes.io/service-name={svc_name}"));
    let ep_slice_events = runtime::watcher(ep_slice, label_selector).applied_objects();
    ep_slice_events
        .try_for_each(|event| async {
            debug!(
                "Got a new event for EndpointSlices (kubernetes.io/service-name={svc_name}): {}",
                serde_json::to_string(&event).unwrap_or("Error serialising event.".to_owned())
            );
            if event
                .endpoints
                .into_iter()
                .any(|ep| ep.conditions.and_then(|epc| epc.serving) == Some(true))
            {
                barker.notify_waiters();
                info!("Backend is up again.");
            }

            Ok(())
        })
        .await?;

    Ok(())
}

/// Scale up if there are currently no replicas.
async fn scale_up(deploy_name: &str, client: Arc<Client>) -> Result<()> {
    if get_deploy_replicas(deploy_name, client.clone()).await? < 1 {
        scale_deploy(deploy_name, 1, client).await?;
    } else {
        warn!("Got a message to scale up even though there are replicas.");
    }
    Ok(())
}

/// Scale down to zero, warning if there are currently more than one replicas.
async fn scale_down(deploy_name: &str, client: Arc<Client>) -> Result<()> {
    if get_deploy_replicas(deploy_name, client.clone()).await? > 1 {
        warn!("Got a message to scale down even though there are multiple replicas.");
    }
    scale_deploy(deploy_name, 0, client).await
}

/// Get number of replicas of a deployment.
async fn get_deploy_replicas(name: &str, client: Arc<Client>) -> Result<i32> {
    let deploy: Api<Deployment> = Api::default_namespaced((*client).clone());
    let replicas = deploy
        .get_scale(name)
        .await?
        .spec
        .context("Deployment has no ScaleSpec.")?
        .replicas
        .unwrap_or_default();
    Ok(replicas)
}

/// Scale a deployment.
async fn scale_deploy(name: &str, replicas: i32, client: Arc<Client>) -> Result<()> {
    let deploy: Api<Deployment> = Api::default_namespaced((*client).clone());
    let scale: Scale = serde_json::from_value(json!({
        "apiVersion": "autoscaling/v1",
        "kind": "Scale",
        "spec": {"replicas": replicas }
    }))?;

    // do a server-side apply where "sero" is the fieldManager
    let patch = Patch::Apply(&scale);
    let params = PatchParams::apply("sero").force();
    info!("Scaling deployment/{name} to {replicas} replicas.");
    deploy.patch_scale(name, &params, &patch).await?;
    info!("Successfully scaled deployment/{name} to {replicas} replicas.");

    Ok(())
}
