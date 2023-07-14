use anyhow::Result;
use futures::{Stream, StreamExt};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::{
    api::Api,
    core::params::ListParams,
    runtime::{self, reflector::Store, WatchStreamExt},
    Client,
};
use std::{pin::Pin, sync::Arc};
use tokio::sync::watch;
use tracing::*;

#[derive(PartialEq, Default, Debug)]
struct EndpointCount {
    sero: usize,
    backend: usize,
}

impl From<(usize, usize)> for EndpointCount {
    fn from((sero, backend): (usize, usize)) -> Self {
        EndpointCount { sero, backend }
    }
}

struct EndpointWatcher {
    name: String,
    port_name: String,
    sender: watch::Sender<EndpointCount>,
    store: Store<EndpointSlice>,
    events: Pin<Box<dyn Stream<Item = Result<EndpointSlice, runtime::watcher::Error>> + Send>>,
}

impl EndpointWatcher {
    fn new(
        svc_name: &str,
        svc_port_name: &str,
        sender: watch::Sender<EndpointCount>,
        client: Arc<Client>,
    ) -> Self {
        let api: Api<EndpointSlice> = Api::default_namespaced((*client).clone());
        let selector =
            ListParams::default().labels(&format!("kubernetes.io/service-name={svc_name}"));
        let (store, writer) = runtime::reflector::store();
        let events = runtime::reflector(writer, runtime::watcher(api, selector))
            .touched_objects()
            .boxed();

        info!(
            "Watching EndpointSlices associated to service/{}.",
            svc_name
        );

        EndpointWatcher {
            name: svc_name.to_owned(),
            port_name: svc_port_name.to_owned(),
            sender,
            store,
            events,
        }
    }

    async fn run(mut self) {
        while let Some(event) = self.events.next().await {
            match event {
                Err(e) => error!("Error getting next event for EndpointSlices: {e}"),
                Ok(ep_slice) => {
                    debug!(
                        "Got a new event for EndpointSlices: {}",
                        serde_json::to_string(&ep_slice)
                            .unwrap_or("Error serialising event.".to_owned())
                    );
                    self.send_state_update();
                }
            }
        }
    }

    fn send_state_update(&self) {
        let current_ep = self.serving_endpoints();
        self.sender.send_if_modified(|ep| {
            if *ep != current_ep {
                info!("Sending state update: {current_ep:?}");
                *ep = current_ep;
                return true;
            }
            false
        });
    }

    fn serving_endpoints(&self) -> EndpointCount {
        self.store
            .state()
            .iter()
            .filter(|ep_slice| {
                ep_slice.ports.as_ref().map(|ports| {
                    ports
                        .iter()
                        .any(|port| port.name == Some(self.port_name.clone()))
                }) == Some(true)
            })
            .fold((0_usize, 0_usize), |(sero, backend), ep_slice| {
                let serving_ep = ep_slice
                    .endpoints
                    .iter()
                    .filter(|&ep| {
                        ep.conditions
                            .as_ref()
                            .and_then(|ep_conditions| ep_conditions.serving)
                            == Some(true)
                    })
                    .count();
                if ep_slice
                    .metadata
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get("sero.rs/service-name"))
                    == Some(&self.name)
                {
                    (sero + serving_ep, backend)
                } else {
                    (sero, backend + serving_ep)
                }
            })
            .into()
    }
}

#[derive(Clone)]
pub struct EndpointWatcherHandle {
    receiver: watch::Receiver<EndpointCount>,
}

impl EndpointWatcherHandle {
    pub fn new(svc_name: &str, svc_port_name: &str, client: Arc<Client>) -> Self {
        let (sender, receiver) = watch::channel(EndpointCount::default());
        let watcher = EndpointWatcher::new(svc_name, svc_port_name, sender, client);
        tokio::spawn(watcher.run());
        EndpointWatcherHandle { receiver }
    }

    pub fn backend_is_serving(&self) -> bool {
        self.receiver.borrow().backend > 0
    }

    pub fn sero_is_serving(&self) -> bool {
        self.receiver.borrow().sero > 0
    }

    pub async fn changed(&mut self) {
        if let Err(e) = self.receiver.changed().await {
            warn!("Error while waiting for EndpointSlice updates: {e}");
        }
    }
}
