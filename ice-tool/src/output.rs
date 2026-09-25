//! Explicit, non-secret projections for the CLI's machine-readable output.
use std::io::{self, Write};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::model::{Cloud, CloudMachineCandidate, IceConfig, RuntimeCostEstimate};
use crate::providers::vast::VastOffer;
use crate::workload::InstanceWorkload;

pub(crate) fn document(command: &str, cloud: Cloud, result: Value) -> Value {
    json!({"schema_version": 1, "command": command, "cloud": cloud, "result": result})
}

pub(crate) fn write_document(mut writer: impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut writer, value).context("Failed to write JSON output")?;
    writeln!(writer).context("Failed to finish JSON output")?;
    writer.flush().context("Failed to flush JSON output")
}

pub(crate) fn emit(command: &str, cloud: Cloud, result: Value) -> Result<()> {
    write_document(io::stdout().lock(), &document(command, cloud, result))
}

pub(crate) fn cost(cost: &RuntimeCostEstimate) -> Value {
    json!({
        "currency": "USD", "hourly_usd": cost.hourly_usd,
        "requested_hours": cost.requested_hours, "scheduled_hours": cost.billed_hours,
        "estimated_total_usd": cost.total_usd,
    })
}

pub(crate) fn workload(workload: Option<&InstanceWorkload>) -> Value {
    match workload {
        Some(InstanceWorkload::Shell) => json!({"kind": "shell"}),
        Some(InstanceWorkload::Container(image)) => {
            json!({"kind": "container", "image": image.container_ref()})
        }
        Some(InstanceWorkload::Unpack(source)) => json!({"kind": "unpack", "source": source}),
        None => Value::Null,
    }
}

pub(crate) fn vast_offer(offer: &VastOffer) -> Value {
    json!({
        "offer_id": offer.id, "gpu_model": offer.gpu_name,
        "num_gpus": offer.num_gpus, "cpu_cores": offer.cpu_cores_effective,
        "ram_mb": offer.cpu_ram, "hourly_usd": offer.hourly_price(),
        "available_seconds": offer.duration, "location": offer.geolocation,
        "verification": offer.verification, "reliability": offer.reliability,
    })
}

pub(crate) fn machine(candidate: &CloudMachineCandidate) -> Value {
    json!({
        "machine_type": candidate.machine, "cpu_cores": candidate.vcpus,
        "ram_mb": candidate.ram_mb, "gpus": candidate.gpus,
        "hourly_usd": candidate.hourly_usd, "region": candidate.region,
        "zone": candidate.zone,
    })
}

pub(crate) fn allocated_disk_gb(config: &IceConfig, cloud: Cloud) -> Option<u32> {
    match cloud {
        Cloud::Gcp => Some(config.default.gcp.boot_disk_gb.unwrap_or(50)),
        // Without an override, AWS uses the chosen AMI's root volume size.
        Cloud::Aws => config.default.aws.root_disk_gb,
        Cloud::VastAi => Some(crate::support::VAST_DEFAULT_DISK_GB as u32),
        Cloud::Local => None,
    }
}

pub(crate) fn connection(cloud: Cloud, instance: Value, command: &str) -> Result<()> {
    emit(
        "shell",
        cloud,
        json!({"instance": instance, "connect_command": command}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_one_json_document_with_typed_values_and_newline() {
        let expected = document(
            "list",
            Cloud::VastAi,
            json!({"instances": [], "cost": null}),
        );
        let mut bytes = Vec::new();
        write_document(&mut bytes, &expected).unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), expected);
        assert_eq!(expected["schema_version"], 1);
        assert!(
            expected["result"]["instances"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(expected["result"]["cost"].is_null());
    }

    #[test]
    fn output_reports_write_failures() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("broken pipe"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(write_document(Broken, &json!({})).is_err());
    }

    #[test]
    fn unknown_aws_disk_size_is_not_reported_as_an_invented_default() {
        let mut config = IceConfig::default();
        assert_eq!(allocated_disk_gb(&config, Cloud::Aws), None);
        config.default.aws.root_disk_gb = Some(80);
        assert_eq!(allocated_disk_gb(&config, Cloud::Aws), Some(80));
        assert_eq!(allocated_disk_gb(&config, Cloud::Gcp), Some(50));
    }

    #[test]
    fn vast_quote_and_output_do_not_expose_unrelated_provider_fields() {
        let offer: VastOffer = serde_json::from_value(json!({
            "id": 123, "gpu_name": "RTX 5060 Ti", "num_gpus": 1,
            "dph_total": 0.12, "search": {"totalHour": 0.15},
            "image_login": "secret-registry-token", "api_key": "secret-api-token"
        }))
        .unwrap();
        let value = vast_offer(&offer);
        assert_eq!(value["offer_id"], 123);
        assert_eq!(value["num_gpus"], 1);
        assert_eq!(offer.quoted_total_hourly_price(), Some(0.15));
        assert!(!value.to_string().contains("secret-"));
        let incomplete: VastOffer = serde_json::from_value(json!({"id": 456})).unwrap();
        assert_eq!(incomplete.quoted_total_hourly_price(), None);
    }
}
