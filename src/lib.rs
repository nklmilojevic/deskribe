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

//! Native Kubernetes descriptions with caller-owned clients and separate gathering/rendering.
//! See the README for the supported behavior baseline and compatibility notes.

mod api;
mod autoscaling;
mod batch;
mod certificate;
mod config;
mod containers;
mod controllers;
mod description;
mod events;
mod fetch;
mod format;
mod generic;
mod json;
mod metadata;
mod networking;
mod node;
mod pod;
mod policy;
mod quantity;
mod quotas;
mod rbac;
mod resource;
mod scheduling;
mod service_account;
mod storage;
mod time;
mod volumes;

pub use config::{render_config_map, render_secret};
pub use description::{Description, RenderOptions};
pub use fetch::{fetch, gather};

/// Return true if this resource has a supported description format.
pub fn supports(resource: &kube::api::ApiResource) -> bool {
    resource::ResourceKind::classify(resource).is_some()
}
