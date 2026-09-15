use std::sync::{Arc, Mutex};

use deskribe::{RenderOptions, gather};
use kube::{
    Client,
    api::{ApiResource, DynamicObject},
    core::GroupVersionKind,
};
use serde_json::{Value, json};

fn client(
    respond: impl Fn(&str) -> Result<(u16, Value), std::io::Error> + Send + Sync + 'static,
) -> (Client, Arc<Mutex<Vec<String>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let client = Client::new(
        tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(request.method(), http::Method::GET);
            let path = request.uri().path();
            seen.lock().unwrap().push(path.to_owned());
            let response = if path.ends_with("/events") {
                Ok((
                    200,
                    json!({"apiVersion":"v1","kind":"EventList","items":[]}),
                ))
            } else {
                respond(path)
            };
            async move {
                let (status, body) = response?;
                Ok::<_, std::io::Error>(
                    http::Response::builder()
                        .status(status)
                        .body(kube::client::Body::from(body.to_string().into_bytes()))
                        .unwrap(),
                )
            }
        }),
        "default",
    );
    (client, requests)
}

fn resource(group: &str, version: &str, kind: &str) -> ApiResource {
    ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, kind))
}

fn selected(ar: &ApiResource) -> DynamicObject {
    let object = DynamicObject::new("demo", ar);
    if ar.kind == "HorizontalPodAutoscaler" || ar.kind == "Ingress" {
        object.within("default")
    } else {
        object
    }
}

fn object_paths(requests: &Mutex<Vec<String>>) -> Vec<String> {
    requests
        .lock()
        .unwrap()
        .iter()
        .filter(|path| !path.ends_with("/events"))
        .cloned()
        .collect()
}

fn failure(code: u16, reason: &str, message: &str) -> (u16, Value) {
    (
        code,
        json!({"apiVersion":"v1","kind":"Status","status":"Failure",
            "code":code,"reason":reason,"message":message}),
    )
}

const MISSING_ENDPOINT: &str = "the server could not find the requested resource";

fn fallback_cases() -> Vec<(ApiResource, &'static str, &'static str, &'static str)> {
    vec![
        (
            resource("autoscaling", "v2", "HorizontalPodAutoscaler"),
            "/apis/autoscaling/v2/namespaces/default/horizontalpodautoscalers/demo",
            "/apis/autoscaling/v1/namespaces/default/horizontalpodautoscalers/demo",
            "v1",
        ),
        (
            resource("networking.k8s.io", "v1", "ServiceCIDR"),
            "/apis/networking.k8s.io/v1/servicecidrs/demo",
            "/apis/networking.k8s.io/v1beta1/servicecidrs/demo",
            "v1beta1",
        ),
        (
            resource("networking.k8s.io", "v1", "IPAddress"),
            "/apis/networking.k8s.io/v1/ipaddresses/demo",
            "/apis/networking.k8s.io/v1beta1/ipaddresses/demo",
            "v1beta1",
        ),
    ]
}

#[tokio::test]
async fn fresh_get_preserves_discovered_versions_and_plural() {
    let mut custom_plural = resource("storage.k8s.io", "v1beta1", "VolumeAttributesClass");
    custom_plural.plural = "discoveredclasses".into();
    let cases = [
        (
            resource("autoscaling", "v1", "HorizontalPodAutoscaler"),
            "/apis/autoscaling/v1/namespaces/default/horizontalpodautoscalers/demo",
        ),
        (
            resource("autoscaling", "v2", "HorizontalPodAutoscaler"),
            "/apis/autoscaling/v2/namespaces/default/horizontalpodautoscalers/demo",
        ),
        (
            resource("networking.k8s.io", "v1beta1", "ServiceCIDR"),
            "/apis/networking.k8s.io/v1beta1/servicecidrs/demo",
        ),
        (
            resource("networking.k8s.io", "v1beta1", "IPAddress"),
            "/apis/networking.k8s.io/v1beta1/ipaddresses/demo",
        ),
        (
            custom_plural,
            "/apis/storage.k8s.io/v1beta1/discoveredclasses/demo",
        ),
    ];
    for (ar, expected) in cases {
        let selection = selected(&ar);
        let body = serde_json::to_value(&selection).unwrap();
        let (client, requests) = client(move |path| {
            assert_eq!(path, expected);
            Ok((200, body.clone()))
        });
        let description = gather(client, &ar, &selection).await.unwrap();
        assert_eq!(description.object().types, selection.types);
        description.render(&RenderOptions::default()).unwrap();
        assert_eq!(object_paths(&requests), [expected]);
    }
}

