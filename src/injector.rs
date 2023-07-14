use anyhow::{Context, Result};
use json_patch::PatchOperation::{Add, Remove};
use json_patch::{AddOperation, RemoveOperation};
use k8s_openapi::{
    api::{
        core::v1::{ObjectReference, Pod},
        discovery::v1::{Endpoint, EndpointPort, EndpointSlice},
    },
    apimachinery::pkg::apis::meta::v1::OwnerReference,
    Resource,
};
use kube::{
    api::{Api, ListParams, Patch, PatchParams, PostParams},
    core::ObjectMeta,
    Client,
};
use serde_json::Value;
use std::{collections::BTreeMap, net::IpAddr, str::FromStr, sync::Arc};
use tokio::sync::mpsc;
use tracing::*;

struct Injector {
    name: String,
    port: u16,
    svc_name: String,
    svc_port_name: String,
    receiver: mpsc::Receiver<InjectorMessage>,
    client: Arc<Client>,
}

impl Injector {
    fn try_new(
        receiver: mpsc::Receiver<InjectorMessage>,
        svc_name: &str,
        svc_port_name: &str,
        port: u16,
        client: Arc<Client>,
    ) -> Result<Self> {
        // read own hostname
        let hostname = hostname::get()?;
        let name = hostname.to_str().context("Hostname is not valid UTF8")?;
        Ok(Injector {
            name: name.to_owned(),
            port,
            svc_name: svc_name.to_owned(),
            svc_port_name: svc_port_name.to_owned(),
            receiver,
            client,
        })
    }

    async fn init_endpointslice(&self) -> Result<()> {
        // query kube api for info about self
        let pod: Api<Pod> = Api::default_namespaced((*self.client).clone());
        let pod = pod.get(&self.name).await?;
        // construct managed endpointslice
        let api_version = Pod::API_VERSION.to_owned();
        let kind = Pod::KIND.to_owned();
        let uid = pod.metadata.uid.context("self.metadata.uid is empty.")?;
        let namespace = pod
            .metadata
            .namespace
            .context("self.metadata.namespace is empty")?;
        let ip_addresses: Vec<IpAddr> = pod
            .status
            .context("self.status is empty")?
            .pod_ips
            .context("self.status.podIPs is empty")?
            .iter()
            .filter_map(|pod_ip| pod_ip.ip.as_ref())
            .filter_map(|ip| IpAddr::from_str(ip).ok())
            .collect();
        let ipv4_addresses: Vec<String> = ip_addresses
            .iter()
            .filter(|ip| ip.is_ipv4())
            .map(|ip| ip.to_string())
            .collect();
        let owner_ref = OwnerReference {
            api_version: api_version.clone(),
            kind: kind.clone(),
            name: self.name.clone(),
            uid: uid.clone(),
            ..Default::default()
        };
        let target_ref = ObjectReference {
            api_version: Some(api_version),
            kind: Some(kind),
            name: Some(self.name.clone()),
            namespace: Some(namespace),
            uid: Some(uid),
            ..Default::default()
        };
        let labels: BTreeMap<String, String> = BTreeMap::from([
            (
                "kubernetes.io/service-name".to_owned(),
                self.svc_name.clone(),
            ),
            ("sero.rs/service-name".to_owned(), self.svc_name.clone()),
        ]);
        // create managed endpointslice (currently only IPv4)
        let ep_slice_ipv4 = EndpointSlice {
            address_type: "IPv4".to_owned(),
            metadata: ObjectMeta {
                generate_name: Some(format!("{}-sero-", self.svc_name)),
                owner_references: Some(vec![owner_ref]),
                labels: Some(labels),
                ..Default::default()
            },
            endpoints: vec![Endpoint {
                addresses: ipv4_addresses,
                target_ref: Some(target_ref),
                ..Default::default()
            }],
            ports: Some(vec![EndpointPort {
                name: Some(self.svc_port_name.clone()),
                port: Some(self.port as i32),
                ..Default::default()
            }]),
        };
        let api: Api<EndpointSlice> = Api::default_namespaced((*self.client).clone());
        let params = PostParams {
            field_manager: Some("injector.sero.rs".to_owned()),
            dry_run: false,
        };
        api.create(&params, &ep_slice_ipv4).await?;
        Ok(())
    }

    async fn inject(&self) -> Result<()> {
        let api: Api<EndpointSlice> = Api::default_namespaced((*self.client).clone());
        let selector =
            ListParams::default().labels(&format!("sero.rs/service-name={}", self.svc_name));
        let params = PatchParams::apply("injector.sero.rs").force();
        let patch = json_patch::Patch(vec![Add(AddOperation {
            path: "/metadata/labels/kubernetes.io/service-name".to_owned(),
            value: Value::String(self.svc_name.clone()),
        })]);
        let patch = Patch::Json::<()>(patch);

        info!(
            "Ejecting sero from endpointslices for service/{}.",
            self.svc_name
        );
        let ep_slices = api.list(&selector).await?;
        for ep_slice in ep_slices {
            if let Some(name) = ep_slice.metadata.name {
                api.patch_metadata(&name, &params, &patch).await?;
            }
        }

        Ok(())
    }

    async fn eject(&self) -> Result<()> {
        let api: Api<EndpointSlice> = Api::default_namespaced((*self.client).clone());
        let selector =
            ListParams::default().labels(&format!("sero.rs/service-name={}", self.svc_name));
        let params = PatchParams::apply("injector.sero.rs").force();
        let patch = json_patch::Patch(vec![Remove(RemoveOperation {
            path: "/metadata/labels/kubernetes.io/service-name".to_owned(),
        })]);
        let patch = Patch::Json::<()>(patch);

        info!(
            "Ejecting sero from endpointslices for service/{}.",
            self.svc_name
        );
        let ep_slices = api.list(&selector).await?;
        for ep_slice in ep_slices {
            if let Some(name) = ep_slice.metadata.name {
                api.patch_metadata(&name, &params, &patch).await?;
            }
        }

        Ok(())
    }

    async fn handle_message(&self, msg: InjectorMessage) -> Result<()> {
        use InjectorMessage::*;
        match msg {
            Inject => self.inject().await?,
            Eject => self.eject().await?,
        };
        Ok(())
    }

    async fn run(mut self) {
        if let Err(e) = self.init_endpointslice().await {
            error!("Error while initialising endpointslice/{}: {e}", self.name);
            return;
        }

        while let Some(msg) = self.receiver.recv().await {
            if let Err(e) = self.handle_message(msg).await {
                error!("Error while handling InjectorMessage: {e}");
            };
        }
    }
}

#[allow(dead_code)]
#[derive(Debug)]
enum InjectorMessage {
    Inject,
    Eject,
}

#[derive(Clone)]
pub struct InjectorHandle {
    sender: mpsc::Sender<InjectorMessage>,
}

#[allow(dead_code)]
impl InjectorHandle {
    pub fn try_new(
        max_concurrency: usize,
        svc_name: &str,
        svc_port_name: &str,
        port: u16,
        client: Arc<Client>,
    ) -> Result<Self> {
        let (sender, receiver) = mpsc::channel(max_concurrency);
        let injector = Injector::try_new(receiver, svc_name, svc_port_name, port, client)?;
        tokio::spawn(injector.run());
        Ok(InjectorHandle { sender })
    }

    pub fn inject(&self) -> Result<()> {
        self.sender.try_send(InjectorMessage::Inject)?;
        Ok(())
    }

    pub fn eject(&self) -> Result<()> {
        self.sender.try_send(InjectorMessage::Eject)?;
        Ok(())
    }
}
