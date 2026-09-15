// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.37.0 pkg/describe/describe.go and component-helpers/resource.
// See LICENSE-APACHE.

use crate::api::{fetch_events, fetch_events_with_uid, list_objects};
use crate::events::with_events;
use crate::json::{items, text};
use crate::metadata::{annotation_section, identity, label_section};
use crate::quantity;
use crate::time::{age, optional_timestamp, value_timestamp};
use k8s_openapi::api::core::v1::Event;
use k8s_openapi::jiff::Timestamp;
use k8s_openapi::jiff::tz::TimeZone;
use kube::Client;
use kube::api::{Api, ApiResource, DynamicObject, ListParams};
use serde_json::Value;
use smallvec::SmallVec;
use std::collections::BTreeMap;
use std::fmt::Write;

pub(crate) struct Related {
    pub pods: Option<Vec<DynamicObject>>,
    pub lease: Result<DynamicObject, String>,
    pub events: Option<Vec<Event>>,
    pub resource_slices: Vec<DynamicObject>,
}

pub(crate) async fn related(client: Client, object: &DynamicObject) -> Result<Related, String> {
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
        fetch_events_with_uid(client.clone(), &object.metadata, "Node", Some(name)),
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

pub(crate) fn render(
    object: &DynamicObject,
    related: &Related,
    now: Timestamp,
    zone: &TimeZone,
) -> String {
    let spec = &object.data["spec"];
    let status = &object.data["status"];
    let mut out = identity(&object.metadata, false);
    let mut roles: SmallVec<[&str; 4]> = SmallVec::new();
    for (k, v) in object.metadata.labels.iter().flatten() {
        if let Some(role) = k.strip_prefix("node-role.kubernetes.io/")
            && !role.is_empty()
        {
            roles.push(role);
        }
        if k == "kubernetes.io/role" && !v.is_empty() {
            roles.push(v);
        }
    }
    roles.sort_unstable();
    roles.dedup();
    out.push_str("Roles:\t");
    if roles.is_empty() {
        out.push_str("<none>");
    } else {
        for (index, role) in roles.into_iter().enumerate() {
            if index != 0 {
                out.push(',');
            }
            out.push_str(role);
        }
    }
    out.push('\n');
    out.push_str(&label_section(&object.metadata, ""));
    out.push_str(&annotation_section(&object.metadata, ""));
    writeln!(
        out,
        "CreationTimestamp:\t{}",
        optional_timestamp(
            object
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|time| time.0),
            zone
        )
    )
    .unwrap();
    let mut taints: SmallVec<[&Value; 8]> = items(&spec["taints"]).iter().collect();
    taints.sort_by(|a, b| {
        text(&a["effect"])
            .cmp(text(&b["effect"]))
            .then_with(|| text(&a["key"]).cmp(text(&b["key"])))
    });
    out.push_str("Taints:\t");
    if taints.is_empty() {
        out.push_str("<none>");
    } else {
        for (index, taint) in taints.into_iter().enumerate() {
            if index != 0 {
                out.push_str("\n\t");
            }
            let (key, value, effect) = (
                text(&taint["key"]),
                text(&taint["value"]),
                text(&taint["effect"]),
            );
            out.push_str(key);
            if !value.is_empty() {
                write!(out, "={value}").unwrap();
            }
            if !value.is_empty() || !effect.is_empty() {
                write!(out, ":{effect}").unwrap();
            }
        }
    }
    writeln!(
        out,
        "\nUnschedulable:\t{}",
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
                        value_timestamp(&spec[field], zone)
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
                value_timestamp(&c["lastHeartbeatTime"], zone),
                value_timestamp(&c["lastTransitionTime"], zone),
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
            let mut resources: SmallVec<[_; 8]> = resources.iter().collect();
            resources.sort_unstable_by_key(|(name, _)| *name);
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
        out.push_str("PodCIDRs:\t");
        for (index, cidr) in items(&spec["podCIDRs"]).iter().enumerate() {
            if index != 0 {
                out.push(',');
            }
            out.push_str(text(cidr));
        }
        out.push('\n');
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
    let mut sorted: SmallVec<[_; 16]> = pools.iter().collect();
    sorted.sort_unstable_by(|((driver_a, pool_a), _), ((driver_b, pool_b), _)| {
        driver_a
            .bytes()
            .chain(std::iter::once(b'/'))
            .chain(pool_a.bytes())
            .cmp(
                driver_b
                    .bytes()
                    .chain(std::iter::once(b'/'))
                    .chain(pool_b.bytes()),
            )
    });
    for ((driver, pool), (slices, devices)) in sorted.into_iter().take(10) {
        writeln!(out, "  {driver}\t{pool}\t{slices}\t{devices}").unwrap();
    }
    if pools.len() > 10 {
        writeln!(out, "  ...and {} more pools", pools.len() - 10).unwrap();
    }
}

pub(crate) fn pod_level_resource(name: &str) -> bool {
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
        // This is the same decimal-suffix normalization as
        // `quantity::canonical("<nanos>n")`, without formatting a temporary
        // quantity and parsing it back on every rendered resource value.
        let mut value = self.nanos;
        let mut index = 0;
        let suffixes = ["n", "u", "m", "", "k", "M", "G", "T", "P", "E"];
        while index + 1 < suffixes.len() && value % 1000 == 0 {
            value /= 1000;
            index += 1;
        }
        write!(f, "{value}{}", suffixes[index])
    }
}
/// A resource list, keyed by names borrowed from the object being described.
///
/// A Kubernetes resource list holds a handful of entries — cpu, memory,
/// occasionally ephemeral-storage or a device-plugin name — and a node builds
/// several of them per container per pod. CPU and memory are kept in direct
/// slots because virtually every hot-path operation touches them; uncommon
/// names spill into a small sorted vector only after two inline entries.
#[derive(Clone, Default)]
struct Resources<'a> {
    cpu: Option<Quantity>,
    memory: Option<Quantity>,
    other: SmallVec<[(&'a str, Quantity); 2]>,
}

impl<'a> Resources<'a> {
    fn new() -> Self {
        Self::default()
    }
    fn slot(&self, key: &str) -> Result<usize, usize> {
        self.other.binary_search_by(|(name, _)| (*name).cmp(key))
    }
    fn get(&self, key: &str) -> Option<&Quantity> {
        match key {
            "cpu" => self.cpu.as_ref(),
            "memory" => self.memory.as_ref(),
            _ => self.slot(key).ok().map(|i| &self.other[i].1),
        }
    }
    fn insert(&mut self, key: &'a str, value: Quantity) {
        match key {
            "cpu" => {
                self.cpu = Some(value);
                return;
            }
            "memory" => {
                self.memory = Some(value);
                return;
            }
            _ => {}
        }
        match self.slot(key) {
            Ok(i) => self.other[i].1 = value,
            Err(i) => self.other.insert(i, (key, value)),
        }
    }
    /// The entry for `key`, inserted as zero when absent.
    fn entry(&mut self, key: &'a str) -> &mut Quantity {
        match key {
            "cpu" => return self.cpu.get_or_insert_default(),
            "memory" => return self.memory.get_or_insert_default(),
            _ => {}
        }
        let i = match self.slot(key) {
            Ok(i) => i,
            Err(i) => {
                self.other.insert(i, (key, Quantity::default()));
                i
            }
        };
        &mut self.other[i].1
    }
    #[cfg(test)]
    fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    fn keys(&self) -> SmallVec<[&'a str; 8]> {
        let mut keys: SmallVec<_> = self.other.iter().map(|(name, _)| *name).collect();
        if self.cpu.is_some() {
            keys.push("cpu");
        }
        if self.memory.is_some() {
            keys.push("memory");
        }
        keys.sort_unstable();
        keys
    }
}

impl std::ops::Index<&str> for Resources<'_> {
    type Output = Quantity;
    fn index(&self, key: &str) -> &Quantity {
        self.get(key).expect("no such resource")
    }
}

