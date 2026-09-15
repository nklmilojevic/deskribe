// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0

use crate::config::{render_config_map, render_secret};
use crate::events::with_events;
use crate::networking::{endpoints, ingress, service};
use crate::{
    autoscaling, batch, certificate, controllers, generic, networking, node, pod, policy, quotas,
    rbac, scheduling, service_account, storage,
};
use k8s_openapi::api::core::v1::{ConfigMap, Event, Secret};
use k8s_openapi::jiff::Timestamp;
use k8s_openapi::jiff::tz::TimeZone;
use kube::api::DynamicObject;

/// Render settings. Set explicit values for repeatable output.
#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub now: Timestamp,
    pub timezone: TimeZone,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            now: Timestamp::now(),
            timezone: TimeZone::system(),
        }
    }
}

/// A fetched resource and its related data. Rendering performs no network I/O.
///
/// Descriptions can contain sensitive data, including service-account tokens.
pub struct Description {
    pub(crate) object: DynamicObject,
    pub(crate) events: Option<Vec<Event>>,
    pub(crate) snapshot: Snapshot,
}

pub(crate) enum Snapshot {
    Pod,
    Service(Vec<DynamicObject>),
    Node(Box<node::Related>),
    ConfigMap,
    Secret,
    ReplicationController(Result<Vec<DynamicObject>, String>),
    ServiceAccount,
    LimitRange,
    ResourceQuota,
    PersistentVolume,
    PersistentVolumeClaim(Vec<DynamicObject>),
    Namespace {
        quotas: Option<Vec<DynamicObject>>,
        limits: Option<Vec<DynamicObject>>,
    },
    Endpoints,
    EndpointSlice,
    HorizontalPodAutoscaler,
    Ingress(ingress::Backends),
    IngressClass,
    ServiceCIDR,
    IPAddress,
    NetworkPolicy,
    Job,
    CronJob,
    StatefulSet(Result<Vec<DynamicObject>, String>),
    Deployment(Result<Vec<DynamicObject>, String>),
    DaemonSet(Result<Vec<DynamicObject>, String>),
    ReplicaSet(Result<Vec<DynamicObject>, String>),
    CertificateSigningRequest,
    StorageClass,
    CSINode,
    VolumeAttributesClass,
    PodDisruptionBudget,
    Role,
    ClusterRole,
    RoleBinding,
    ClusterRoleBinding,
    PriorityClass,
    Generic,
    PodGetFailure(String),
}

impl Description {
    /// Return the fresh object. If the Pod read failed and events were found,
    /// return the original selection.
    pub fn object(&self) -> &DynamicObject {
        &self.object
    }

    /// Return the object without a clone.
    pub fn into_object(self) -> DynamicObject {
        self.object
    }

    /// Render this snapshot without network reads.
    pub fn render(&self, options: &RenderOptions) -> Result<String, String> {
        let object = &self.object;
        let events = self.events.as_deref();
        let now = options.now;
        let zone = &options.timezone;
        let output = match &self.snapshot {
            Snapshot::Node(related) => node::render(object, related, now, zone),
            Snapshot::Ingress(backends) => ingress::render(object, backends, events, now),
            Snapshot::Service(slices) => service::render(object, slices, events, now),
            Snapshot::PersistentVolumeClaim(pods) => {
                storage::render(object, "PersistentVolumeClaim", pods, events, now, zone)
            }
            Snapshot::PersistentVolume => {
                storage::render(object, "PersistentVolume", &[], events, now, zone)
            }
            Snapshot::Namespace { quotas, limits } => quotas::render(
                object,
                "Namespace",
                quotas.as_deref(),
                limits.as_deref(),
                zone,
            ),
            Snapshot::HorizontalPodAutoscaler => autoscaling::render(object, events, now, zone),
            Snapshot::CertificateSigningRequest => certificate::render(object, events, now, zone)?,
            Snapshot::Pod => pod::render(object, events, now, zone),
            Snapshot::Generic => generic::render(object, events, now),
            Snapshot::PodGetFailure(message) => with_events(message.clone(), events, now),
            Snapshot::ServiceAccount => service_account::render(object, events, now),
            Snapshot::PriorityClass => scheduling::render(object, events, now),
            Snapshot::StorageClass => storage::render_class(object, events, now),
            Snapshot::CSINode => storage::render_csi_node(object, events, now, zone),
            Snapshot::VolumeAttributesClass => {
                storage::render_volume_attributes_class(object, events, now)
            }
            Snapshot::IngressClass => networking::render_ingress_class(object, events, now),
            Snapshot::IPAddress => networking::render_ip_address(object, events, now),
            Snapshot::ServiceCIDR => networking::render_service_cidr(object, events, now, zone),
            Snapshot::ReplicationController(related) => controllers::render(
                object,
                "ReplicationController",
                related.as_deref().map_err(String::as_str),
                events,
                now,
                zone,
            )?,
            Snapshot::ReplicaSet(related) => controllers::render(
                object,
                "ReplicaSet",
                related.as_deref().map_err(String::as_str),
                events,
                now,
                zone,
            )?,
            Snapshot::DaemonSet(related) => controllers::render(
                object,
                "DaemonSet",
                related.as_deref().map_err(String::as_str),
                events,
                now,
                zone,
            )?,
            Snapshot::StatefulSet(related) => controllers::render(
                object,
                "StatefulSet",
                related.as_deref().map_err(String::as_str),
                events,
                now,
                zone,
            )?,
            Snapshot::Deployment(related) => controllers::render(
                object,
                "Deployment",
                related.as_deref().map_err(String::as_str),
                events,
                now,
                zone,
            )?,
            Snapshot::ResourceQuota => quotas::render(object, "ResourceQuota", None, None, zone),
            Snapshot::LimitRange => quotas::render(object, "LimitRange", None, None, zone),
            Snapshot::Job => batch::render(object, "Job", events, now, zone),
            Snapshot::CronJob => batch::render(object, "CronJob", events, now, zone),
            Snapshot::PodDisruptionBudget => {
                policy::render(object, "PodDisruptionBudget", events, now, zone)
            }
            Snapshot::NetworkPolicy => policy::render(object, "NetworkPolicy", events, now, zone),
            Snapshot::Endpoints => endpoints::render(object, "Endpoints", events, now),
            Snapshot::EndpointSlice => endpoints::render(object, "EndpointSlice", events, now),
            Snapshot::Role | Snapshot::ClusterRole => rbac::render(object),
            Snapshot::RoleBinding | Snapshot::ClusterRoleBinding => rbac::render_binding(object),
            Snapshot::Secret => {
                let value: Secret = serde_json::from_value(
                    serde_json::to_value(object).map_err(|_| "invalid describe object")?,
                )
                .map_err(|_| "invalid Secret response")?;
                render_secret(&value)
            }
            Snapshot::ConfigMap => {
                let value: ConfigMap = serde_json::from_value(
                    serde_json::to_value(object).map_err(|_| "invalid describe object")?,
                )
                .map_err(|_| "invalid ConfigMap response")?;
                render_config_map(&value, events.unwrap_or_default(), now)
            }
        };
        Ok(output)
    }
}
