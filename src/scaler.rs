use anyhow::{Context, Result};
use futures::{pin_mut, TryStreamExt};
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
use tracing::*;

/// Scale up.
pub async fn scale_up(target_deploy: &str, target_svc: &str) -> Result<()> {
    // Only scale up if there are currently no replicas.
    if get_replicas(target_deploy).await? < 1 {
        scale_deploy(target_deploy, 1).await?;
    } else {
        warn!("Got a message to scale up even though there are replicas.");
    }

    // in any case, wait for endpoints
    debug!("Waiting for service/{target_svc} to have serving endpoints.");
    wait_for_target_endpoints(target_svc).await
}

/// Get number of replicas of a deployment.
async fn get_replicas(name: &str) -> Result<i32> {
    let client = Client::try_default().await?;
    let deploy: Api<Deployment> = Api::default_namespaced(client);
    deploy
        .get_scale(name)
        .await?
        .spec
        .context("Deployment has no ScaleSpec.")?
        .replicas
        .context("Deployment has no replicas defined.")
}

/// Scale a deployment. Returns Ok(()) if everything went well.
async fn scale_deploy(name: &str, replicas: i32) -> Result<()> {
    let client = Client::try_default().await?;
    let deploy: Api<Deployment> = Api::default_namespaced(client);
    let scale: Scale = serde_json::from_value(json!({
        "apiVersion": "autoscaling/v1",
        "kind": "Scale",
        "spec": {"replicas": replicas }
    }))?;

    // do a server-side apply where "sero" is the fieldManager
    let patch = Patch::Apply(&scale);
    let params = PatchParams::apply("sero");
    info!("Scaling deployment/{name} to {replicas} replicas.");
    deploy.patch_scale(name, &params, &patch).await?;
    info!("Successfully scaled deployment/{name} to {replicas} replicas.");

    Ok(())
}

/// Poll the kube api for EndpointSlices associated to service with name `name`.
/// Returns Ok(()) as soon as there is an epSlice containing at least one `serving` endpoint.
async fn wait_for_target_endpoints(name: &str) -> Result<()> {
    let client = Client::try_default().await?;
    let ep_slice: Api<EndpointSlice> = Api::default_namespaced(client);
    let label_selector =
        ListParams::default().labels(&format!("kubernetes.io/service-name={name}"));
    let ep_slice_events = runtime::watcher(ep_slice, label_selector).applied_objects();

    pin_mut!(ep_slice_events);
    while let Some(event) = ep_slice_events.try_next().await? {
        debug!(
            "Got a new event for EndpointSlices (kubernetes.io/service-name={name}): {}",
            serde_json::to_string(&event).unwrap_or("Error serialising event.".to_owned())
        );
        // take all endpoints in this EndpointSlice and check if at least one of them is serving
        if event
            .endpoints
            .into_iter()
            .any(|ep| ep.conditions.and_then(|epc| epc.serving) == Some(true))
        {
            return Ok(());
        }
    }

    Ok(())
}
