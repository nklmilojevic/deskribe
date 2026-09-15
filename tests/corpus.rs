use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use deskribe::{RenderOptions, gather};
use kube::{
    Client,
    api::{ApiResource, DynamicObject},
    core::GroupVersionKind,
};
use serde_json::{Value, json};

async fn render_fixture(fixture: &Value) -> String {
    let object: DynamicObject = serde_json::from_value(fixture["object"].clone()).unwrap();
    let types = object.types.as_ref().unwrap();
    let (group, version) = types
        .api_version
        .split_once('/')
        .unwrap_or(("", &types.api_version));
    let mut resource = ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, &types.kind));
    if let Some(plural) = fixture["plural"].as_str() {
        resource.plural = plural.into();
    }
    let prefix = if group.is_empty() {
        format!("/api/{version}")
    } else {
        format!("/apis/{group}/{version}")
    };
    let scope = object
        .metadata
        .namespace
        .as_deref()
        .filter(|ns| !ns.is_empty())
        .map(|ns| format!("/namespaces/{ns}"))
        .unwrap_or_default();
    let object_path = format!(
        "{prefix}{scope}/{}/{}",
        resource.plural,
        object.metadata.name.as_deref().unwrap()
    );
    let mut responses: BTreeMap<String, Value> = fixture["responses"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    responses.insert(object_path.clone(), fixture["object"].clone());
    if let Some(path) = fixture["relatedPath"].as_str() {
        let (default_version, default_kind) = if path.ends_with("/replicasets") {
            ("apps/v1", "ReplicaSetList")
        } else {
            ("v1", "PodList")
        };
        let version = fixture["relatedVersion"]
            .as_str()
            .unwrap_or(default_version);
        let kind = fixture["relatedKind"]
            .as_str()
            .map(|kind| format!("{kind}List"))
            .unwrap_or_else(|| default_kind.into());
        responses.insert(
            path.into(),
            json!({"apiVersion":version,"kind":kind,"metadata":{},"items":fixture["related"]}),
        );
    }
    let events = fixture.get("events").cloned().unwrap_or_else(|| json!([]));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let client = Client::new(
        tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(request.method(), http::Method::GET);
            let path = request.uri().path().to_owned();
            seen.lock().unwrap().push(path.clone());
            let body = responses.get(&path).cloned().unwrap_or_else(|| {
            if path.ends_with("/events") { json!({"apiVersion":"v1","kind":"EventList","items":events}) }
            else if path.ends_with("/pods") { json!({"apiVersion":"v1","kind":"PodList","items":[]}) }
            else if path.ends_with("/namespaces") { json!({"apiVersion":"v1","kind":"NamespaceList","items":[]}) }
            else if path.ends_with("/resourceslices") { json!({"apiVersion":"resource.k8s.io/v1","kind":"ResourceSliceList","items":[]}) }
            else if path.ends_with("/endpointslices") { json!({"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSliceList","items":[]}) }
            else { panic!("unexpected GET {path}") }
        });
            async move {
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(200)
                        .body(kube::client::Body::from(body.to_string().into_bytes()))
                        .unwrap(),
                )
            }
        }),
        "default",
    );
    let description = gather(client, &resource, &object).await.unwrap();
    let options = RenderOptions {
        now: "2026-01-01T00:00:00Z".parse().unwrap(),
        timezone: k8s_openapi::jiff::tz::TimeZone::UTC,
    };
    let output = description.render(&options).unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|path| **path == object_path)
            .count(),
        1
    );
    output
}

#[tokio::test]
async fn public_gather_and_render_match_regression_corpus() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/describe");
    let mut paths: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    for path in paths {
        let fixture: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let output = render_fixture(&fixture).await;
        assert_eq!(
            output,
            std::fs::read_to_string(path.with_extension("txt")).unwrap(),
            "{}",
            path.display()
        );
        assert!(!output.contains("SYNTHETIC-DO-NOT-PRINT"));
        assert!(!output.contains("MUST NOT SHOW"));
    }
}

