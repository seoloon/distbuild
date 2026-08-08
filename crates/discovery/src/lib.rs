use std::collections::HashMap;
use std::net::IpAddr;

use thiserror::Error;

pub use mdns_sd::{Receiver, ResolvedService, ServiceDaemon, ServiceEvent};

/// mDNS service type DistBuild workers advertise under.
pub const SERVICE_TYPE: &str = "_distbuild._tcp.local.";

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("mDNS daemon error: {0}")]
    Mdns(#[from] mdns_sd::Error),
}

/// Information a Worker broadcasts about itself over mDNS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerAnnounce {
    pub name: String,
    pub os: String,
    pub arch: String,
    pub port: u16,
    pub version: String,
}

/// A worker discovered by browsing, decoded from mDNS TXT records.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredWorker {
    pub name: String,
    pub os: String,
    pub arch: String,
    pub port: u16,
    pub version: String,
    pub addresses: Vec<IpAddr>,
}

/// Registers a Worker's mDNS service and starts responding to browsers on
/// [`SERVICE_TYPE`]. The returned [`ServiceDaemon`] must be kept alive for
/// as long as the service should stay advertised; dropping it (or calling
/// `.shutdown()`) stops the broadcast.
pub fn broadcast(announce: &WorkerAnnounce) -> Result<ServiceDaemon, DiscoveryError> {
    let daemon = ServiceDaemon::new()?;
    let host_name = format!("{}.local.", announce.name);

    let mut properties = HashMap::new();
    properties.insert("name".to_string(), announce.name.clone());
    properties.insert("os".to_string(), announce.os.clone());
    properties.insert("arch".to_string(), announce.arch.clone());
    properties.insert("port".to_string(), announce.port.to_string());
    properties.insert("version".to_string(), announce.version.clone());

    let service_info = mdns_sd::ServiceInfo::new(
        SERVICE_TYPE,
        &announce.name,
        &host_name,
        "",
        announce.port,
        properties,
    )?
    .enable_addr_auto();

    daemon.register(service_info)?;
    Ok(daemon)
}

/// Starts browsing for DistBuild workers on the LAN. Each event on the
/// returned receiver is a discovery update; decode `ServiceResolved` events
/// with [`parse_worker`].
pub fn browse(daemon: &ServiceDaemon) -> Result<Receiver<ServiceEvent>, DiscoveryError> {
    Ok(daemon.browse(SERVICE_TYPE)?)
}

/// Decodes a resolved mDNS service into a [`DiscoveredWorker`], if it
/// carries all the TXT properties DistBuild workers publish.
pub fn parse_worker(resolved: &ResolvedService) -> Option<DiscoveredWorker> {
    let props = &resolved.txt_properties;
    Some(DiscoveredWorker {
        name: props.get_property_val_str("name")?.to_string(),
        os: props.get_property_val_str("os")?.to_string(),
        arch: props.get_property_val_str("arch")?.to_string(),
        version: props.get_property_val_str("version")?.to_string(),
        port: resolved.port,
        addresses: resolved.addresses.iter().map(|a| a.to_ip_addr()).collect(),
    })
}
