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
    let (ar, object) = selection("", "Pod");
    let mut fresh = object.clone();
    fresh.metadata.annotations =
        Some([("kubernetes.io/config.mirror".into(), "mirror-uid".into())].into());
    let body = serde_json::to_value(&fresh).unwrap();
    let (client, requests) = client(move |uri| {
        if uri.path().ends_with("/events") {
            let query = uri.query().unwrap();
            assert!(query.contains("involvedObject.uid%3Dmirror-uid"), "{uri}");
            assert!(!query.contains("involvedObject.kind"));
            assert!(!query.contains("selected-uid"));
            (
                200,
                json!({"apiVersion":"v1","kind":"EventList","items":[]}),
            )
        } else {
            assert!(uri.path().ends_with("/pods/demo"));
            (200, body.clone())
        }
    });
    let description = gather(client, &ar, &object).await.unwrap();
    let output = description.render(&RenderOptions::default()).unwrap();
    assert!(output.contains("Events:"));
    assert_eq!(
        description.object().metadata.annotations,
        fresh.metadata.annotations
    );
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].path().ends_with("/pods/demo"));
}

#[tokio::test]
async fn failed_pod_read_keeps_the_selection_and_surviving_events() {
    let (ar, object) = selection("", "Pod");
    let (client, requests) = client(move |uri| {
        if uri.path().ends_with("/events") {
            let query = uri.query().unwrap();
            assert!(query.contains("involvedObject.name%3Ddemo"), "{uri}");
            assert!(!query.contains("involvedObject.uid"));
            assert!(!query.contains("involvedObject.kind"));
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
    assert_eq!(requests.lock().unwrap().len(), 2);
}