#[tokio::test]
async fn describe_037_fields() {
    let cases = [
        (
            "services",
            "v1",
            "Service",
            json!({"ports":[{"name":"web","port":80,"targetPort":8080,"protocol":"TCP","appProtocol":"kubernetes.io/h2c"}]}),
            vec!["AppProtocol: kubernetes.io/h2c"],
        ),
        (
            "cronjobs",
            "batch/v1",
            "CronJob",
            json!({"schedule":"0 * * * *","timeZone":"Europe/London"}),
            vec!["Time Zone: Europe/London"],
        ),
        (
            "cronjobs",
            "batch/v1",
            "CronJob",
            json!({"schedule":"0 * * * *"}),
            vec!["Time Zone: <unset>"],
        ),
        (
            "statefulsets",
            "apps/v1",
            "StatefulSet",
            json!({"replicas":1,"selector":{"matchLabels":{"app":"demo"}},"serviceName":"headless","podManagementPolicy":"Parallel","persistentVolumeClaimRetentionPolicy":{"whenDeleted":"Delete","whenScaled":"Retain"},"template":{"spec":{"schedulingGroup":{"podGroupName":"workers"}}}}),
            vec![
                "Service Name: headless",
                "Pod Management Policy: Parallel",
                "WhenDeleted: Delete",
                "WhenScaled: Retain",
                "SchedulingGroup: PodGroupName: workers",
            ],
        ),
        (
            "pods",
            "v1",
            "Pod",
            json!({"schedulingGroup":{"podGroupName":"workers"},"containers":[{"name":"web","livenessProbe":{"exec":{"command":["true"]},"successThreshold":1,"failureThreshold":3}}]}),
            vec![
                "SchedulingGroup: PodGroupName: workers",
                "successThreshold=1 failureThreshold=3",
            ],
        ),
        (
            "widgets",
            "example.com/v1",
            "Widget",
            json!({"HTTPProxy":"proxy","IPAddress":"127.0.0.1"}),
            vec!["HTTPProxy: proxy", "IPAddress: 127.0.0.1"],
        ),
        (
            "pods",
            "v1",
            "Pod",
            json!({"resources":{},"containers":[{"name":"web","resources":{"requests":{"cpu":"1","memory":"1Gi"},"limits":{"cpu":"1","memory":"1Gi"}}}]}),
            vec!["QoS Class: Guaranteed"],
        ),
        (
            "pods",
            "v1",
            "Pod",
            json!({"containers":[{"name":"a","resources":{"requests":{"cpu":"2","memory":"1Gi"},"limits":{"cpu":"1","memory":"1Gi"}}},{"name":"b","resources":{"requests":{"cpu":"1","memory":"1Gi"},"limits":{"cpu":"2","memory":"1Gi"}}}]}),
            vec!["QoS Class: Burstable"],
        ),
    ];
    for (plural, version, kind, spec, expected) in cases {
        let fixture = json!({"plural":plural,"object":{"apiVersion":version,"kind":kind,"metadata":{"name":"demo","namespace":"default","uid":"demo-uid"},"spec":spec}});
        let rendered = render_fixture(&fixture).await;
        let output = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
        for expected in expected {
            assert!(
                output.contains(expected),
                "{kind}: missing {expected} in {output}"
            );
        }
    }
}

#[tokio::test]
async fn node_resource_slices_and_accounting() {
    for code in [200, 403, 404] {
        let node = json!({"apiVersion":"v1","kind":"Node","metadata":{"name":"demo","uid":"demo-uid"},"status":{"allocatable":{"cpu":"10","memory":"10Gi"}}});
        let resource = ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Node"));
        let selected: DynamicObject = serde_json::from_value(node.clone()).unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let requests = seen.clone();
        let client = kube::Client::new(
            tower::service_fn(move |request: http::Request<kube::client::Body>| {
                assert_eq!(request.method(), http::Method::GET);
                let uri = request.uri().to_string();
                requests.lock().unwrap().push(uri.clone());
                let (status, body) = match request.uri().path() {
                    "/api/v1/nodes/demo" => (200, node.clone()),
                    "/api/v1/pods" => (
                        200,
                        json!({"apiVersion":"v1","kind":"PodList","items":[{"metadata":{"name":"resizing","namespace":"default"},"spec":{"containers":[{"name":"a","resources":{"requests":{"cpu":"2"},"limits":{"cpu":"3"}}},{"name":"b","resources":{"requests":{"cpu":"1"},"limits":{"cpu":"4"}}}]},"status":{"containerStatuses":[{"name":"a","resources":{"requests":{"cpu":"1"},"limits":{"cpu":"8"}}},{"name":"b","resources":{"requests":{"cpu":"2"},"limits":{"cpu":"1"}}}]}}]}),
                    ),
                    "/apis/resource.k8s.io/v1/resourceslices" => {
                        let query: Vec<_> = request
                            .uri()
                            .query()
                            .unwrap_or_default()
                            .split('&')
                            .collect();
                        assert!(
                            query.contains(&"fieldSelector=spec.nodeName%3Ddemo"),
                            "{uri}"
                        );
                        if code == 200 {
                            let second_page = query.contains(&"continue=next");
                            let metadata = if second_page {
                                json!({})
                            } else {
                                json!({"continue":"next"})
                            };
                            (
                                200,
                                json!({"apiVersion":"resource.k8s.io/v1","kind":"ResourceSliceList","metadata":metadata,"items":[{"spec":{"driver":"gpu.example.com","pool":{"name":"local"},"devices":[{"name":"gpu"}]}}]}),
                            )
                        } else {
                            (
                                code,
                                json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Unavailable","message":"not available","code":code}),
                            )
                        }
                    }
                    path if path.ends_with("/events") => (
                        200,
                        json!({"apiVersion":"v1","kind":"EventList","items":[]}),
                    ),
                    path if path.ends_with("/leases/demo") => (
                        200,
                        json!({"apiVersion":"coordination.k8s.io/v1","kind":"Lease","metadata":{"name":"demo"},"spec":{}}),
                    ),
                    _ => panic!("unexpected request {uri}"),
                };
                async move {
                    Ok::<_, std::convert::Infallible>(
                        http::Response::builder()
                            .status(status)
                            .body(kube::client::Body::from(body.to_string().into_bytes()))
                            .unwrap(),
                    )
                }
            }),
            "default",
        );
        let snapshot = gather(client, &resource, &selected).await.unwrap();
        let rendered = snapshot.render(&RenderOptions::default()).unwrap();
        let output = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(output.contains("resizing 3 (30%) 9 (90%)"), "{output}");
        assert!(output.contains("cpu 3 (30%) 7 (70%)"), "{output}");
        assert_eq!(output.contains("Node-Local ResourceSlices:"), code == 200);
        assert!(!output.contains("Kube-Proxy Version:"));
        if code == 200 {
            assert!(output.contains("gpu.example.com local 2 2"), "{output}");
        }
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .filter(|uri| uri.starts_with("/apis/resource.k8s.io/v1/resourceslices"))
                .count(),
            if code == 200 { 2 } else { 1 }
        );
    }
}
