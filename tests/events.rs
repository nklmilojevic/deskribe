use std::sync::{Arc, Mutex};

use deskribe::{RenderOptions, gather};
use kube::{
    Client,
    api::{ApiResource, DynamicObject},
    core::GroupVersionKind,
};
use serde_json::{Value, json};

fn selection(group: &str, kind: &str) -> (ApiResource, DynamicObject) {
    let ar = ApiResource::from_gvk(&GroupVersionKind::gvk(group, "v1", kind));
    let mut object = DynamicObject::new("demo", &ar).within("default");
    object.metadata.uid = Some("selected-uid".into());
    object.data = json!({"spec": {}});
    (ar, object)
}

fn client(
    respond: impl Fn(&http::Uri) -> (u16, Value) + Send + Sync + 'static,
) -> (Client, Arc<Mutex<Vec<http::Uri>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let client = Client::new(
        tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(request.method(), http::Method::GET);
            seen.lock().unwrap().push(request.uri().clone());
            let (code, body) = respond(request.uri());
            async move {
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(code)
                        .body(kube::client::Body::from(body.to_string().into_bytes()))
                        .unwrap(),
                )
            }
        }),
        "default",
    );
    (client, requests)
}

fn failure(code: u16) -> Value {
    json!({"apiVersion":"v1","kind":"Status","status":"Failure",
        "code":code,"reason":"Forbidden","message":"fixture read failed"})
}

#[tokio::test]
async fn resources_without_events_only_read_the_object() {
    for (group, kind) in [
        ("", "Secret"),
        ("", "ResourceQuota"),
        ("", "LimitRange"),
        ("networking.k8s.io", "NetworkPolicy"),
        ("rbac.authorization.k8s.io", "Role"),
        ("rbac.authorization.k8s.io", "ClusterRole"),
        ("rbac.authorization.k8s.io", "RoleBinding"),
        ("rbac.authorization.k8s.io", "ClusterRoleBinding"),
    ] {
        let (ar, object) = selection(group, kind);
        let body = serde_json::to_value(&object).unwrap();
        let (client, requests) = client(move |uri| {
            assert!(uri.path().ends_with("/demo"), "unexpected read: {uri}");
            (200, body.clone())
        });
        let description = gather(client, &ar, &object).await.unwrap();
        let output = description.render(&RenderOptions::default()).unwrap();
        assert!(!output.contains("Events:"), "{kind}: {output}");
        assert_eq!(requests.lock().unwrap().len(), 1, "{kind}");
    }
}