fn quantities(value: &Value) -> Resources<'_> {
    let mut resources = Resources::new();
    for (name, value) in quantity_values(value) {
        resources.insert(name, value);
    }
    resources
}

fn quantity_values(value: &Value) -> impl Iterator<Item = (&str, Quantity)> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.as_str(), Quantity::from_value(v)))
}
fn add<'a>(into: &mut Resources<'a>, other: &Resources<'a>) {
    if let Some(value) = other.cpu {
        into.entry("cpu").add(value);
    }
    if let Some(value) = other.memory {
        into.entry("memory").add(value);
    }
    for &(name, value) in &other.other {
        into.entry(name).add(value);
    }
}

fn maximum<'a>(into: &mut Resources<'a>, other: &Resources<'a>) {
    let mut apply = |name: &'a str, value: Quantity| {
        if into.get(name).is_none_or(|old| old.nanos < value.nanos) {
            into.insert(name, value);
        }
    };
    if let Some(value) = other.cpu {
        apply("cpu", value);
    }
    if let Some(value) = other.memory {
        apply("memory", value);
    }
    for &(name, value) in &other.other {
        apply(name, value);
    }
}

#[derive(Clone, Copy)]
enum ResourceView {
    Allocated,
    Actuated,
}

/// Borrow a pod's two status slices once and share them between aggregates.
/// Container lists are normally very short, so a reverse linear lookup is
/// cheaper than building a heap-allocated tree. Reversing also preserves the
/// old map's last-entry-wins behaviour for malformed duplicate names.
struct StatusIndex<'a> {
    containers: &'a [Value],
    init_containers: &'a [Value],
}

