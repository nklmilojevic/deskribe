// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 sofka contributors.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use serde_json::Value;
use std::fmt::Write;

pub(super) fn render(out: &mut String, volumes: &[Value], space: &str) {
    if volumes.is_empty() {
        writeln!(out, "{space}Volumes:\t<none>").unwrap();
        return;
    }
    writeln!(out, "{space}Volumes:").unwrap();
    for volume in volumes {
        writeln!(
            out,
            "  {}{}:",
            if space.is_empty() { "" } else { " " },
            text(&volume["name"])
        )
        .unwrap();
        source(out, volume, false);
    }
}

#[derive(Clone, Copy)]
enum Format {
    Text,
    Bool,
    Int,
    List,
    Reference,
    Map,
}
use Format::*;
type Field = (&'static str, &'static str, Format);

pub(super) fn source(out: &mut String, volume: &Value, persistent: bool) {
    let (key, description, fields): (&str, &str, &[Field]) = if !volume["hostPath"].is_null() {
        (
            "hostPath",
            "HostPath (bare host directory volume)",
            &[("Path:", "path", Text)],
        )
    } else if !volume["emptyDir"].is_null() {
        (
            "emptyDir",
            "EmptyDir (a temporary directory that shares a pod's lifetime)",
            &[("Medium:", "medium", Text)],
        )
    } else if !volume["gcePersistentDisk"].is_null() {
        (
            "gcePersistentDisk",
            "GCEPersistentDisk (a Persistent Disk resource in Google Compute Engine)",
            &[
                ("PDName:", "pdName", Text),
                ("FSType:", "fsType", Text),
                ("Partition:", "partition", Int),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["awsElasticBlockStore"].is_null() {
        (
            "awsElasticBlockStore",
            "AWSElasticBlockStore (a Persistent Disk resource in AWS)",
            &[
                ("VolumeID:", "volumeID", Text),
                ("FSType:", "fsType", Text),
                ("Partition:", "partition", Int),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["gitRepo"].is_null() {
        (
            "gitRepo",
            "GitRepo (a volume that is pulled from git when the pod is created)",
            &[
                ("Repository:", "repository", Text),
                ("Revision:", "revision", Text),
            ],
        )
    } else if !volume["secret"].is_null() {
        (
            "secret",
            "Secret (a volume populated by a Secret)",
            &[
                ("SecretName:", "secretName", Text),
                ("Optional:", "optional", Bool),
            ],
        )
    } else if !volume["configMap"].is_null() {
        (
            "configMap",
            "ConfigMap (a volume populated by a ConfigMap)",
            &[("Name:", "name", Text), ("Optional:", "optional", Bool)],
        )
    } else if !volume["nfs"].is_null() {
        (
            "nfs",
            "NFS (an NFS mount that lasts the lifetime of a pod)",
            &[
                ("Server:", "server", Text),
                ("Path:", "path", Text),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["iscsi"].is_null() {
        (
            "iscsi",
            "ISCSI (an ISCSI Disk resource that is attached to a kubelet's host machine and then exposed to the pod)",
            &[
                ("TargetPortal:", "targetPortal", Text),
                ("IQN:", "iqn", Text),
                ("Lun:", "lun", Int),
                ("ISCSIInterface", "iscsiInterface", Text),
                ("FSType:", "fsType", Text),
                ("ReadOnly:", "readOnly", Bool),
                ("Portals:", "portals", List),
                ("DiscoveryCHAPAuth:", "chapAuthDiscovery", Bool),
                ("SessionCHAPAuth:", "chapAuthSession", Bool),
                ("SecretRef:", "secretRef", Reference),
            ],
        )
    } else if !volume["glusterfs"].is_null() {
        (
            "glusterfs",
            "Glusterfs (a Glusterfs mount on the host that shares a pod's lifetime)",
            &[
                ("EndpointsName:", "endpoints", Text),
                ("Path:", "path", Text),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["persistentVolumeClaim"].is_null() {
        (
            "persistentVolumeClaim",
            "PersistentVolumeClaim (a reference to a PersistentVolumeClaim in the same namespace)",
            &[
                ("ClaimName:", "claimName", Text),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["ephemeral"].is_null() {
        (
            "ephemeral",
            "EphemeralVolume (an inline specification for a volume that gets created and deleted with the pod)",
            &[],
        )
    } else if !volume["rbd"].is_null() {
        (
            "rbd",
            "RBD (a Rados Block Device mount on the host that shares a pod's lifetime)",
            &[
                ("CephMonitors:", "monitors", List),
                ("RBDImage:", "image", Text),
                ("FSType:", "fsType", Text),
                ("RBDPool:", "pool", Text),
                ("RadosUser:", "user", Text),
                ("Keyring:", "keyring", Text),
                ("SecretRef:", "secretRef", Reference),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["quobyte"].is_null() {
        (
            "quobyte",
            "Quobyte (a Quobyte mount on the host that shares a pod's lifetime)",
            &[
                ("Registry:", "registry", Text),
                ("Volume:", "volume", Text),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["downwardAPI"].is_null() {
        (
            "downwardAPI",
            "DownwardAPI (a volume populated by information about the pod)",
            &[],
        )
    } else if !volume["azureDisk"].is_null() {
        (
            "azureDisk",
            "AzureDisk (an Azure Data Disk mount on the host and bind mount to the pod)",
            &[
                ("DiskName:", "diskName", Text),
                ("DiskURI:", "diskURI", Text),
                ("Kind: ", "kind", Text),
                ("FSType:", "fsType", Text),
                ("CachingMode:", "cachingMode", Text),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["vsphereVolume"].is_null() {
        (
            "vsphereVolume",
            "vSphereVolume (a Persistent Disk resource in vSphere)",
            &[
                ("VolumePath:", "volumePath", Text),
                ("FSType:", "fsType", Text),
                ("StoragePolicyName:", "storagePolicyName", Text),
            ],
        )
    } else if !volume["cinder"].is_null() {
        (
            "cinder",
            "Cinder (a Persistent Disk resource in OpenStack)",
            &[
                ("VolumeID:", "volumeID", Text),
                ("FSType:", "fsType", Text),
                ("ReadOnly:", "readOnly", Bool),
                ("SecretRef:", "secretRef", Reference),
            ],
        )
    } else if !volume["photonPersistentDisk"].is_null() {
        (
            "photonPersistentDisk",
            "PhotonPersistentDisk (a Persistent Disk resource in photon platform)",
            &[("PdID:", "pdID", Text), ("FSType:", "fsType", Text)],
        )
    } else if !volume["portworxVolume"].is_null() {
        (
            "portworxVolume",
            "PortworxVolume (a Portworx Volume resource)",
            &[("VolumeID:", "volumeID", Text)],
        )
    } else if !volume["scaleIO"].is_null() {
        (
            "scaleIO",
            "ScaleIO (a persistent volume backed by a block device in ScaleIO)",
            &[
                ("Gateway:", "gateway", Text),
                ("System:", "system", Text),
                ("Protection Domain:", "protectionDomain", Text),
                ("Storage Pool:", "storagePool", Text),
                ("Storage Mode:", "storageMode", Text),
                ("VolumeName:", "volumeName", Text),
                ("FSType:", "fsType", Text),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["cephfs"].is_null() {
        (
            "cephfs",
            "CephFS (a CephFS mount on the host that shares a pod's lifetime)",
            &[
                ("Monitors:", "monitors", List),
                ("Path:", "path", Text),
                ("User:", "user", Text),
                ("SecretFile:", "secretFile", Text),
                ("SecretRef:", "secretRef", Reference),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["storageos"].is_null() {
        (
            "storageos",
            "StorageOS (a StorageOS Persistent Disk resource)",
            &[
                ("VolumeName:", "volumeName", Text),
                ("VolumeNamespace:", "volumeNamespace", Text),
                ("FSType:", "fsType", Text),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["fc"].is_null() {
        ("fc", "FC (a Fibre Channel disk)", &[])
    } else if !volume["azureFile"].is_null() {
        (
            "azureFile",
            "AzureFile (an Azure File Service mount on the host and bind mount to the pod)",
            &[
                ("SecretName:", "secretName", Text),
                ("ShareName:", "shareName", Text),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["flexVolume"].is_null() {
        (
            "flexVolume",
            "FlexVolume (a generic volume resource that is provisioned/attached using an exec based plugin)",
            &[
                ("Driver:", "driver", Text),
                ("FSType:", "fsType", Text),
                ("SecretRef:", "secretRef", Reference),
                ("ReadOnly:", "readOnly", Bool),
                ("Options:", "options", Map),
            ],
        )
    } else if !volume["flocker"].is_null() {
        (
            "flocker",
            "Flocker (a Flocker volume mounted by the Flocker agent)",
            &[
                ("DatasetName:", "datasetName", Text),
                ("DatasetUUID:", "datasetUUID", Text),
            ],
        )
    } else if !volume["projected"].is_null() {
        (
            "projected",
            "Projected (a volume that contains injected data from multiple sources)",
            &[],
        )
    } else if !volume["csi"].is_null() {
        (
            "csi",
            "CSI (a Container Storage Interface (CSI) volume source)",
            &[
                ("Driver:", "driver", Text),
                ("FSType:", "fsType", Text),
                ("ReadOnly:", "readOnly", Bool),
            ],
        )
    } else if !volume["image"].is_null() {
        (
            "image",
            "Image (a container image or OCI artifact)",
            &[
                ("Reference:", "reference", Text),
                ("PullPolicy:", "pullPolicy", Text),
            ],
        )
    } else if persistent && !volume["local"].is_null() {
        (
            "local",
            "LocalVolume (a persistent volume backed by local storage on a node)",
            &[("Path:", "path", Text)],
        )
    } else {
        out.push_str("  <unknown>\n");
        return;
    };
    let value = &volume[key];
    writeln!(out, "    Type:\t{description}").unwrap();
    for &(label, field, format) in fields {
        if persistent && key == "glusterfs" && field == "path" {
            writeln!(
                out,
                "    EndpointsNamespace:\t{}",
                value["endpointsNamespace"].as_str().unwrap_or("<unset>")
            )
            .unwrap();
        }
        if persistent && key == "azureFile" && field == "shareName" {
            writeln!(
                out,
                "    SecretNamespace:\t{}",
                text(&value["secretNamespace"])
            )
            .unwrap();
        }
        if persistent && key == "scaleIO" && field == "fsType" {
            writeln!(
                out,
                "    SecretName:\t{}\n    SecretNamespace:\t{}",
                text(&value["secretRef"]["name"]),
                text(&value["secretRef"]["namespace"])
            )
            .unwrap();
        }
        if persistent && key == "csi" && field == "readOnly" {
            writeln!(out, "    VolumeHandle:\t{}", text(&value["volumeHandle"])).unwrap();
        }
        writeln!(
            out,
            "    {label}\t{}",
            formatted(&value[field], format, persistent)
        )
        .unwrap();
    }
    match key {
        "hostPath" => writeln!(
            out,
            "    HostPathType:\t{}",
            value["type"].as_str().unwrap_or("<none>")
        )
        .unwrap(),
        "emptyDir" => writeln!(
            out,
            "    SizeLimit:\t{}",
            value["sizeLimit"]
                .as_str()
                .filter(|s| *s != "0")
                .unwrap_or("<unset>")
        )
        .unwrap(),
        "iscsi" => writeln!(
            out,
            "    InitiatorName:\t{}",
            value["initiatorName"].as_str().unwrap_or("<none>")
        )
        .unwrap(),
        "ephemeral" => {
            let template = &value["volumeClaimTemplate"];
            if !template.is_null() {
                let spec = &template["spec"];
                let meta = serde_json::from_value::<
                    k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
                >(template["metadata"].clone())
                .unwrap_or_default();
                let class = meta
                    .annotations
                    .as_ref()
                    .and_then(|a| a.get("volume.beta.kubernetes.io/storage-class"))
                    .map(String::as_str)
                    .unwrap_or_else(|| text(&spec["storageClassName"]));
                writeln!(
                    out,
                    "    StorageClass:\t{class}\n    Volume:\t{}",
                    text(&spec["volumeName"])
                )
                .unwrap();
                let header = super::metadata_header(&meta, false);
                for line in header
                    .split_once('\n')
                    .map(|(_, tail)| tail)
                    .unwrap_or_default()
                    .lines()
                {
                    writeln!(out, "    {}", line.replace('\t', "\t    ")).unwrap();
                }
                out.push_str("    Capacity:\t\n    Access Modes:\t\n");
                if !spec["volumeMode"].is_null() {
                    writeln!(out, "    VolumeMode:\t{}", text(&spec["volumeMode"])).unwrap();
                }
                let data = &spec["dataSource"];
                if !data.is_null() {
                    out.push_str("    DataSource:\n");
                    if !data["apiGroup"].is_null() {
                        writeln!(out, "      APIGroup:\t{}", text(&data["apiGroup"])).unwrap();
                    }
                    writeln!(
                        out,
                        "      Kind:\t{}\n      Name:\t{}",
                        text(&data["kind"]),
                        text(&data["name"])
                    )
                    .unwrap();
                }
            }
        }
        "downwardAPI" => {
            out.push_str("    Items:\n");
            for item in items(&value["items"]) {
                for (kind, field) in [("fieldRef", "fieldPath"), ("resourceFieldRef", "resource")] {
                    if !item[kind].is_null() {
                        writeln!(
                            out,
                            "      {} -> {}",
                            text(&item[kind][field]),
                            text(&item["path"])
                        )
                        .unwrap();
                    }
                }
            }
        }
        "fc" => writeln!(
            out,
            "    TargetWWNs:\t{}\n    LUN:\t{}\n    FSType:\t{}\n    ReadOnly:\t{}",
            items(&value["targetWWNs"])
                .iter()
                .map(text)
                .collect::<Vec<_>>()
                .join(", "),
            value["lun"]
                .as_i64()
                .map(|n| n.to_string())
                .unwrap_or("<none>".into()),
            text(&value["fsType"]),
            formatted(&value["readOnly"], Bool, persistent)
        )
        .unwrap(),
        "projected" => {
            for projection in items(&value["sources"]) {
                if !projection["secret"].is_null() {
                    writeln!(
                        out,
                        "    SecretName:\t{}\n    Optional:\t{}",
                        text(&projection["secret"]["name"]),
                        formatted(&projection["secret"]["optional"], Bool, false)
                    )
                    .unwrap();
                } else if !projection["downwardAPI"].is_null() {
                    out.push_str("    DownwardAPI:\ttrue\n");
                } else if !projection["configMap"].is_null() {
                    writeln!(
                        out,
                        "    ConfigMapName:\t{}\n    Optional:\t{}",
                        text(&projection["configMap"]["name"]),
                        formatted(&projection["configMap"]["optional"], Bool, false)
                    )
                    .unwrap();
                } else if !projection["serviceAccountToken"].is_null() {
                    writeln!(
                        out,
                        "    TokenExpirationSeconds:\t{}",
                        projection["serviceAccountToken"]["expirationSeconds"]
                            .as_i64()
                            .unwrap_or_default()
                    )
                    .unwrap();
                }
            }
        }
        "csi" => {
            out.push_str("    VolumeAttributes:\t");
            let mut pairs: Vec<_> = value["volumeAttributes"]
                .as_object()
                .into_iter()
                .flatten()
                .collect();
            pairs.sort_by_key(|(key, _)| *key);
            if pairs.is_empty() {
                out.push_str("<none>\n");
            }
            for (index, (key, value)) in pairs.iter().enumerate() {
                if index > 0 {
                    out.push_str("        \t");
                }
                let line = format!("{key}={}", text(value));
                let short = String::from_utf8_lossy(&line.as_bytes()[..line.len().min(140)]);
                writeln!(
                    out,
                    "    {short}{}",
                    if line.len() > 140 { "..." } else { "" }
                )
                .unwrap();
            }
        }
        _ => {}
    }
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
fn formatted(v: &Value, format: Format, persistent: bool) -> String {
    match format {
        Text => text(v).into(),
        Bool => v.as_bool().unwrap_or_default().to_string(),
        Int => v.as_i64().unwrap_or_default().to_string(),
        List => format!(
            "[{}]",
            items(v).iter().map(text).collect::<Vec<_>>().join(" ")
        ),
        Reference if v.is_null() => "nil".into(),
        Reference if persistent => format!(
            "&SecretReference{{Name:{},Namespace:{},}}",
            text(&v["name"]),
            text(&v["namespace"])
        ),
        Reference => format!("&LocalObjectReference{{Name:{},}}", text(&v["name"])),
        Map => {
            let mut pairs: Vec<_> = v
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| format!("{k}:{}", text(v)))
                .collect();
            pairs.sort();
            format!("map[{}]", pairs.join(" "))
        }
    }
}
