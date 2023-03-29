use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Service;
use kube::{api::Api, Client};
use std::sync::Arc;
use tracing::*;

#[derive(Debug)]
pub struct ServicePortInfo {
    pub name: String,
    pub number: u16,
}

impl ServicePortInfo {
    pub async fn try_new(name: &str, port_name: Option<&str>, client: Arc<Client>) -> Result<Self> {
        let svc: Api<Service> = Api::default_namespaced((*client).clone());
        let ports = svc
            .get(name)
            .await?
            .spec
            .and_then(|spec| spec.ports)
            .context("Service does not have any ports.")?;
        let port = match port_name {
            None => ports.first(),
            Some(port_name) => ports
                .iter()
                .find(|port| port.name == Some(port_name.to_owned())),
        }
        .context("Could not find secified port in service.")?
        .clone();

        let res = ServicePortInfo {
            name: port.name.unwrap_or_default(),
            number: port.port as u16,
        };
        debug!("Successfully got info about backend service/{name}: {res:?}");

        Ok(res)
    }
}