impl<'a> StatusIndex<'a> {
    fn get(&self, name: &str) -> Option<&'a Value> {
        self.init_containers
            .iter()
            .rev()
            .chain(self.containers.iter().rev())
            .find(|status| text(&status["name"]) == name)
    }
}

fn status_index(pod: &DynamicObject) -> StatusIndex<'_> {
    StatusIndex {
        containers: items(&pod.data["status"]["containerStatuses"]),
        init_containers: items(&pod.data["status"]["initContainerStatuses"]),
    }
}

fn status_resources<'a>(
    container: &'a Value,
    status: Option<&'a Value>,
    field: &str,
    view: ResourceView,
    infeasible: bool,
) -> &'a Value {
    let status = status.unwrap_or(&Value::Null);
    match view {
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
    }
}

/// The regular-container sum and the restartable/non-restartable init-container
/// maximum for one resource view. All storage remains inline for the usual
/// cpu/memory-sized resource lists.
#[derive(Default)]
struct ContainerAggregate<'a> {
    total: Resources<'a>,
    restartable: Resources<'a>,
    init: Resources<'a>,
}

impl<'a> ContainerAggregate<'a> {
    fn add(&mut self, values: &Resources<'a>, init: bool, restartable: bool) {
        if init {
            self.init(values, restartable);
        } else {
            self.regular(values);
        }
    }

    fn regular(&mut self, values: &Resources<'a>) {
        add(&mut self.total, values);
    }

    fn init(&mut self, values: &Resources<'a>, restartable: bool) {
        let mut values = values.clone();
        if restartable {
            add(&mut self.total, &values);
            add(&mut self.restartable, &values);
            values.clone_from(&self.restartable);
        } else {
            add(&mut values, &self.restartable);
        }
        maximum(&mut self.init, &values);
    }

    fn finish(mut self) -> Resources<'a> {
        maximum(&mut self.total, &self.init);
        self.total
    }
}

/// Apply the pod-level resources and overhead that sit outside the containers.
fn apply_pod_level<'a>(total: &mut Resources<'a>, spec: &'a Value, field: &str) {
    for (name, value) in quantity_values(&spec["resources"][field]) {
        if pod_level_resource(name) {
            total.insert(name, value);
        }
    }
    for (name, value) in quantity_values(&spec["overhead"]) {
        if field == "requests" || total.get(name).is_some_and(|q| q.nanos != 0) {
            total.entry(name).add(value);
        }
    }
}

struct PodResources<'a> {
    requests: Resources<'a>,
    request_spec: Resources<'a>,
    limits: Resources<'a>,
    limit_spec: Resources<'a>,
}

#[derive(Default)]
struct PodAggregates<'a> {
    request_spec: ContainerAggregate<'a>,
    limit_spec: ContainerAggregate<'a>,
    request_actuated: ContainerAggregate<'a>,
    limit_actuated: ContainerAggregate<'a>,
    request_allocated: ContainerAggregate<'a>,
}

impl<'a> PodAggregates<'a> {
    fn add(
        &mut self,
        container: &'a Value,
        status: Option<&'a Value>,
        init: bool,
        infeasible: bool,
    ) {
        let restartable = init && text(&container["restartPolicy"]) == "Always";
        let request_spec_value = &container["resources"]["requests"];
        let limit_spec_value = &container["resources"]["limits"];
        let request_spec = quantities(request_spec_value);
        let limit_spec = quantities(limit_spec_value);
        self.request_spec.add(&request_spec, init, restartable);
        self.limit_spec.add(&limit_spec, init, restartable);

        let request_actuated_value = status_resources(
            container,
            status,
            "requests",
            ResourceView::Actuated,
            infeasible,
        );
        let request_actuated = (!std::ptr::eq(request_actuated_value, request_spec_value))
            .then(|| quantities(request_actuated_value));
        let request_actuated = request_actuated.as_ref().unwrap_or(&request_spec);
        self.request_actuated
            .add(request_actuated, init, restartable);

        let limit_actuated_value = status_resources(
            container,
            status,
            "limits",
            ResourceView::Actuated,
            infeasible,
        );
        let limit_actuated = (!std::ptr::eq(limit_actuated_value, limit_spec_value))
            .then(|| quantities(limit_actuated_value));
        self.limit_actuated.add(
            limit_actuated.as_ref().unwrap_or(&limit_spec),
            init,
            restartable,
        );

        let request_allocated_value = status_resources(
            container,
            status,
            "requests",
            ResourceView::Allocated,
            infeasible,
        );
        let request_allocated = if std::ptr::eq(request_allocated_value, request_spec_value) {
            &request_spec
        } else if std::ptr::eq(request_allocated_value, request_actuated_value) {
            request_actuated
        } else {
            &quantities(request_allocated_value)
        };
        self.request_allocated
            .add(request_allocated, init, restartable);
    }
}

