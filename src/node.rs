// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 sofka contributors.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.37.0 pkg/describe/describe.go and component-helpers/resource.
// See LICENSE-APACHE.

use super::*;
use k8s_openapi::jiff::tz::TimeZone;
use serde_json::Value;
fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
pub(super) struct Related {
    pub pods: Option<Vec<DynamicObject>>,
    pub lease: Result<DynamicObject, String>,
    pub events: Option<Vec<Event>>,
    pub resource_slices: Vec<DynamicObject>,
}

pub(super) async fn related(client: Client, object: &DynamicObject) -> Result<Related, String> {
    let name = object.metadata.name.as_deref().unwrap_or_default();
    let pod_ar = ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk("", "v1", "Pod"));
    let lease_ar = ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk(
        "coordination.k8s.io",
        "v1",
        "Lease",
    ));
    let lease_api =
        Api::<DynamicObject>::namespaced_with(client.clone(), "kube-node-lease", &lease_ar);
    let params = ListParams::default().fields(&format!(
        "spec.nodeName={name},status.phase!=Succeeded,status.phase!=Failed"
    ));
    let mut name_ref = object.metadata.clone();
    name_ref.uid = Some(name.into());
    let slice_ar = ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk(
        "resource.k8s.io",
        "v1",
        "ResourceSlice",
    ));
    let (pods, lease, events, name_events, resource_slices) = tokio::join!(
        list_objects(
            client.clone(),
            &pod_ar,
            object.metadata.namespace.as_deref().unwrap_or_default(),
            params
        ),
        lease_api.get(name),
        fetch_events(client.clone(), &object.metadata, "Node"),
        fetch_events(client.clone(), &name_ref, "Node"),
        list_objects(
            client,
            &slice_ar,
            "",
            ListParams::default().fields(&format!("spec.nodeName={name}"))
        )
    );
    let pods = match pods {
        Ok(pods) => Some(pods),
        Err(kube::Error::Api(error)) if error.code == 403 => None,
        Err(error) => return Err(error.to_string()),
    };
    let mut events = events.ok();
    if let Ok(more) = name_events {
        events.get_or_insert_default().extend(more);
    }
    Ok(Related {
        pods,
        lease: lease.map_err(|e| match e {
            kube::Error::Api(e) => e.message,
            _ => e.to_string(),
        }),
        events,
        resource_slices: resource_slices.unwrap_or_default(),
    })
}

