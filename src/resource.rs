// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0

use kube::api::ApiResource;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResourceKind {
    Pod,
    Service,
    Node,
    ConfigMap,
    Secret,
    ReplicationController,
    ServiceAccount,
    LimitRange,
    ResourceQuota,
    PersistentVolume,
    PersistentVolumeClaim,
    Namespace,
    Endpoints,
    EndpointSlice,
    HorizontalPodAutoscaler,
    Ingress,
    IngressClass,
    ServiceCIDR,
    IPAddress,
    NetworkPolicy,
    Job,
    CronJob,
    StatefulSet,
    Deployment,
    DaemonSet,
    ReplicaSet,
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventPolicy {
    Skip,
    Optional,
    Required,
    Pod,
}

impl ResourceKind {
    pub(crate) fn classify(resource: &ApiResource) -> Option<Self> {
        let kind = match (resource.group.as_str(), resource.kind.as_str()) {
            ("", "Pod") => Self::Pod,
            ("", "Service") => Self::Service,
            ("", "Node") => Self::Node,
            ("", "ConfigMap") => {
                if resource.version != "v1" || resource.plural != "configmaps" {
                    return None;
                }
                Self::ConfigMap
            }
            ("", "Secret") => {
                if resource.version != "v1" || resource.plural != "secrets" {
                    return None;
                }
                Self::Secret
            }
            ("", "ReplicationController") => Self::ReplicationController,
            ("", "ServiceAccount") => Self::ServiceAccount,
            ("", "LimitRange") => Self::LimitRange,
            ("", "ResourceQuota") => Self::ResourceQuota,
            ("", "PersistentVolume") => Self::PersistentVolume,
            ("", "PersistentVolumeClaim") => Self::PersistentVolumeClaim,
            ("", "Namespace") => Self::Namespace,
            ("", "Endpoints") => Self::Endpoints,
            ("discovery.k8s.io", "EndpointSlice") => Self::EndpointSlice,
            ("autoscaling", "HorizontalPodAutoscaler") => Self::HorizontalPodAutoscaler,
            ("extensions" | "networking.k8s.io", "Ingress") => Self::Ingress,
            ("networking.k8s.io", "IngressClass") => Self::IngressClass,
            ("networking.k8s.io", "ServiceCIDR") => Self::ServiceCIDR,
            ("networking.k8s.io", "IPAddress") => Self::IPAddress,
            ("networking.k8s.io", "NetworkPolicy") => Self::NetworkPolicy,
            ("batch", "Job") => Self::Job,
            ("batch", "CronJob") => Self::CronJob,
            ("apps", "StatefulSet") => Self::StatefulSet,
            ("apps", "Deployment") => Self::Deployment,
            ("apps", "DaemonSet") => Self::DaemonSet,
            ("apps", "ReplicaSet") => Self::ReplicaSet,
            ("certificates.k8s.io", "CertificateSigningRequest") => Self::CertificateSigningRequest,
            ("storage.k8s.io", "StorageClass") => Self::StorageClass,
            ("storage.k8s.io", "CSINode") => Self::CSINode,
            ("storage.k8s.io", "VolumeAttributesClass") => Self::VolumeAttributesClass,
            ("policy", "PodDisruptionBudget") => Self::PodDisruptionBudget,
            ("rbac.authorization.k8s.io", "Role") => Self::Role,
            ("rbac.authorization.k8s.io", "ClusterRole") => Self::ClusterRole,
            ("rbac.authorization.k8s.io", "RoleBinding") => Self::RoleBinding,
            ("rbac.authorization.k8s.io", "ClusterRoleBinding") => Self::ClusterRoleBinding,
            ("" | "scheduling.k8s.io", "PriorityClass") => Self::PriorityClass,
            _ => Self::Generic,
        };
        Some(kind)
    }

    pub(crate) fn events(self) -> EventPolicy {
        match self {
            Self::Pod => EventPolicy::Pod,
            Self::ConfigMap => EventPolicy::Required,
            Self::Node
            | Self::Secret
            | Self::Namespace
            | Self::ResourceQuota
            | Self::LimitRange
            | Self::Role
            | Self::ClusterRole
            | Self::RoleBinding
            | Self::ClusterRoleBinding
            | Self::NetworkPolicy => EventPolicy::Skip,
            Self::Service
            | Self::ReplicationController
            | Self::ServiceAccount
            | Self::PersistentVolume
            | Self::PersistentVolumeClaim
            | Self::Endpoints
            | Self::EndpointSlice
            | Self::HorizontalPodAutoscaler
            | Self::Ingress
            | Self::IngressClass
            | Self::ServiceCIDR
            | Self::IPAddress
            | Self::Job
            | Self::CronJob
            | Self::StatefulSet
            | Self::Deployment
            | Self::DaemonSet
            | Self::ReplicaSet
            | Self::CertificateSigningRequest
            | Self::StorageClass
            | Self::CSINode
            | Self::VolumeAttributesClass
            | Self::PodDisruptionBudget
            | Self::PriorityClass
            | Self::Generic => EventPolicy::Optional,
        }
    }

    pub(crate) fn prefetch_events(self) -> bool {
        matches!(self.events(), EventPolicy::Optional | EventPolicy::Required)
            && !matches!(
                self,
                Self::Ingress
                    | Self::Service
                    | Self::PersistentVolumeClaim
                    | Self::ReplicationController
                    | Self::ReplicaSet
                    | Self::DaemonSet
                    | Self::StatefulSet
                    | Self::Deployment
            )
    }
}
