// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
//
// Adapted from kubernetes/kubectl v0.35.1 pkg/describe/describe.go and
// pkg/util/event/sorted_event_list.go; duration formatting from
// kubernetes/apimachinery v0.35.1 pkg/util/duration/duration.go (Copyright 2018).
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License. See NOTICE and README.md.

use k8s_openapi::api::core::v1::Event;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::Client;
use kube::api::{Api, ApiResource, DynamicObject, ListParams};
use std::fmt::Write;

pub(crate) async fn fetch_object(
    client: Client,
    selected: &ApiResource,
    namespace: &str,
    name: &str,
) -> Result<DynamicObject, kube::Error> {
    let api = |resource: &ApiResource| {
        if namespace.is_empty() {
            Api::<DynamicObject>::all_with(client.clone(), resource)
        } else {
            Api::namespaced_with(client.clone(), namespace, resource)
        }
    };
    let primary_error = match api(selected).get(name).await {
        Ok(object) => return Ok(object),
        Err(error) => error,
    };
    if !api_version_unavailable(&primary_error) {
        return Err(primary_error);
    }
    let version = match (
        selected.group.as_str(),
        selected.version.as_str(),
        selected.kind.as_str(),
    ) {
        ("autoscaling", "v2", "HorizontalPodAutoscaler") => "v1",
        ("networking.k8s.io", "v1", "ServiceCIDR" | "IPAddress") => "v1beta1",
        _ => return Err(primary_error),
    };
    let mut fallback = selected.clone();
    fallback.version = version.into();
    fallback.api_version = format!("{}/{version}", selected.group);
    api(&fallback).get(name).await.or(Err(primary_error))
}

fn api_version_unavailable(error: &kube::Error) -> bool {
    // An object or namespace can also return 404. Only the generic endpoint
    // response permits a version fallback.
    matches!(error, kube::Error::Api(status)
        if status.code == 404 && status.reason == "NotFound"
            && (status.message == "the server could not find the requested resource"
                || status.message.starts_with("the server could not find the requested resource (")))
}

pub(crate) async fn list_objects(
    client: Client,
    ar: &ApiResource,
    namespace: &str,
    mut params: ListParams,
) -> Result<Vec<DynamicObject>, kube::Error> {
    let api = if namespace.is_empty() {
        Api::<DynamicObject>::all_with(client, ar)
    } else {
        Api::namespaced_with(client, namespace, ar)
    };
    let mut objects = Vec::new();
    loop {
        let page = api.list(&params).await?;
        objects.extend(page.items);
        match page.metadata.continue_.filter(|s| !s.is_empty()) {
            Some(token) => params = params.continue_token(&token),
            None => return Ok(objects),
        }
    }
}

pub(crate) async fn fetch_events(
    client: Client,
    meta: &ObjectMeta,
    kind: &str,
) -> Result<Vec<Event>, String> {
    fetch_events_with_uid(client, meta, kind, meta.uid.as_deref()).await
}

/// Fetch events while overriding the UID component of the object reference.
/// This avoids cloning all metadata when only event identity differs, as it
/// does for static Pods and the Node name-based compatibility query.
pub(crate) async fn fetch_events_with_uid(
    client: Client,
    meta: &ObjectMeta,
    kind: &str,
    uid: Option<&str>,
) -> Result<Vec<Event>, String> {
    let ns = meta.namespace.as_deref().unwrap_or_default();
    let name = meta.name.as_deref().unwrap_or_default();
    let mut selector = format!("involvedObject.name={name},involvedObject.namespace={ns}");
    if !kind.is_empty() {
        write!(selector, ",involvedObject.kind={kind}").unwrap();
    }
    if let Some(uid) = uid.filter(|s| !s.is_empty()) {
        write!(selector, ",involvedObject.uid={uid}").unwrap();
    }
    let api: Api<Event> = if ns.is_empty() {
        Api::all(client)
    } else {
        Api::namespaced(client, ns)
    };
    let mut params = ListParams::default().fields(&selector);
    let mut events = Vec::new();
    loop {
        let page = api
            .list(&params)
            .await
            .map_err(|e| format!("describe events failed: {e}"))?;
        events.extend(page.items);
        match page.metadata.continue_.filter(|s| !s.is_empty()) {
            Some(token) => params = params.continue_token(&token),
            None => return Ok(events),
        }
    }
}
