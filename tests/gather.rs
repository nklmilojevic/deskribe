use std::sync::{Arc, Mutex};

use deskribe::{RenderOptions, gather};
use kube::{
    Client,
    api::{ApiResource, DynamicObject},
    core::GroupVersionKind,
};
use serde_json::json;

fn fixture(
    uid: &str,
    events_status: u16,
) -> (Client, Arc<Mutex<Vec<String>>>, ApiResource, DynamicObject) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let uid = uid.to_owned();
    let client = Client::new(
        tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(request.method(), http::Method::GET);
            let path = request.uri().path().to_owned();
            seen.lock().unwrap().push(path.clone());
            let (status, body) = if path.ends_with("/configmaps/demo") {
                (
                    200,
                    json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"demo","namespace":"default","uid":uid},"data":{"key":"fresh-value"}}),
                )
            } else {
                assert!(path.ends_with("/events"), "{path}");
                if events_status == 200 {
                    (
                        200,
                        json!({"apiVersion":"v1","kind":"EventList","items":[]}),
                    )
                } else {
                    (
                        events_status,
                        json!({"apiVersion":"v1","kind":"Status","status":"Failure","code":events_status,"reason":"Forbidden","message":"fixture events forbidden"}),
                    )
                }
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
    let resource = ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "ConfigMap"));
    let mut selected = DynamicObject::new("demo", &resource);
    selected.metadata.namespace = Some("default".into());
    selected.metadata.uid = Some("selected-uid".into());
    (client, requests, resource, selected)
}

#[tokio::test]
async fn caller_owned_client_gathers_once_and_rendering_is_offline() {
    let (client, requests, resource, selected) = fixture("selected-uid", 200);
    let description = gather(client.clone(), &resource, &selected).await.unwrap();
    drop(client);
    assert_eq!(requests.lock().unwrap().len(), 2);
    assert_eq!(description.object().data["data"]["key"], "fresh-value");
    let options = RenderOptions {
        now: "2026-01-01T00:00:00Z".parse().unwrap(),
        timezone: k8s_openapi::jiff::tz::TimeZone::UTC,
    };
    let first = description.render(&options).unwrap();
    assert_eq!(first, description.render(&options).unwrap());
    assert!(first.contains("fresh-value"));
    assert_eq!(requests.lock().unwrap().len(), 2);
    assert_eq!(
        description.into_object().metadata.uid.as_deref(),
        Some("selected-uid")
    );
}

#[tokio::test]
async fn replacement_is_rejected_before_rendering() {
    let (client, _, resource, selected) = fixture("replacement-uid", 200);
    let error = gather(client, &resource, &selected)
        .await
        .err()
        .expect("changed UID must fail");
    assert!(error.contains("resource was replaced"));
}

#[tokio::test]
async fn required_configmap_events_failure_is_not_silently_ignored() {
    let (client, _, resource, selected) = fixture("selected-uid", 403);
    let error = gather(client, &resource, &selected)
        .await
        .err()
        .expect("required events must fail");
    assert!(error.contains("fixture events forbidden"));
}
