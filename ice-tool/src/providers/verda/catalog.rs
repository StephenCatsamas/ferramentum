use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::client::Client;
use crate::model::{CreateSearchRequirements, VerdaDefaults};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct Machine {
    pub instance_type: String,
    pub model: String,
    pub name: String,
    pub manufacturer: String,
    pub currency: String,
    pub price_per_hour: Value,
    pub cpu: Cpu,
    pub gpu: Gpu,
    pub gpu_memory: Memory,
    pub memory: Memory,
    pub supported_os: Vec<String>,
    #[serde(default)]
    pub description: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct Cpu {
    pub number_of_cores: u32,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct Gpu {
    pub number_of_gpus: u32,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct Memory {
    pub size_in_gigabytes: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct Image {
    pub id: String,
    pub image_type: String,
    pub name: String,
    pub category: String,
    pub is_cluster: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct Availability {
    pub location_code: String,
    pub availabilities: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct VolumeType {
    #[serde(rename = "type")]
    pub kind: String,
    pub price: VolumePrice,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct VolumePrice {
    pub price_per_month_per_gb: Value,
    pub currency: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Catalog {
    pub machines: Vec<Machine>,
    pub images: Vec<Image>,
    pub availability: Vec<Availability>,
    pub volume_types: Vec<VolumeType>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Offer {
    pub machine: Machine,
    pub location: String,
    pub image: Image,
    pub allocated_disk_gb: u32,
    pub compute_hourly_usd: f64,
    pub storage_hourly_usd: f64,
    pub hourly_usd: f64,
    pub cost_scope: &'static str,
}

fn price(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|p| p.is_finite() && *p > 0.0)
}

fn confidential(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("confidential") || lower.split(['-', '.', '_', ' ']).any(|part| part == "cc")
}

pub(super) fn image_version(image: &Image) -> Option<(u32, u32, u32, u32)> {
    if image.is_cluster
        || confidential(&image.category)
        || confidential(&image.image_type)
        || confidential(&image.name)
    {
        return None;
    }
    let (ubuntu, cuda) = image
        .image_type
        .strip_prefix("ubuntu-")?
        .split_once("-cuda-")?;
    let mut ubuntu = ubuntu.split('.');
    let ubuntu_major = ubuntu.next()?.parse::<u32>().ok()?;
    let ubuntu_minor = ubuntu.next()?.parse::<u32>().ok()?;
    let mut cuda = cuda.split('-').next()?.split('.');
    let major = cuda.next()?.parse::<u32>().ok()?;
    let minor = cuda.next()?.parse::<u32>().ok()?;
    (matches!(ubuntu_major, 24 | 26) && (major, minor) >= (12, 9)).then_some((
        ubuntu_major,
        ubuntu_minor,
        major,
        minor,
    ))
}

impl Catalog {
    pub(super) fn load(client: &Client) -> Result<Self> {
        Ok(Self {
            machines: client.list("/instance-types?currency=usd")?,
            images: client.list("/images")?,
            availability: client.list("/instance-availability?is_spot=false")?,
            volume_types: client.list("/volume-types")?,
        })
    }

    pub(super) fn select(
        &self,
        requirements: &CreateSearchRequirements,
        defaults: &VerdaDefaults,
        pinned_machine: Option<&str>,
    ) -> Result<Offer> {
        let disk = requirements.disk_gb.unwrap_or(100);
        let nvme = self
            .volume_types
            .iter()
            .find(|v| v.kind == "NVMe" && v.price.currency.eq_ignore_ascii_case("usd"))
            .context(
                "Verda has no USD NVMe storage quote; cannot enforce the full price ceiling",
            )?;
        let monthly = price(&nvme.price.price_per_month_per_gb)
            .context("Missing or invalid Verda NVMe price")?;
        // Same conversion and rounding as Verda CLI's VolumeHourlyPrice.
        let storage_hourly_usd = (monthly * f64::from(disk) / 730.0 * 10_000.0).ceil() / 10_000.0;
        let mut offers = Vec::new();
        for machine in &self.machines {
            let Some(compute_hourly_usd) = price(&machine.price_per_hour) else {
                continue;
            };
            let gpu_count = machine.gpu.number_of_gpus;
            let hourly_usd = compute_hourly_usd + storage_hourly_usd;
            if !machine.manufacturer.eq_ignore_ascii_case("nvidia")
                || gpu_count == 0
                || !machine.currency.eq_ignore_ascii_case("usd")
                || !machine.instance_type.ends_with('V')
                || confidential(&machine.instance_type)
                || confidential(&machine.description)
                || confidential(&machine.name)
                || pinned_machine.is_some_and(|name| name != machine.instance_type)
                || machine.cpu.number_of_cores < requirements.min_cpus
                || !machine.memory.size_in_gigabytes.is_finite()
                || machine.memory.size_in_gigabytes < requirements.min_ram_gb
                || requirements
                    .gpu_count
                    .is_some_and(|count| count != gpu_count)
                || requirements.min_gpu_memory_gb.is_some_and(|size| {
                    !machine.gpu_memory.size_in_gigabytes.is_finite()
                        || machine.gpu_memory.size_in_gigabytes < size
                })
                || (!requirements.allowed_gpus.is_empty()
                    && !requirements.allowed_gpus.iter().any(|gpu| {
                        gpu.eq_ignore_ascii_case(&machine.model)
                            || gpu.eq_ignore_ascii_case(&machine.name)
                    }))
                || !hourly_usd.is_finite()
                || hourly_usd > requirements.max_price_per_hr
            {
                continue;
            }
            let image =
                self.images
                    .iter()
                    .filter(|image| {
                        image_version(image).is_some()
                            && machine
                                .supported_os
                                .iter()
                                .any(|os| os == &image.image_type || os == &image.id)
                            && defaults.image.as_ref().is_none_or(|wanted| {
                                wanted == &image.image_type || wanted == &image.id
                            })
                    })
                    .max_by_key(|image| (image_version(image), &image.image_type));
            let Some(image) = image else { continue };
            for location in &self.availability {
                if defaults
                    .location
                    .as_ref()
                    .is_some_and(|wanted| wanted != &location.location_code)
                    || !location.availabilities.contains(&machine.instance_type)
                {
                    continue;
                }
                offers.push(Offer {
                    machine: machine.clone(),
                    image: image.clone(),
                    location: location.location_code.clone(),
                    allocated_disk_gb: disk,
                    compute_hourly_usd,
                    storage_hourly_usd,
                    hourly_usd,
                    cost_scope: "compute_and_os_storage",
                });
            }
        }
        offers.sort_by(|a, b| {
            a.hourly_usd
                .total_cmp(&b.hourly_usd)
                .then(a.machine.instance_type.cmp(&b.machine.instance_type))
                .then(a.location.cmp(&b.location))
        });
        offers.into_iter().next().ok_or_else(|| crate::automation::error("no_matching_offers",
            "No available ordinary NVIDIA GPU VM matches the filters, current Ubuntu 24/26 CUDA 12.9+ image compatibility and compute-plus-OS-storage price ceiling.",
            json!({"max_price_per_hr": requirements.max_price_per_hr, "allocated_disk_gb": disk, "cost_scope":"compute_and_os_storage", "profiling_access":"unverified"})))
    }
}