#[tokio::test]
async fn older_ingress_uses_selected_endpoint_and_renders_both_backend_forms() {
    for group in ["extensions", "networking.k8s.io"] {
        let ar = resource(group, "v1beta1", "Ingress");
        let selection = selected(&ar);
        let expected = format!("/apis/{group}/v1beta1/namespaces/default/ingresses/demo");
        let object_path = expected.clone();
        let version = ar.api_version.clone();
        let (client, requests) = client(move |path| {
            let body = if path == object_path {
                json!({"apiVersion":version,"kind":"Ingress",
                    "metadata":{"name":"demo","namespace":"default"},
                    "spec":{"backend":{"serviceName":"web","servicePort":80},
                        "rules":[{"http":{"paths":[{"path":"/","backend":{
                            "serviceName":"web","servicePort":"http"}}]}}]}})
            } else if path == "/api/v1/namespaces/default/services/web" {
                json!({"apiVersion":"v1","kind":"Service",
                    "metadata":{"name":"web","namespace":"default"},
                    "spec":{"ports":[{"name":"http","port":80}]}})
            } else {
                assert_eq!(
                    path,
                    "/apis/discovery.k8s.io/v1/namespaces/default/endpointslices"
                );
                json!({"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSliceList",
                    "items":[{"apiVersion":"discovery.k8s.io/v1","kind":"EndpointSlice",
                        "metadata":{"name":"web"},"addressType":"IPv4",
                        "ports":[{"name":"http","port":8080}],
                        "endpoints":[{"addresses":["10.0.0.1"]}]}]})
            };
            Ok((200, body))
        });
        let description = gather(client, &ar, &selection).await.unwrap();
        let output = description.render(&RenderOptions::default()).unwrap();
        assert!(output.contains("web:80 (10.0.0.1:8080)"), "{output}");
        assert!(output.contains("web:http (10.0.0.1:8080)"), "{output}");
        let paths = object_paths(&requests);
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0], expected);
    }
}

#[tokio::test]
async fn missing_endpoint_allows_one_targeted_version_fallback() {
    for (ar, primary, fallback, version) in fallback_cases() {
        let selection = selected(&ar);
        let mut fresh = selection.clone();
        fresh.types.as_mut().unwrap().api_version = format!("{}/{version}", ar.group);
        let body = serde_json::to_value(&fresh).unwrap();
        let (client, requests) = client(move |path| {
            if path == primary {
                Ok(failure(404, "NotFound", MISSING_ENDPOINT))
            } else {
                assert_eq!(path, fallback);
                Ok((200, body.clone()))
            }
        });
        let description = gather(client, &ar, &selection).await.unwrap();
        assert_eq!(description.object().types, fresh.types);
        description.render(&RenderOptions::default()).unwrap();
        assert_eq!(object_paths(&requests), [primary, fallback]);
    }
}

#[tokio::test]
async fn other_get_failures_do_not_trigger_version_fallback() {
    for (ar, primary, _, _) in fallback_cases() {
        for (code, reason, message) in [
            (401, "Unauthorized", "fixture authentication failed"),
            (403, "Forbidden", "fixture access denied"),
            (404, "NotFound", "fixture object not found"),
            (404, "NotFound", "namespaces \"default\" not found"),
            (404, "NotFound", "unknown proxy response"),
            (429, "TooManyRequests", "fixture rate limit"),
            (500, "InternalError", "fixture server failed"),
            (503, "ServiceUnavailable", "fixture unavailable"),
            (504, "Timeout", "fixture request timed out"),
            (0, "", "fixture transport failed"),
        ] {
            let (client, requests) = client(move |path| {
                assert_eq!(path, primary);
                if code == 0 {
                    Err(std::io::Error::other(message))
                } else {
                    Ok(failure(code, reason, message))
                }
            });
            let error = gather(client, &ar, &selected(&ar)).await.err().unwrap();
            assert!(error.contains(message), "{error}");
            assert_eq!(object_paths(&requests), [primary]);
        }
    }
}

#[tokio::test]
async fn failed_fallback_preserves_the_primary_error() {
    for (ar, primary, fallback, _) in fallback_cases() {
        let (client, requests) = client(move |path| {
            if path == primary {
                Ok(failure(404, "NotFound", MISSING_ENDPOINT))
            } else {
                assert_eq!(path, fallback);
                Ok(failure(403, "Forbidden", "fixture fallback denied"))
            }
        });
        let error = gather(client, &ar, &selected(&ar)).await.err().unwrap();
        assert!(error.contains(MISSING_ENDPOINT), "{error}");
        assert!(!error.contains("fixture fallback denied"), "{error}");
        assert_eq!(object_paths(&requests), [primary, fallback]);
    }
}