#[tokio::test]
async fn optional_event_errors_do_not_fail_rendering() {
    for uid in [None, Some("selected-uid".into())] {
        let (ar, mut object) = selection("example.com", "Widget");
        object.metadata.uid = uid;
        let body = serde_json::to_value(&object).unwrap();
        let (client, requests) = client(move |uri| {
            if uri.path().ends_with("/events") {
                (403, failure(403))
            } else {
                assert!(uri.path().ends_with("/widgets/demo"));
                (200, body.clone())
            }
        });
        let description = gather(client, &ar, &object).await.unwrap();
        let output = description.render(&RenderOptions::default()).unwrap();
        assert!(output.contains("Widget"));
        assert!(!output.contains("Events:"));
        assert_eq!(requests.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn pod_events_use_the_fresh_mirror_uid() {
    // The selection predates the mirror annotation, so the read issued against
    // it is scoped by the wrong uid. The fresh object disagrees, and the events
    // that reach the description must be the ones read again against it.
    let (ar, object) = selection("", "Pod");
    let mut fresh = object.clone();
    fresh.metadata.annotations =
        Some([("kubernetes.io/config.mirror".into(), "mirror-uid".into())].into());
    let body = serde_json::to_value(&fresh).unwrap();
    let (client, requests) = client(move |uri| {
        if uri.path().ends_with("/events") {
            let query = uri.query().unwrap();
            assert!(!query.contains("involvedObject.kind"));
            let reason = if query.contains("involvedObject.uid%3Dmirror-uid") {
                "FromMirror"
            } else {
                assert!(query.contains("involvedObject.uid%3Dselected-uid"), "{uri}");
                "FromStaleSelection"
            };
            (
                200,
                json!({"apiVersion":"v1","kind":"EventList","items":[{
                    "involvedObject":{"name":"demo"},"reason":reason,"message":"fixture event"
                }]}),
            )
        } else {
            assert!(uri.path().ends_with("/pods/demo"));
            (200, body.clone())
        }
    });
    let description = gather(client, &ar, &object).await.unwrap();
    let output = description.render(&RenderOptions::default()).unwrap();
    assert!(output.contains("Events:"));
    assert!(output.contains("FromMirror"), "{output}");
    assert!(!output.contains("FromStaleSelection"), "{output}");
    assert_eq!(
        description.object().metadata.annotations,
        fresh.metadata.annotations
    );
    let requests = requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|uri| uri.path().ends_with("/pods/demo")),
        "the fresh object is always read"
    );
}

#[tokio::test]
async fn a_mirror_pod_selection_reads_events_once() {
    // The ordinary case: the selection already carries the annotation, so the
    // read issued against it is the one that is kept.
    let (ar, mut object) = selection("", "Pod");
    object.metadata.annotations =
        Some([("kubernetes.io/config.mirror".into(), "mirror-uid".into())].into());
    let body = serde_json::to_value(&object).unwrap();
    let (client, requests) = client(move |uri| {
        if uri.path().ends_with("/events") {
            assert!(
                uri.query()
                    .unwrap()
                    .contains("involvedObject.uid%3Dmirror-uid"),
                "{uri}"
            );
            (
                200,
                json!({"apiVersion":"v1","kind":"EventList","items":[]}),
            )
        } else {
            assert!(uri.path().ends_with("/pods/demo"));
            (200, body.clone())
        }
    });
    gather(client, &ar, &object).await.unwrap();
    assert_eq!(requests.lock().unwrap().len(), 2, "no read is repeated");
}

#[tokio::test]
async fn pod_spec_changes_do_not_repeat_the_same_event_read() {
    let (ar, mut object) = selection("", "Pod");
    object.data["spec"] = json!({"containers":[{"name":"old"}]});
    let mut fresh = object.clone();
    fresh.data["spec"] = json!({"containers":[{"name":"fresh"}]});
    let body = serde_json::to_value(&fresh).unwrap();
    let (client, requests) = client(move |uri| {
        if uri.path().ends_with("/events") {
            (
                200,
                json!({"apiVersion":"v1","kind":"EventList","items":[]}),
            )
        } else {
            (200, body.clone())
        }
    });
    let description = gather(client, &ar, &object).await.unwrap();
    assert_eq!(description.object().data["spec"], fresh.data["spec"]);
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn controller_reads_repeat_only_when_their_selector_changes() {
    for selector_changed in [false, true] {
        let (ar, mut selected) = selection("apps", "Deployment");
        selected.data["spec"] = json!({"replicas":1,"selector":{"matchLabels":{"app":"old"}}});
        let mut fresh = selected.clone();
        fresh.data["spec"]["replicas"] = json!(2);
        if selector_changed {
            fresh.data["spec"]["selector"]["matchLabels"]["app"] = json!("fresh");
        }
        let body = serde_json::to_value(&fresh).unwrap();
        let (client, requests) = client(move |uri| {
            if uri.path().ends_with("/deployments/demo") {
                (200, body.clone())
            } else if uri.path().ends_with("/replicasets") {
                (
                    200,
                    json!({"apiVersion":"apps/v1","kind":"ReplicaSetList","items":[]}),
                )
            } else {
                assert!(uri.path().ends_with("/events"), "{uri}");
                (
                    200,
                    json!({"apiVersion":"v1","kind":"EventList","items":[]}),
                )
            }
        });
        gather(client, &ar, &selected).await.unwrap();
        assert_eq!(
            requests.lock().unwrap().len(),
            if selector_changed { 5 } else { 3 }
        );
    }
}

#[tokio::test]
async fn failed_get_does_not_wait_for_speculative_reads() {
    let (ar, object) = selection("example.com", "Widget");
    let client = Client::new(
        tower::service_fn(move |request: http::Request<kube::client::Body>| {
            let events = request.uri().path().ends_with("/events");
            async move {
                let (status, body) = if events {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    (
                        200,
                        json!({"apiVersion":"v1","kind":"EventList","items":[]}),
                    )
                } else {
                    (403, failure(403))
                };
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
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        gather(client, &ar, &object),
    )
    .await
    .expect("the GET error should cancel the slow speculative event read");
    let error = result.err().expect("the GET must fail");
    assert!(error.contains("describe GET failed"));
}

#[tokio::test]
async fn failed_pod_read_keeps_the_selection_and_surviving_events() {
    // A failed GET cancels its uid-scoped speculative read. The fallback reads
    // by name, so a replaced pod still shows the events recorded against it.
    let (ar, object) = selection("", "Pod");
    let (client, requests) = client(move |uri| {
        if uri.path().ends_with("/events") {
            let query = uri.query().unwrap();
            assert!(query.contains("involvedObject.name%3Ddemo"), "{uri}");
            assert!(!query.contains("involvedObject.kind"));
            if query.contains("involvedObject.uid") {
                // The read against the selection; the GET it raced has failed.
                return (
                    200,
                    json!({"apiVersion":"v1","kind":"EventList","items":[]}),
                );
            }
            (
                200,
                json!({"apiVersion":"v1","kind":"EventList","items":[{
                    "involvedObject":{"name":"demo"},"reason":"Scheduled","message":"fixture event"
                }]}),
            )
        } else {
            assert!(uri.path().ends_with("/pods/demo"));
            (403, failure(403))
        }
    });
    let description = gather(client, &ar, &object).await.unwrap();
    assert_eq!(description.object().metadata, object.metadata);
    assert_eq!(description.object().data, object.data);
    let output = description.render(&RenderOptions::default()).unwrap();
    assert!(output.contains("fixture read failed"));
    assert!(output.contains("but found events"));
    assert!(output.contains("fixture event"));
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|uri| uri.path().ends_with("/pods/demo"))
    );
}