pub(super) fn render(
    object: &DynamicObject,
    related: &Related,
    now: Timestamp,
    zone: &TimeZone,
) -> String {
    let spec = &object.data["spec"];
    let status = &object.data["status"];
    let header = metadata_header(&object.metadata, false);
    let (name, tail) = header.split_once('\n').unwrap();
    let mut roles = std::collections::BTreeSet::new();
    for (k, v) in object.metadata.labels.iter().flatten() {
        if let Some(role) = k.strip_prefix("node-role.kubernetes.io/")
            && !role.is_empty()
        {
            roles.insert(role);
        }
        if k == "kubernetes.io/role" && !v.is_empty() {
            roles.insert(v);
        }
    }
    let mut out = format!(
        "{name}\nRoles:\t{}\n{tail}",
        if roles.is_empty() {
            "<none>".into()
        } else {
            roles.into_iter().collect::<Vec<_>>().join(",")
        }
    );
    writeln!(
        out,
        "CreationTimestamp:\t{}",
        containers::timestamp(
            &serde_json::to_value(&object.metadata.creation_timestamp).unwrap_or_default(),
            zone
        )
    )
    .unwrap();
    let mut taints: Vec<_> = items(&spec["taints"]).iter().collect();
    taints.sort_by_key(|t| format!("{},{}", text(&t["effect"]), text(&t["key"])));
    let taints = taints
        .iter()
        .map(|t| {
            let (key, value, effect) = (text(&t["key"]), text(&t["value"]), text(&t["effect"]));
            if value.is_empty() && effect.is_empty() {
                key.into()
            } else {
                format!(
                    "{key}{}:{effect}",
                    if value.is_empty() {
                        String::new()
                    } else {
                        format!("={value}")
                    }
                )
            }
        })
        .collect::<Vec<String>>();
    writeln!(
        out,
        "Taints:\t{}\nUnschedulable:\t{}",
        if taints.is_empty() {
            "<none>".into()
        } else {
            taints.join("\n\t")
        },
        spec["unschedulable"].as_bool().unwrap_or_default()
    )
    .unwrap();
    match &related.lease {
        Ok(lease) => {
            let spec = &lease.data["spec"];
            writeln!(
                out,
                "Lease:\n  HolderIdentity:\t{}",
                spec["holderIdentity"].as_str().unwrap_or("<unset>")
            )
            .unwrap();
            for (field, label) in [("acquireTime", "AcquireTime"), ("renewTime", "RenewTime")] {
                writeln!(
                    out,
                    "  {label}:\t{}",
                    if spec[field].is_null() {
                        "<unset>".into()
                    } else {
                        containers::timestamp(&spec[field], zone)
                    }
                )
                .unwrap();
            }
        }
        Err(error) => {
            writeln!(out, "Lease:\tFailed to get lease: {error}").unwrap();
        }
    }
    let conditions = items(&status["conditions"]);
    if !conditions.is_empty() {
        out.push_str("Conditions:\n  Type\tStatus\tLastHeartbeatTime\tLastTransitionTime\tReason\tMessage\n  ----\t------\t-----------------\t------------------\t------\t-------\n");
        for c in conditions {
            writeln!(
                out,
                "  {} \t{} \t{} \t{} \t{} \t{}",
                text(&c["type"]),
                text(&c["status"]),
                containers::timestamp(&c["lastHeartbeatTime"], zone),
                containers::timestamp(&c["lastTransitionTime"], zone),
                text(&c["reason"]),
                text(&c["message"])
            )
            .unwrap();
        }
    }
    out.push_str("Addresses:\n");
    for address in items(&status["addresses"]) {
        writeln!(
            out,
            "  {}:\t{}",
            text(&address["type"]),
            text(&address["address"])
        )
        .unwrap();
    }
    for (field, label) in [("capacity", "Capacity"), ("allocatable", "Allocatable")] {
        if let Some(resources) = status[field].as_object().filter(|r| !r.is_empty()) {
            writeln!(out, "{label}:").unwrap();
            let resources: BTreeMap<_, _> = resources.iter().collect();
            for (name, value) in resources {
                writeln!(out, "  {name}:\t{}", quantity::canonical(text(value))).unwrap();
            }
        }
    }
    resource_slices(&mut out, &related.resource_slices);
    out.push_str("System Info:\n");
    for (field, label) in [
        ("machineID", "Machine ID"),
        ("systemUUID", "System UUID"),
        ("bootID", "Boot ID"),
        ("kernelVersion", "Kernel Version"),
        ("osImage", "OS Image"),
        ("operatingSystem", "Operating System"),
        ("architecture", "Architecture"),
        ("containerRuntimeVersion", "Container Runtime Version"),
        ("kubeletVersion", "Kubelet Version"),
    ] {
        writeln!(out, "  {label}:\t{}", text(&status["nodeInfo"][field])).unwrap();
    }
    if !text(&status["nodeInfo"]["kubeProxyVersion"]).is_empty() {
        writeln!(
            out,
            "  Kube-Proxy Version:\t{}",
            text(&status["nodeInfo"]["kubeProxyVersion"])
        )
        .unwrap();
    }
    if !text(&spec["podCIDR"]).is_empty() {
        writeln!(out, "PodCIDR:\t{}", text(&spec["podCIDR"])).unwrap();
    }
    if !items(&spec["podCIDRs"]).is_empty() {
        writeln!(
            out,
            "PodCIDRs:\t{}",
            items(&spec["podCIDRs"])
                .iter()
                .map(text)
                .collect::<Vec<_>>()
                .join(",")
        )
        .unwrap();
    }
    if !text(&spec["providerID"]).is_empty() {
        writeln!(out, "ProviderID:\t{}", text(&spec["providerID"])).unwrap();
    }
    if let Some(pods) = &related.pods {
        resources(&mut out, pods, object, now);
    } else {
        out.push_str("Pods:\tnot authorized\n");
    }
    with_events(out, related.events.as_deref(), now)
}

fn resource_slices(out: &mut String, slices: &[DynamicObject]) {
    let mut pools: BTreeMap<(&str, &str), (usize, usize)> = BTreeMap::new();
    for slice in slices {
        let spec = &slice.data["spec"];
        let counts = pools
            .entry((text(&spec["driver"]), text(&spec["pool"]["name"])))
            .or_default();
        counts.0 += 1;
        counts.1 += items(&spec["devices"]).len();
    }
    if pools.is_empty() {
        return;
    }
    out.push_str("Node-Local ResourceSlices:\n  Driver\tPool\tSlices\tDevices\n  ------\t----\t------\t-------\n");
    let mut sorted: Vec<_> = pools.iter().collect();
    sorted.sort_by_cached_key(|((driver, pool), _)| format!("{driver}/{pool}"));
    for ((driver, pool), (slices, devices)) in sorted.into_iter().take(10) {
        writeln!(out, "  {driver}\t{pool}\t{slices}\t{devices}").unwrap();
    }
    if pools.len() > 10 {
        writeln!(out, "  ...and {} more pools", pools.len() - 10).unwrap();
    }
}

