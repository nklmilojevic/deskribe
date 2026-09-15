// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl describers; see NOTICE and LICENSE-APACHE.

use crate::api::{fetch_events, fetch_object};
use crate::description::Snapshot;
use crate::networking::{ingress, service};
use crate::resource::{EventPolicy, ResourceKind};
use crate::{Description, RenderOptions, controllers, node, quotas, storage};
use kube::{
    Client,
    api::{ApiResource, DynamicObject},
};

/// Read a fresh object and check its UID before related reads.
/// Drop this future to cancel outstanding requests.
pub async fn gather(
    client: Client,
    ar: &ApiResource,
    selected: &DynamicObject,
) -> Result<Description, String> {
    let kind = ResourceKind::classify(ar).ok_or("native describe unsupported")?;
    let ns = selected.metadata.namespace.as_deref().unwrap_or_default();
    let name = selected
        .metadata
        .name
        .as_deref()
        .ok_or("resource name is missing")?;
    let get = fetch_object(client.clone(), ar, ns, name);
    let prefetch = kind.prefetch_events()
        && selected
            .metadata
            .uid
            .as_deref()
            .is_some_and(|uid| !uid.is_empty());
    let (fetched, prefetched_events) = if prefetch {
        match tokio::try_join!(get, async {
            Ok::<_, kube::Error>(fetch_events(client.clone(), &selected.metadata, &ar.kind).await)
        }) {
            Ok((object, events)) => (Ok(object), Some(events)),
            Err(error) => (Err(error), None),
        }
    } else {
        (get.await, None)
    };
    let fresh = match fetched {
        Ok(fresh) => fresh,
        Err(error) => {
            if kind == ResourceKind::Pod {
                let mut reference = selected.metadata.clone();
                reference.uid = None;
                if let Ok(events) = fetch_events(client, &reference, "").await
                    && !events.is_empty()
                {
                    let message = match &error {
                        kube::Error::Api(response) => response.message.clone(),
                        _ => error.to_string(),
                    };
                    return Ok(Description {
                        object: selected.clone(),
                        events: Some(events),
                        snapshot: Snapshot::PodGetFailure(format!(
                            "Pod '{name}': error '{message}', but found events.\n"
                        )),
                    });
                }
            }
            return Err(format!("describe GET failed: {error}"));
        }
    };
    if selected.metadata.uid.is_some() && selected.metadata.uid != fresh.metadata.uid {
        return Err(
            "resource was replaced; return to the table and select the new resource".into(),
        );
    }
    let resource_events = async {
        match kind.events() {
            EventPolicy::Skip => Ok(None),
            EventPolicy::Pod => {
                let mut reference = fresh.metadata.clone();
                if let Some(uid) = reference
                    .annotations
                    .as_ref()
                    .and_then(|a| a.get("kubernetes.io/config.mirror"))
                {
                    reference.uid = Some(uid.clone());
                }
                Ok(fetch_events(client.clone(), &reference, "").await.ok())
            }
            EventPolicy::Optional | EventPolicy::Required => {
                let result = match prefetched_events {
                    Some(events) => events,
                    None => fetch_events(client.clone(), &fresh.metadata, &ar.kind).await,
                };
                if kind.events() == EventPolicy::Required {
                    result.map(Some)
                } else {
                    Ok(result.ok())
                }
            }
        }
    };
    let related = async {
        let snapshot = match kind {
            ResourceKind::Pod => Snapshot::Pod,
            ResourceKind::Service => Snapshot::Service(
                service::related(client.clone(), &fresh)
                    .await
                    .unwrap_or_default(),
            ),
            ResourceKind::Node => {
                Snapshot::Node(Box::new(node::related(client.clone(), &fresh).await?))
            }
            ResourceKind::ConfigMap => Snapshot::ConfigMap,
            ResourceKind::Secret => Snapshot::Secret,
            ResourceKind::ReplicationController => Snapshot::ReplicationController(
                controllers::related(client.clone(), &fresh, "ReplicationController").await,
            ),
            ResourceKind::ServiceAccount => Snapshot::ServiceAccount,
            ResourceKind::LimitRange => Snapshot::LimitRange,
            ResourceKind::ResourceQuota => Snapshot::ResourceQuota,
            ResourceKind::PersistentVolume => Snapshot::PersistentVolume,
            ResourceKind::PersistentVolumeClaim => {
                Snapshot::PersistentVolumeClaim(storage::related(client.clone(), &fresh).await?)
            }
            ResourceKind::Namespace => {
                let (quotas, limits) = quotas::related(client.clone(), &fresh).await?;
                Snapshot::Namespace { quotas, limits }
            }
            ResourceKind::Endpoints => Snapshot::Endpoints,
            ResourceKind::EndpointSlice => Snapshot::EndpointSlice,
            ResourceKind::HorizontalPodAutoscaler => Snapshot::HorizontalPodAutoscaler,
            ResourceKind::Ingress => {
                Snapshot::Ingress(ingress::related(client.clone(), &fresh).await)
            }
            ResourceKind::IngressClass => Snapshot::IngressClass,
            ResourceKind::ServiceCIDR => Snapshot::ServiceCIDR,
            ResourceKind::IPAddress => Snapshot::IPAddress,
            ResourceKind::NetworkPolicy => Snapshot::NetworkPolicy,
            ResourceKind::Job => Snapshot::Job,
            ResourceKind::CronJob => Snapshot::CronJob,
            ResourceKind::StatefulSet => Snapshot::StatefulSet(
                controllers::related(client.clone(), &fresh, "StatefulSet").await,
            ),
            ResourceKind::Deployment => Snapshot::Deployment(
                controllers::related(client.clone(), &fresh, "Deployment").await,
            ),
            ResourceKind::DaemonSet => {
                Snapshot::DaemonSet(controllers::related(client.clone(), &fresh, "DaemonSet").await)
            }
            ResourceKind::ReplicaSet => Snapshot::ReplicaSet(
                controllers::related(client.clone(), &fresh, "ReplicaSet").await,
            ),
            ResourceKind::CertificateSigningRequest => Snapshot::CertificateSigningRequest,
            ResourceKind::StorageClass => Snapshot::StorageClass,
            ResourceKind::CSINode => Snapshot::CSINode,
            ResourceKind::VolumeAttributesClass => Snapshot::VolumeAttributesClass,
            ResourceKind::PodDisruptionBudget => Snapshot::PodDisruptionBudget,
            ResourceKind::Role => Snapshot::Role,
            ResourceKind::ClusterRole => Snapshot::ClusterRole,
            ResourceKind::RoleBinding => Snapshot::RoleBinding,
            ResourceKind::ClusterRoleBinding => Snapshot::ClusterRoleBinding,
            ResourceKind::PriorityClass => Snapshot::PriorityClass,
            ResourceKind::Generic => Snapshot::Generic,
        };
        Ok::<_, String>(snapshot)
    };
    let (snapshot, events) = tokio::join!(related, resource_events);
    Ok(Description {
        object: fresh,
        snapshot: snapshot?,
        events: events?,
    })
}

/// Gather fresh data and render it with the current time and local timezone.
/// Use [`gather`] and [`Description::render`] to control rendering separately.
pub async fn fetch(
    client: Client,
    ar: &ApiResource,
    selected: &DynamicObject,
) -> Result<(DynamicObject, String), String> {
    let description = gather(client, ar, selected).await?;
    let output = description.render(&RenderOptions::default())?;
    Ok((description.into_object(), output))
}
