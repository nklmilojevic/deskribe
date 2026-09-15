// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 sofka contributors.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl describers; see NOTICE and LICENSE-APACHE.

use super::*;
use crate::description::Related;

/// Fresh GET and identity-checked related reads; dropping this future cancels
/// outstanding requests. Never reads kubeconfig, spawns kubectl, or uses a cache.
pub async fn gather(
    client: Client,
    ar: &ApiResource,
    selected: &DynamicObject,
) -> Result<Description, String> {
    if !supports(ar) {
        return Err("native describe unsupported".into());
    }
    let ns = selected.metadata.namespace.as_deref().unwrap_or_default();
    let name = selected
        .metadata
        .name
        .as_deref()
        .ok_or("resource name is missing")?;
    let get = fetch_object(client.clone(), ar, ns, name);
    let prefetch = selected
        .metadata
        .uid
        .as_deref()
        .is_some_and(|uid| !uid.is_empty())
        && !(ar.group.is_empty() && matches!(ar.kind.as_str(), "Pod" | "Node" | "Secret"))
        && !quotas::supports(ar)
        && !rbac::supports(ar)
        && !controllers::supports(ar)
        && !ingress::supports(ar)
        && !(ar.group.is_empty()
            && matches!(ar.kind.as_str(), "Service" | "PersistentVolumeClaim"))
        && (!classes::supports(ar) || classes::events(ar))
        && (!policy::supports(ar) || policy::events(ar));
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
            if ar.group.is_empty() && ar.kind == "Pod" {
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
                        resource: ar.clone(),
                        events: Some(events),
                        related: Related::PodGetFailure(format!(
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
    let event_client = client.clone();
    let event_metadata = fresh.metadata.clone();
    let resource_events = async move {
        match prefetched_events {
            Some(events) => events,
            None => fetch_events(event_client, &event_metadata, &ar.kind).await,
        }
    };
    let mut events = None;
    let related = if ar.group.is_empty() && ar.kind == "Node" {
        Related::Node(Box::new(node::related(client, &fresh).await?))
    } else if ingress::supports(ar) {
        let (backends, result) =
            tokio::join!(ingress::related(client.clone(), &fresh), resource_events);
        events = result.ok();
        Related::Ingress(backends)
    } else if storage::supports(ar) {
        let (pods, result) = tokio::join!(
            async {
                if ar.kind == "PersistentVolumeClaim" {
                    storage::related(client.clone(), &fresh).await
                } else {
                    Ok(Vec::new())
                }
            },
            resource_events
        );
        events = result.ok();
        Related::Storage(pods?)
    } else if quotas::supports(ar) {
        let (quotas, limits) = if ar.kind == "Namespace" {
            quotas::related(client, &fresh).await?
        } else {
            (None, None)
        };
        Related::Quotas { quotas, limits }
    } else if ar.group.is_empty() && ar.kind == "Pod" {
        let mut reference = fresh.metadata.clone();
        if let Some(uid) = reference
            .annotations
            .as_ref()
            .and_then(|a| a.get("kubernetes.io/config.mirror"))
        {
            reference.uid = Some(uid.clone());
        }
        events = fetch_events(client, &reference, "").await.ok();
        Related::None
    } else if ar.group.is_empty() && ar.kind == "Service" {
        let (slices, result) =
            tokio::join!(service::related(client.clone(), &fresh), resource_events);
        events = result.ok();
        Related::Service(slices.unwrap_or_default())
    } else if controllers::supports(ar) {
        let (related, result) = tokio::join!(
            controllers::related(client.clone(), &fresh, &ar.kind),
            resource_events
        );
        events = result.ok();
        Related::Controller(related)
    } else {
        if !(ar.group.is_empty() && ar.kind == "Secret")
            && !rbac::supports(ar)
            && (!classes::supports(ar) || classes::events(ar))
            && (!policy::supports(ar) || policy::events(ar))
        {
            let result = resource_events.await;
            events = if ar.group.is_empty() && ar.kind == "ConfigMap" {
                Some(result?)
            } else {
                result.ok()
            };
        }
        Related::None
    };
    Ok(Description {
        object: fresh,
        resource: ar.clone(),
        events,
        related,
    })
}

/// Gather fresh data and render it using the current time and local timezone.
/// Use [`gather`] and [`Description::render`] separately to control rendering.
pub async fn fetch(
    client: Client,
    ar: &ApiResource,
    selected: &DynamicObject,
) -> Result<(DynamicObject, String), String> {
    let description = gather(client, ar, selected).await?;
    let output = description.render(&RenderOptions::default())?;
    Ok((description.into_object(), output))
}