pub(super) fn pod_level_resource(name: &str) -> bool {
    matches!(name, "cpu" | "memory") || name.starts_with("hugepages-")
}

#[derive(Clone, Copy, Default)]
struct Quantity {
    nanos: i128,
    binary: bool,
    exponent: bool,
}
impl Quantity {
    fn from_value(value: &Value) -> Self {
        let s = text(value);
        Self {
            nanos: quantity::parse(s).unwrap_or_default(),
            binary: s.ends_with('i'),
            exponent: s.contains('e') || s.find('E').is_some_and(|i| i + 1 < s.len()),
        }
    }
    fn add(&mut self, other: Self) {
        if self.nanos == 0 {
            *self = other;
        } else {
            self.nanos += other.nanos;
        }
    }
    fn scaled(self, scale: i128) -> i128 {
        self.nanos / scale + i128::from(self.nanos % scale > 0) - i128::from(self.nanos % scale < 0)
    }
}
impl std::fmt::Display for Quantity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.nanos == 0 {
            return f.write_str("0");
        }
        if self.binary && self.nanos.abs() >= 1_024_000_000_000 && self.nanos % 1_000_000_000 == 0 {
            let mut n = self.nanos / 1_000_000_000;
            let mut i = 0;
            let suffixes = ["", "Ki", "Mi", "Gi", "Ti", "Pi", "Ei"];
            while n % 1024 == 0 && i < 6 {
                n /= 1024;
                i += 1;
            }
            return write!(f, "{n}{}", suffixes[i]);
        }
        if self.exponent {
            let mut n = self.nanos;
            let mut exponent = -9;
            while n % 1000 == 0 {
                n /= 1000;
                exponent += 3;
            }
            return if exponent == 0 {
                write!(f, "{n}")
            } else {
                write!(f, "{n}e{exponent}")
            };
        }
        f.write_str(&quantity::canonical(&format!("{}n", self.nanos)))
    }
}
type Resources = BTreeMap<String, Quantity>;
fn quantities(value: &Value) -> Resources {
    value
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), Quantity::from_value(v)))
        .collect()
}
fn add(into: &mut Resources, other: &Resources) {
    for (k, v) in other {
        into.entry(k.clone()).or_default().add(*v);
    }
}
fn maximum(into: &mut Resources, other: &Resources) {
    for (k, v) in other {
        if into.get(k).is_none_or(|old| old.nanos < v.nanos) {
            into.insert(k.clone(), *v);
        }
    }
}

#[derive(Clone, Copy)]
enum ResourceView {
    Spec,
    Allocated,
    Actuated,
}

fn aggregate_containers(
    pod: &DynamicObject,
    field: &str,
    view: ResourceView,
    infeasible: bool,
) -> Resources {
    let spec = &pod.data["spec"];
    let statuses: BTreeMap<_, _> = items(&pod.data["status"]["containerStatuses"])
        .iter()
        .chain(items(&pod.data["status"]["initContainerStatuses"]))
        .map(|s| (text(&s["name"]), s))
        .collect();
    let container_resources = |container: &Value| {
        let status = statuses
            .get(text(&container["name"]))
            .copied()
            .unwrap_or(&Value::Null);
        let value = match view {
            ResourceView::Spec => &container["resources"][field],
            ResourceView::Actuated if status["resources"][field].is_object() => {
                &status["resources"][field]
            }
            ResourceView::Allocated | ResourceView::Actuated
                if field == "requests" && status["allocatedResources"].is_object() =>
            {
                &status["allocatedResources"]
            }
            _ if infeasible => &Value::Null,
            _ => &container["resources"][field],
        };
        quantities(value)
    };
    let mut total = Resources::new();
    for c in items(&spec["containers"]) {
        add(&mut total, &container_resources(c));
    }
    let (mut restartable, mut init) = (Resources::new(), Resources::new());
    for c in items(&spec["initContainers"]) {
        let mut values = container_resources(c);
        if text(&c["restartPolicy"]) == "Always" {
            add(&mut total, &values);
            add(&mut restartable, &values);
            values = restartable.clone();
        } else {
            add(&mut values, &restartable);
        }
        maximum(&mut init, &values);
    }
    maximum(&mut total, &init);
    total
}

