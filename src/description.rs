// Copyright 2026 sofka contributors.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use k8s_openapi::jiff::tz::TimeZone;

/// Rendering settings. Supply explicit values for deterministic output.
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
    pub(crate) resource: ApiResource,
    pub(crate) events: Option<Vec<Event>>,
    pub(crate) related: Related,
}

pub(crate) enum Related {
    None,
    Node(Box<node::Related>),
    Ingress(ingress::Backends),
    Storage(Vec<DynamicObject>),
    Quotas {
        quotas: Option<Vec<DynamicObject>>,
        limits: Option<Vec<DynamicObject>>,
    },
    Service(Vec<DynamicObject>),
    Controller(Result<Vec<DynamicObject>, String>),
    PodGetFailure(String),
}

impl Description {
    /// The fetched, identity-checked object, except for the documented Pod-error
    /// case where surviving events are returned with the original selection.
    pub fn object(&self) -> &DynamicObject {
        &self.object
    }

    /// Return the object after rendering without cloning it.
    pub fn into_object(self) -> DynamicObject {
        self.object
    }

    /// Render this snapshot without fetching anything or consulting kubeconfig.
    pub fn render(&self, options: &RenderOptions) -> Result<String, String> {
        let object = &self.object;
        let ar = &self.resource;
        let events = self.events.as_deref();
        let now = options.now;
        let zone = &options.timezone;
        let output = match &self.related {
            Related::Node(related) => node::render(object, related, now, zone),
            Related::Ingress(backends) => ingress::render(object, backends, events, now),
            Related::Storage(pods) => storage::render(object, &ar.kind, pods, events, now, zone),
            Related::Quotas { quotas, limits } => {
                quotas::render(object, &ar.kind, quotas.as_deref(), limits.as_deref(), zone)
            }
            Related::Service(slices) => service::render(object, slices, events, now),
            Related::Controller(related) => controllers::render(
                object,
                &ar.kind,
                related.as_deref().map_err(String::as_str),
                events,
                now,
                zone,
            )?,
            Related::PodGetFailure(message) => with_events(message.clone(), events, now),
            Related::None => {
                if autoscaling::supports(ar) {
                    autoscaling::render(object, events, now, zone)
                } else if ar.group == "certificates.k8s.io"
                    && ar.kind == "CertificateSigningRequest"
                {
                    certificate::render(object, events, now, zone)?
                } else if ar.group.is_empty() && ar.kind == "Pod" {
                    pod::render(object, events, now, zone)
                } else if workloads::supports(ar) {
                    workloads::render(object, &ar.kind, events, now, zone)
                } else if policy::supports(ar) {
                    policy::render(object, &ar.kind, events, now, zone)
                } else if endpoints::supports(ar) {
                    endpoints::render(object, &ar.kind, events, now)
                } else if rbac::supports(ar) {
                    rbac::render(object)
                } else if classes::supports(ar) {
                    classes::render(object, &ar.kind, events, now, zone)
                } else if !has_upstream_specialized(ar) {
                    generic::render(object, events, now)
                } else if ar.kind == "Secret" {
                    let secret: Secret = serde_json::from_value(
                        serde_json::to_value(object).map_err(|_| "invalid describe object")?,
                    )
                    .map_err(|_| "invalid Secret response")?;
                    render_secret(&secret)
                } else {
                    let cm: ConfigMap = serde_json::from_value(
                        serde_json::to_value(object).map_err(|_| "invalid describe object")?,
                    )
                    .map_err(|_| "invalid ConfigMap response")?;
                    render_config_map(&cm, events.unwrap_or_default(), now)
                }
            }
        };
        Ok(output)
    }
}
