// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl describers; see NOTICE and LICENSE-APACHE.

use crate::api::{fetch_events, fetch_object};
use crate::description::Snapshot;
use crate::networking::{ingress, service};
use crate::resource::{EventPolicy, ResourceKind};
use crate::{Description, RenderOptions, controllers, node, quotas, storage};
use k8s_openapi::api::core::v1::Event;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::{
    Client,
    api::{ApiResource, DynamicObject},
};
use serde_json::Value;
use std::borrow::Cow;

/// A static pod's events are recorded against the uid of its mirror, not the
/// uid of the API object the kubelet publishes.
const MIRROR: &str = "kubernetes.io/config.mirror";

/// Read a fresh object and check its UID before related reads.
/// Drop this future to cancel outstanding requests.
///
/// Related reads and the event query are issued against the selection at the
/// same time as the fresh GET rather than after it. A describe used to cost two
/// serial round trips for most kinds; it now costs one whenever the fresh
/// object agrees with the selection on the fields those reads were built from,
/// and falls back to reading them again when it does not.
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

    // Without an identity there is nothing to validate a speculative read
    // against, so those kinds keep reading strictly after the GET.
    let identified = selected
        .metadata
        .uid
        .as_deref()
        .is_some_and(|uid| !uid.is_empty());

    let get = fetch_object(client.clone(), ar, ns, name);
    let speculative = async {
        if !identified {
            return None;
        }
        Some(tokio::join!(
            related(client.clone(), kind, selected),
            resource_events(client.clone(), kind, &ar.kind, selected),
        ))
    };
    let (fetched, speculated) = tokio::join!(get, speculative);

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

    let (snapshot, events) = match speculated {
        Some(results) if reads_agree(selected, &fresh) => results,
        _ => tokio::join!(
            related(client.clone(), kind, &fresh),
            resource_events(client, kind, &ar.kind, &fresh),
        ),
    };
    Ok(Description {
        object: fresh,
        snapshot: snapshot?,
        events: events?,
    })
}

/// Whether reads built from the selection would have been built the same way
/// from the fresh object.
///
/// Related reads derive from the name, namespace and `spec` — never `status`,
/// which is the field that actually churns — and the Pod event query
/// additionally follows the mirror annotation. Name and namespace are what the
/// GET asked for, and identity is checked separately, so `spec` and that one
/// annotation are what is left to compare.
fn reads_agree(selected: &DynamicObject, fresh: &DynamicObject) -> bool {
    fn inputs(object: &DynamicObject) -> (&Value, Option<&String>) {
        (
            &object.data["spec"],
            object
                .metadata
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.get(MIRROR)),
        )
    }
    inputs(selected) == inputs(fresh)
}

/// The object an event query is scoped by.
fn event_reference(kind: ResourceKind, meta: &ObjectMeta) -> Cow<'_, ObjectMeta> {
    if kind != ResourceKind::Pod {
        return Cow::Borrowed(meta);
    }
    match meta.annotations.as_ref().and_then(|a| a.get(MIRROR)) {
        Some(uid) => {
            let mut reference = meta.clone();
            reference.uid = Some(uid.clone());
            Cow::Owned(reference)
        }
        None => Cow::Borrowed(meta),
    }
}

async fn resource_events(
    client: Client,
    kind: ResourceKind,
    ar_kind: &str,
    object: &DynamicObject,
) -> Result<Option<Vec<Event>>, String> {
    match kind.events() {
        EventPolicy::Skip => Ok(None),
        // Upstream scopes pod events by uid alone, without the kind filter.
        EventPolicy::Pod => {
            let reference = event_reference(kind, &object.metadata);
            Ok(fetch_events(client, &reference, "").await.ok())
        }
        EventPolicy::Optional => Ok(fetch_events(client, &object.metadata, ar_kind).await.ok()),
        EventPolicy::Required => fetch_events(client, &object.metadata, ar_kind)
            .await
            .map(Some),
    }
}

async fn related(
    client: Client,
    kind: ResourceKind,
    object: &DynamicObject,
) -> Result<Snapshot, String> {
    let snapshot = match kind {
        ResourceKind::Pod => Snapshot::Pod,
        ResourceKind::Service => Snapshot::Service(
            service::related(client.clone(), object)
                .await
                .unwrap_or_default(),
        ),
        ResourceKind::Node => {
            Snapshot::Node(Box::new(node::related(client.clone(), object).await?))
        }
        ResourceKind::ConfigMap => Snapshot::ConfigMap,
        ResourceKind::Secret => Snapshot::Secret,
        ResourceKind::ReplicationController => Snapshot::ReplicationController(
            controllers::related(client.clone(), object, "ReplicationController").await,
        ),
        ResourceKind::ServiceAccount => Snapshot::ServiceAccount,
        ResourceKind::LimitRange => Snapshot::LimitRange,
        ResourceKind::ResourceQuota => Snapshot::ResourceQuota,
        ResourceKind::PersistentVolume => Snapshot::PersistentVolume,
        ResourceKind::PersistentVolumeClaim => {
            Snapshot::PersistentVolumeClaim(storage::related(client.clone(), object).await?)
        }
        ResourceKind::Namespace => {
            let (quotas, limits) = quotas::related(client.clone(), object).await?;
            Snapshot::Namespace { quotas, limits }
        }
        ResourceKind::Endpoints => Snapshot::Endpoints,
        ResourceKind::EndpointSlice => Snapshot::EndpointSlice,
        ResourceKind::HorizontalPodAutoscaler => Snapshot::HorizontalPodAutoscaler,
        ResourceKind::Ingress => Snapshot::Ingress(ingress::related(client.clone(), object).await),
        ResourceKind::IngressClass => Snapshot::IngressClass,
        ResourceKind::ServiceCIDR => Snapshot::ServiceCIDR,
        ResourceKind::IPAddress => Snapshot::IPAddress,
        ResourceKind::NetworkPolicy => Snapshot::NetworkPolicy,
        ResourceKind::Job => Snapshot::Job,
        ResourceKind::CronJob => Snapshot::CronJob,
        ResourceKind::StatefulSet => {
            Snapshot::StatefulSet(controllers::related(client.clone(), object, "StatefulSet").await)
        }
        ResourceKind::Deployment => {
            Snapshot::Deployment(controllers::related(client.clone(), object, "Deployment").await)
        }
        ResourceKind::DaemonSet => {
            Snapshot::DaemonSet(controllers::related(client.clone(), object, "DaemonSet").await)
        }
        ResourceKind::ReplicaSet => {
            Snapshot::ReplicaSet(controllers::related(client.clone(), object, "ReplicaSet").await)
        }
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
    Ok(snapshot)
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