fn pod_resources(pod: &DynamicObject, field: &str, use_status: bool) -> Resources {
    let spec = &pod.data["spec"];
    let infeasible = use_status
        && items(&pod.data["status"]["conditions"])
            .iter()
            .find(|c| text(&c["type"]) == "PodResizePending")
            .is_some_and(|c| text(&c["reason"]) == "Infeasible");
    // Match v0.37's maximum of whole-pod aggregates, not a sum of per-container maxima.
    let mut total = if infeasible {
        Resources::new()
    } else {
        aggregate_containers(pod, field, ResourceView::Spec, false)
    };
    if use_status {
        maximum(
            &mut total,
            &aggregate_containers(pod, field, ResourceView::Actuated, infeasible),
        );
        if field == "requests" {
            maximum(
                &mut total,
                &aggregate_containers(pod, field, ResourceView::Allocated, infeasible),
            );
        }
    }
    for (name, value) in quantities(&spec["resources"][field]) {
        if pod_level_resource(&name) {
            total.insert(name, value);
        }
    }
    for (name, value) in quantities(&spec["overhead"]) {
        if field == "requests" || total.get(&name).is_some_and(|q| q.nanos != 0) {
            total.entry(name).or_default().add(value);
        }
    }
    total
}

fn percent(value: Quantity, capacity: Quantity, cpu: bool, guard: bool) -> i64 {
    let scale = if cpu { 1_000_000 } else { 1_000_000_000 };
    let capacity = capacity.scaled(scale);
    if capacity == 0 {
        return if guard { 0 } else { i64::MIN };
    }
    (value.scaled(scale) as f64 / capacity as f64 * 100.) as i64
}
fn resources(out: &mut String, pods: &[DynamicObject], node: &DynamicObject, now: Timestamp) {
    let status = &node.data["status"];
    let allocatable = if status["allocatable"]
        .as_object()
        .is_some_and(|v| !v.is_empty())
    {
        quantities(&status["allocatable"])
    } else {
        quantities(&status["capacity"])
    };
    writeln!(out,"Non-terminated Pods:\t({} in total)\n  Namespace\tName\t\tCPU Requests\tCPU Limits\tMemory Requests\tMemory Limits\tAge\n  ---------\t----\t\t------------\t----------\t---------------\t-------------\t---",pods.len()).unwrap();
    let (mut requests, mut limits) = (Resources::new(), Resources::new());
    for pod in pods {
        let req = pod_resources(pod, "requests", true);
        let lim = pod_resources(pod, "limits", true);
        write!(
            out,
            "  {}\t{}\t",
            pod.metadata.namespace.as_deref().unwrap_or_default(),
            pod.metadata.name.as_deref().unwrap_or_default()
        )
        .unwrap();
        for (name, values) in [
            ("cpu", &req),
            ("cpu", &lim),
            ("memory", &req),
            ("memory", &lim),
        ] {
            let value = values.get(name).copied().unwrap_or_default();
            write!(
                out,
                "\t{value} ({}%)",
                percent(
                    value,
                    allocatable.get(name).copied().unwrap_or_default(),
                    name == "cpu",
                    false
                )
            )
            .unwrap();
        }
        writeln!(
            out,
            "\t{}",
            age(pod.metadata.creation_timestamp.as_ref().map(|t| t.0), now)
        )
        .unwrap();
        // Upstream uses spec resources for the totals, status-aware values for each row.
        add(&mut requests, &pod_resources(pod, "requests", false));
        add(&mut limits, &pod_resources(pod, "limits", false));
    }
    out.push_str("Allocated resources:\n  (Total limits may be over 100 percent, i.e., overcommitted.)\n  Resource\tRequests\tLimits\n  --------\t--------\t------\n");
    let standard = ["cpu", "memory", "ephemeral-storage"];
    let names = standard
        .into_iter()
        .chain(
            allocatable
                .keys()
                .map(String::as_str)
                .filter(|n| n.starts_with("hugepages-")),
        )
        .chain(
            allocatable
                .keys()
                .map(String::as_str)
                .filter(|n| !standard.contains(n) && *n != "pods" && !n.starts_with("hugepages-")),
        );
    for name in names {
        write!(out, "  {name}").unwrap();
        for values in [&requests, &limits] {
            let value = values.get(name).copied().unwrap_or_default();
            write!(out, "\t{value}").unwrap();
            if standard.contains(&name) || name.starts_with("hugepages-") {
                write!(
                    out,
                    " ({}%)",
                    percent(
                        value,
                        allocatable.get(name).copied().unwrap_or_default(),
                        name == "cpu",
                        true
                    )
                )
                .unwrap();
            }
        }
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pod_level_resources_override_only_supported_resources_before_overhead() {
        let pod = serde_json::from_value(json!({"spec":{"containers":[{"name":"web","resources":{"requests":{"cpu":"1","ephemeral-storage":"1Mi"},"limits":{"cpu":"1","ephemeral-storage":"1Gi"}}}],"resources":{"requests":{"cpu":"2","memory":"2Gi","hugepages-2Mi":"4Mi","ephemeral-storage":"99Gi"},"limits":{"cpu":"0","memory":"4Gi","ephemeral-storage":"99Gi"}},"overhead":{"cpu":"100m","memory":"64Mi","ephemeral-storage":"10Mi"}},"status":{"resources":{"requests":{"cpu":"99"}}}})).unwrap();
        for use_status in [false, true] {
            let requests = pod_resources(&pod, "requests", use_status);
            let limits = pod_resources(&pod, "limits", use_status);
            for (name, expected) in [
                ("cpu", "2100m"),
                ("memory", "2112Mi"),
                ("hugepages-2Mi", "4Mi"),
                ("ephemeral-storage", "11Mi"),
            ] {
                assert_eq!(requests[name].to_string(), expected);
            }
            for (name, expected) in [
                ("cpu", "0"),
                ("memory", "4160Mi"),
                ("ephemeral-storage", "1034Mi"),
            ] {
                assert_eq!(limits[name].to_string(), expected);
            }
        }
    }

    #[test]
    fn infeasible_resize_uses_status_for_regular_and_restartable_init_containers() {
        let mut pod: DynamicObject = serde_json::from_value(json!({"spec":{"containers":[{"name":"web","resources":{"requests":{"cpu":"20"},"limits":{"cpu":"30"}}}],"initContainers":[{"name":"init","resources":{"requests":{"cpu":"40"},"limits":{"cpu":"50"}}},{"name":"sidecar","restartPolicy":"Always","resources":{"requests":{"cpu":"10"},"limits":{"cpu":"12"}}}],"overhead":{"cpu":"1"}},"status":{"conditions":[{"type":"PodResizePending","reason":"Infeasible"}],"containerStatuses":[{"name":"web","allocatedResources":{"cpu":"4"},"resources":{"requests":{"cpu":"2"},"limits":{"cpu":"3"}}}],"initContainerStatuses":[{"name":"init","allocatedResources":{"cpu":"7"},"resources":{"requests":{"cpu":"6"},"limits":{"cpu":"8"}}},{"name":"sidecar","allocatedResources":{"cpu":"2"},"resources":{"requests":{"cpu":"1"},"limits":{"cpu":"2"}}}]}})).unwrap();
        assert_eq!(
            pod_resources(&pod, "requests", true)["cpu"].to_string(),
            "8"
        );
        assert_eq!(pod_resources(&pod, "limits", true)["cpu"].to_string(), "9");
        assert_eq!(
            pod_resources(&pod, "requests", false)["cpu"].to_string(),
            "41"
        );
        assert_eq!(
            pod_resources(&pod, "limits", false)["cpu"].to_string(),
            "51"
        );
        pod.data["status"]["containerStatuses"] = json!([]);
        pod.data["status"]["initContainerStatuses"] = json!([]);
        assert_eq!(
            pod_resources(&pod, "requests", true)["cpu"].to_string(),
            "1"
        );
        assert!(!pod_resources(&pod, "limits", true).contains_key("cpu"));
    }

    #[test]
    fn resource_slice_pools_are_aggregated_sorted_and_capped() {
        let mut slices: Vec<DynamicObject> = (0..12).rev().map(|i| serde_json::from_value(json!({"spec":{"driver":"gpu.example.com","pool":{"name":format!("pool-{i:02}")},"devices":[{}]}})).unwrap()).collect();
        slices.push(slices[11].clone());
        let mut output = String::new();
        resource_slices(&mut output, &slices);
        assert!(output.contains("gpu.example.com\tpool-00\t2\t2"));
        assert!(output.find("pool-00").unwrap() < output.find("pool-09").unwrap());
        assert!(!output.contains("pool-10"));
        assert!(output.contains("...and 2 more pools"));
    }
}
