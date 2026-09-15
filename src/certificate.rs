// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 sofka contributors.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use super::*;
use base64::Engine;
use k8s_openapi::jiff::tz::TimeZone;
use x509_parser::{extensions::GeneralName, prelude::*};

pub(super) fn render(
    object: &DynamicObject,
    events: Option<&[Event]>,
    now: Timestamp,
    zone: &TimeZone,
) -> Result<String, String> {
    let spec = &object.data["spec"];
    let text = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_owned();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text(&spec["request"]))
        .map_err(|_| "Error parsing CSR: invalid request encoding")?;
    let (_, pem) = x509_parser::pem::parse_x509_pem(&bytes)
        .map_err(|_| "Error parsing CSR: PEM block type must be CERTIFICATE REQUEST")?;
    if pem.label != "CERTIFICATE REQUEST" {
        return Err("Error parsing CSR: PEM block type must be CERTIFICATE REQUEST".into());
    }
    let (rest, csr) = X509CertificationRequest::from_der(&pem.contents)
        .map_err(|e| format!("Error parsing CSR: {e}"))?;
    if !rest.is_empty() {
        return Err("Error parsing CSR: trailing data".into());
    }
    let inline = |map: &Option<BTreeMap<String, String>>| {
        map.as_ref().filter(|m| !m.is_empty()).map_or_else(
            || "<none>".into(),
            |m| {
                m.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(",")
            },
        )
    };
    let mut out = format!(
        "Name:\t{}\nLabels:\t{}\nAnnotations:\t{}\nCreationTimestamp:\t{}\nRequesting User:\t{}\n",
        object.metadata.name.as_deref().unwrap_or_default(),
        inline(&object.metadata.labels),
        inline(&object.metadata.annotations),
        containers::timestamp(
            &serde_json::to_value(&object.metadata.creation_timestamp).unwrap_or_default(),
            zone
        ),
        text(&spec["username"])
    );
    if !text(&spec["signerName"]).is_empty() {
        writeln!(out, "Signer:\t{}", text(&spec["signerName"])).unwrap();
    }
    if let Some(seconds) = spec["expirationSeconds"].as_i64() {
        writeln!(
            out,
            "Requested Duration:\t{}",
            human_duration(i128::from(seconds))
        )
        .unwrap();
    }
    let conditions = object.data["status"]["conditions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let has = |kind: &str| conditions.iter().any(|c| c["type"].as_str() == Some(kind));
    write!(
        out,
        "Status:\t{}",
        if has("Denied") {
            "Denied"
        } else if has("Approved") {
            "Approved"
        } else {
            "Pending"
        }
    )
    .unwrap();
    if has("Failed") {
        out.push_str(",Failed");
    }
    if !text(&object.data["status"]["certificate"]).is_empty() {
        out.push_str(",Issued");
    }
    out.push_str("\nSubject:\n");
    let mut subject: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for attr in csr.certification_request_info.subject.iter_attributes() {
        let value = if let Ok(value) = attr.as_str() {
            value.to_owned()
        } else if attr.attr_value().tag().0 == 30 {
            let bytes = attr.attr_value().data;
            if bytes.len() % 2 != 0 {
                return Err("Error parsing CSR: invalid BMPString".into());
            }
            String::from_utf16(
                &bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect::<Vec<_>>(),
            )
            .map_err(|_| "Error parsing CSR: invalid BMPString")?
        } else {
            return Err("Error parsing CSR: unsupported subject string".into());
        };
        subject
            .entry(attr.attr_type().to_id_string())
            .or_default()
            .push(value);
    }
    for (oid, label) in [("2.5.4.3", "Common Name"), ("2.5.4.5", "Serial Number")] {
        writeln!(
            out,
            "\t{label}:\t{}",
            subject
                .get(oid)
                .and_then(|v| v.last())
                .map(String::as_str)
                .unwrap_or_default()
        )
        .unwrap();
    }
    for (oid, label) in [
        ("2.5.4.10", "Organization"),
        ("2.5.4.11", "Organizational Unit"),
        ("2.5.4.6", "Country"),
        ("2.5.4.7", "Locality"),
        ("2.5.4.8", "Province"),
        ("2.5.4.9", "StreetAddress"),
        ("2.5.4.17", "PostalCode"),
    ] {
        if let Some(values) = subject.get(oid) {
            list(&mut out, label, values);
        }
    }
    let (mut dns, mut emails, mut uris, mut ips) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for extension in csr.requested_extensions().into_iter().flatten() {
        if let ParsedExtension::SubjectAlternativeName(san) = extension {
            for name in &san.general_names {
                match name {
                    GeneralName::DNSName(v) => dns.push((*v).into()),
                    GeneralName::RFC822Name(v) => emails.push((*v).into()),
                    GeneralName::URI(v) => uris.push((*v).into()),
                    GeneralName::IPAddress(bytes) => {
                        let ip = match bytes.len() {
                            4 => std::net::IpAddr::V4(std::net::Ipv4Addr::from(
                                <[u8; 4]>::try_from(*bytes).unwrap(),
                            )),
                            16 => std::net::IpAddr::V6(std::net::Ipv6Addr::from(
                                <[u8; 16]>::try_from(*bytes).unwrap(),
                            )),
                            _ => return Err("Error parsing CSR: invalid IP address".into()),
                        };
                        ips.push(match ip {
                            std::net::IpAddr::V6(v) => v
                                .to_ipv4_mapped()
                                .map_or_else(|| v.to_string(), |v| v.to_string()),
                            _ => ip.to_string(),
                        });
                    }
                    _ => {}
                }
            }
        }
    }
    if !dns.is_empty() || !emails.is_empty() || !uris.is_empty() || !ips.is_empty() {
        out.push_str("Subject Alternative Names:\n");
        for (label, values) in [
            ("DNS Names", dns),
            ("Email Addresses", emails),
            ("URIs", uris),
            ("IP Addresses", ips),
        ] {
            list(&mut out, label, &values);
        }
    }
    Ok(with_events(out, events, now))
}

fn list(out: &mut String, label: &str, values: &[String]) {
    if !values.is_empty() {
        writeln!(out, "\t{label}:\t{}", values.join("\n\t\t")).unwrap();
    }
}