/// Compute every resource view the node table needs in one container walk.
/// This shares status lookup, JSON traversal and the resize condition between
/// requests and limits while still parsing each distinct quantity only once.
fn pod_resources_all(pod: &DynamicObject) -> PodResources<'_> {
    let spec = &pod.data["spec"];
    let statuses = status_index(pod);
    let infeasible = items(&pod.data["status"]["conditions"])
        .iter()
        .find(|c| text(&c["type"]) == "PodResizePending")
        .is_some_and(|c| text(&c["reason"]) == "Infeasible");

    let mut aggregates = PodAggregates::default();
    for container in items(&spec["containers"]) {
        let status = statuses.get(text(&container["name"]));
        aggregates.add(container, status, false, infeasible);
    }
    for container in items(&spec["initContainers"]) {
        let status = statuses.get(text(&container["name"]));
        aggregates.add(container, status, true, infeasible);
    }

    let mut request_spec = aggregates.request_spec.finish();
    let mut limit_spec = aggregates.limit_spec.finish();
    let mut requests = if infeasible {
        Resources::new()
    } else {
        request_spec.clone()
    };
    let mut limits = if infeasible {
        Resources::new()
    } else {
        limit_spec.clone()
    };
    // Match v0.37's maximum of whole-pod aggregates, not a sum of
    // per-container maxima.
    maximum(&mut requests, &aggregates.request_actuated.finish());
    maximum(&mut requests, &aggregates.request_allocated.finish());
    maximum(&mut limits, &aggregates.limit_actuated.finish());
    apply_pod_level(&mut requests, spec, "requests");
    apply_pod_level(&mut request_spec, spec, "requests");
    apply_pod_level(&mut limits, spec, "limits");
    apply_pod_level(&mut limit_spec, spec, "limits");
    PodResources {
        requests,
        request_spec,
        limits,
        limit_spec,
    }
}

/// Single-field helper retained for the unit tests, which assert one view at a
/// time. The describer itself computes both views together.
#[cfg(test)]
fn pod_resources<'a>(pod: &'a DynamicObject, field: &str, use_status: bool) -> Resources<'a> {
    let resources = pod_resources_all(pod);
    match (field, use_status) {
        ("requests", true) => resources.requests,
        ("requests", false) => resources.request_spec,
        ("limits", true) => resources.limits,
        ("limits", false) => resources.limit_spec,
        _ => unreachable!(),
    }
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
    // The per-Pod table dominates this buffer. Reserve its typical row size up
    // front instead of repeatedly copying the growing document.
    out.reserve(pods.len().saturating_mul(160));
    writeln!(out,"Non-terminated Pods:\t({} in total)\n  Namespace\tName\t\tCPU Requests\tCPU Limits\tMemory Requests\tMemory Limits\tAge\n  ---------\t----\t\t------------\t----------\t---------------\t-------------\t---",pods.len()).unwrap();
    let (mut requests, mut limits) = (Resources::new(), Resources::new());
    for pod in pods {
        let resources = pod_resources_all(pod);
        write!(
            out,
            "  {}\t{}\t",
            pod.metadata.namespace.as_deref().unwrap_or_default(),
            pod.metadata.name.as_deref().unwrap_or_default()
        )
        .unwrap();
        for (name, values) in [
            ("cpu", &resources.requests),
            ("cpu", &resources.limits),
            ("memory", &resources.requests),
            ("memory", &resources.limits),
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
        add(&mut requests, &resources.request_spec);
        add(&mut limits, &resources.limit_spec);
    }
    out.push_str("Allocated resources:\n  (Total limits may be over 100 percent, i.e., overcommitted.)\n  Resource\tRequests\tLimits\n  --------\t--------\t------\n");
    let standard = ["cpu", "memory", "ephemeral-storage"];
    let allocatable_names = allocatable.keys();
    let names = standard
        .into_iter()
        .chain(
            allocatable_names
                .iter()
                .copied()
                .filter(|n| n.starts_with("hugepages-")),
        )
        .chain(
            allocatable_names
                .iter()
                .copied()
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
    use super::{Quantity, pod_resources, resource_slices};
    use kube::api::DynamicObject;
    use serde_json::json;

    #[test]
    fn quantity_display_matches_canonical_formatting() {
        for input in [
            "0", "1n", "1500n", "250m", "1", "128Mi", "1.5Gi", "1e3", "1000e-3",
        ] {
            let value = serde_json::Value::String(input.into());
            assert_eq!(
                Quantity::from_value(&value).to_string(),
                crate::quantity::canonical(input),
                "{input}"
            );
        }
    }

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
