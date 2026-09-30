use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

const ID: &str = "11111111-1111-4111-8111-111111111111";
const OS: &str = "22222222-2222-4222-8222-222222222222";
const EXTRA: &str = "33333333-3333-4333-8333-333333333333";
const KEY: &str = "44444444-4444-4444-8444-444444444444";

#[derive(Clone, Debug)]
struct Request {
    method: String,
    path: String,
    body: Value,
    authorization: String,
}
struct Reply {
    status: u16,
    body: Value,
    headers: String,
}
impl Reply {
    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            body,
            headers: String::new(),
        }
    }
}
struct Server {
    client: Client,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new(handler: impl Fn(&Request) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let client = Client::mock(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let handle = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                let header_end = loop {
                    let mut chunk = [0; 4096];
                    let n = stream.read(&mut chunk).unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let header = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|n| n.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while bytes.len() < header_end + length {
                    let mut chunk = [0; 4096];
                    let n = stream.read(&mut chunk).unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&chunk[..n]);
                }
                let mut first = header.lines().next().unwrap().split_whitespace();
                let request = Request {
                    method: first.next().unwrap().into(),
                    path: first.next().unwrap().into(),
                    body: if length == 0 {
                        Value::Null
                    } else {
                        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
                    },
                    authorization: header
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                                .map(|(_, v)| v.trim().to_owned())
                        })
                        .unwrap_or_default(),
                };
                captured.lock().unwrap().push(request.clone());
                let response = handler(&request);
                let body = if response.status == 204 {
                    String::new()
                } else {
                    response.body.to_string()
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                    response.status,
                    body.len(),
                    response.headers,
                    body
                );
            }
        });
        Self {
            client,
            requests,
            stop,
            handle: Some(handle),
        }
    }
    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.handle.take().unwrap().join().unwrap();
    }
}
fn token() -> Reply {
    Reply::json(200, json!({"access_token":"mock-token","expires_in":3600}))
}

#[test]
fn login_validates_oauth_without_reading_or_creating_resources() {
    for status in [200, 401] {
        let server = Server::new(move |request| {
            assert_eq!(request.method, "POST");
            assert_eq!(request.path, "/oauth2/token");
            assert_eq!(
                request.body,
                json!({"grant_type":"client_credentials", "client_id":"test-client", "client_secret":"test-secret"})
            );
            if status == 200 {
                token()
            } else {
                Reply::json(status, json!({"error":"test-secret"}))
            }
        });
        let result = server.client.authenticate();
        assert_eq!(result.is_ok(), status == 200);
        if let Err(error) = result {
            assert!(!format!("{error:#}").contains("test-secret"));
        }
        assert_eq!(server.requests().len(), 1);
    }
}
fn instance(status: &str) -> Value {
    json!({"id":ID,"hostname":"ice-verda-test","status":status,"ip":"192.0.2.1",
        "os_volume_id":OS,"volume_ids":[OS,EXTRA],"price_per_hour":0.6,
        "jupyter_token":"must-not-appear-in-output"})
}
fn catalog() -> Catalog {
    serde_json::from_value(json!({
        "machines":[{"instance_type":"1RTXA6000.10V","model":"RTX A6000","name":"RTX A6000 48GB","manufacturer":"NVIDIA","currency":"usd","price_per_hour":"0.6",
            "cpu":{"number_of_cores":10},"gpu":{"number_of_gpus":1},"gpu_memory":{"size_in_gigabytes":48},"memory":{"size_in_gigabytes":64},
            "supported_os":["ubuntu-24.04-cuda-12.9-open","ubuntu-26.04-cuda-13.2-open"]}],
        "images":[{"id":OS,"image_type":"ubuntu-24.04-cuda-12.9-open","name":"Ubuntu CUDA","category":"cuda","is_cluster":false},
            {"id":EXTRA,"image_type":"ubuntu-26.04-cuda-13.2-open","name":"Ubuntu CUDA","category":"cuda","is_cluster":false}],
        "availability":[{"location_code":"FIN-01","availabilities":["1RTXA6000.10V"]}],
        "volume_types":[{"type":"NVMe","price":{"currency":"usd","price_per_month_per_gb":0.2}}]
    })).unwrap()
}
fn requirements() -> crate::model::CreateSearchRequirements {
    crate::model::CreateSearchRequirements {
        max_price_per_hr: 0.7,
        ..Default::default()
    }
}
fn offer() -> Offer {
    catalog()
        .select(&requirements(), &Default::default(), None)
        .unwrap()
}

#[test]
fn storage_is_in_the_ceiling_and_preview_has_no_enforced_deadline() {
    let offer = offer();
    assert_eq!(offer.allocated_disk_gb, 100);
    assert!((offer.storage_hourly_usd - 0.0274).abs() < 1e-12);
    let mut req = requirements();
    req.max_price_per_hr = 0.62;
    assert!(catalog().select(&req, &Default::default(), None).is_err());
    let preview = preview(&offer, 0.25, 100).unwrap();
    assert_eq!(preview["cleanup"]["planned_deadline_unix"], 1000);
    assert_eq!(preview["cleanup"]["deadline_enforced"], false);
    assert_eq!(preview["profiling"]["admitted"], false);
    assert_eq!(
        preview["cost"]["estimated_total_usd"],
        offer.hourly_usd * 0.25
    );
}

#[test]
fn live_catalog_shapes_and_authentication_are_supported() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            return token();
        }
        let data = serde_json::to_value(catalog()).unwrap();
        let field = if request.path.starts_with("/instance-types?currency=usd") {
            "machines"
        } else if request
            .path
            .starts_with("/instance-availability?is_spot=false")
        {
            "availability"
        } else if request.path.starts_with("/volume-types?") {
            "volume_types"
        } else if request.path.starts_with("/images?") {
            "images"
        } else {
            return Reply::json(404, json!({}));
        };
        Reply::json(200, data[field].clone())
    });
    let chosen = Catalog::load(&server.client)
        .unwrap()
        .select(&requirements(), &Default::default(), None)
        .unwrap();
    assert_eq!(chosen.image.image_type, "ubuntu-26.04-cuda-13.2-open");
    let requests = server.requests();
    assert_eq!(requests[0].body["grant_type"], "client_credentials");
    assert_eq!(requests[0].body["client_secret"], "test-secret");
    assert!(
        requests[1..]
            .iter()
            .all(|r| r.method == "GET" && r.authorization == "Bearer mock-token")
    );
}

#[test]
fn filters_image_pins_and_availability_are_enforced() {
    let mut req = requirements();
    for count in [0, 2] {
        req.gpu_count = Some(count);
        assert!(catalog().select(&req, &Default::default(), None).is_err());
    }
    req.gpu_count = Some(1);
    req.min_gpu_memory_gb = Some(49.0);
    assert!(catalog().select(&req, &Default::default(), None).is_err());
    req.min_gpu_memory_gb = Some(48.0);
    req.allowed_gpus = vec!["RTX A6000".into()];
    let mut defaults = crate::model::VerdaDefaults {
        image: Some("ubuntu-24.04-cuda-12.9-open".into()),
        ..Default::default()
    };
    assert_eq!(
        catalog()
            .select(&req, &defaults, None)
            .unwrap()
            .image
            .image_type,
        defaults.image.as_ref().unwrap().as_str()
    );
    defaults.location = Some("FIN-02".into());
    assert!(catalog().select(&req, &defaults, None).is_err());
    defaults.location = None;
    req.min_cpus = 11;
    assert!(catalog().select(&req, &defaults, None).is_err());
    req.min_cpus = 0;
    req.min_ram_gb = 65.0;
    assert!(catalog().select(&req, &defaults, None).is_err());
    req.min_ram_gb = 0.0;
    req.allowed_gpus = vec!["H100".into()];
    assert!(catalog().select(&req, &defaults, None).is_err());
    let mut cat = catalog();
    cat.availability.clear();
    assert!(
        cat.select(&requirements(), &Default::default(), None)
            .is_err()
    );
    assert!(
        catalog()
            .select(&requirements(), &Default::default(), Some("missing"))
            .is_err()
    );
}

#[test]
fn unknown_prices_currencies_and_non_vm_types_are_not_accepted() {
    for price in [
        Value::Null,
        json!("NaN"),
        json!(0),
        json!(-1),
        json!("infinity"),
    ] {
        let mut cat = catalog();
        cat.machines[0].price_per_hour = price;
        assert!(
            cat.select(&requirements(), &Default::default(), None)
                .is_err()
        );
    }
    let mut cat = catalog();
    cat.volume_types[0].price.currency = "eur".into();
    assert!(
        cat.select(&requirements(), &Default::default(), None)
            .is_err()
    );
    let mut cat = catalog();
    cat.machines[0].currency = "eur".into();
    assert!(
        cat.select(&requirements(), &Default::default(), None)
            .is_err()
    );
    let mut cat = catalog();
    cat.machines[0].instance_type = "8H100.80S".into();
    assert!(
        cat.select(&requirements(), &Default::default(), None)
            .is_err()
    );
}

#[test]
fn incompatible_old_and_confidential_images_are_rejected() {
    for image_type in [
        "ubuntu-22.04-cuda-13.0",
        "ubuntu-24.04-cuda-12.8",
        "ubuntu-26.04",
        "ubuntu-26.04-cuda-13.2-cc",
    ] {
        let mut image = catalog().images[0].clone();
        image.image_type = image_type.into();
        assert!(catalog::image_version(&image).is_none());
    }
    let mut cat = catalog();
    cat.machines[0].supported_os.clear();
    assert!(
        cat.select(&requirements(), &Default::default(), None)
            .is_err()
    );
    let mut cat = catalog();
    for image in &mut cat.images {
        image.category = "confidentialComputing".into();
    }
    assert!(
        cat.select(&requirements(), &Default::default(), None)
            .is_err()
    );
    let mut cat = catalog();
    cat.machines[0].description = "Confidential Computing".into();
    assert!(
        cat.select(&requirements(), &Default::default(), None)
            .is_err()
    );
}

#[test]
fn create_sends_one_fixed_price_vm_with_priced_storage_and_no_scheduler_claim() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            token()
        } else {
            Reply::json(202, json!(ID))
        }
    });
    assert_eq!(
        create_vm(&server.client, &offer(), "ice-test", KEY).unwrap(),
        ID
    );
    let requests = server.requests();
    let request = &requests[1];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/instances");
    assert_eq!(request.body["os_volume"]["size"], 100);
    assert_eq!(request.body["contract"], "PAY_AS_YOU_GO");
    assert_eq!(request.body["is_spot"], false);
    assert_eq!(request.body["ssh_key_ids"], json!([KEY]));
    assert!(request.body.get("startup_script_id").is_none());
}

#[test]
fn ambiguous_creation_is_not_replayed_and_preserves_hostname() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            token()
        } else {
            Reply::json(503, json!({"message":"test-secret"}))
        }
    });
    let error = create_vm(&server.client, &offer(), "ice-reconcile", KEY).unwrap_err();
    let details = &error
        .downcast_ref::<crate::automation::AgentError>()
        .unwrap()
        .details;
    assert_eq!(details["instance_name"], "ice-reconcile");
    assert_eq!(details["reconcile_before_retry"], true);
    assert!(!format!("{error:#}").contains("test-secret"));
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|r| r.path == "/instances")
            .count(),
        1
    );
}

#[test]
fn startup_failure_returns_instance_and_volume_cleanup_details() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            token()
        } else {
            Reply::json(200, instance("installation_failed"))
        }
    });
    let error = server
        .client
        .wait(ID, true, Duration::from_secs(1))
        .unwrap_err();
    let error = error
        .downcast_ref::<crate::automation::AgentError>()
        .unwrap();
    assert_eq!(error.code, "instance_startup_failed");
    assert_eq!(error.details["instance_id"], ID);
    assert_eq!(error.details["os_volume_id"], OS);
    assert_eq!(error.details["volume_ids"], json!([OS, EXTRA]));
    assert!(
        error.details["cleanup_command"]
            .as_str()
            .unwrap()
            .contains(ID)
    );
}

#[test]
fn readiness_wait_is_bounded_and_unknown_state_does_not_pass() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            token()
        } else {
            Reply::json(200, instance("future_state"))
        }
    });
    let start = Instant::now();
    let error = server
        .client
        .wait(ID, true, Duration::from_millis(100))
        .unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(
        error
            .downcast_ref::<crate::automation::AgentError>()
            .unwrap()
            .code,
        "startup_timeout"
    );
}

#[test]
fn action_partial_failure_is_not_success() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            token()
        } else {
            Reply::json(
                207,
                json!([{"instanceId":ID,"action":"shutdown","status":"error","error":"secret"}]),
            )
        }
    });
    assert!(server.client.action(ID, "shutdown", None).is_err());
}

#[test]
fn start_and_stop_map_to_verda_actions_without_deleting_storage() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            token()
        } else {
            Reply::json(204, Value::Null)
        }
    });
    let vm: Instance = serde_json::from_value(instance("running")).unwrap();
    Provider::set_running(&server.client, &vm, false).unwrap();
    Provider::set_running(&server.client, &vm, true).unwrap();
    let actions: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.path == "/instances")
        .collect();
    assert_eq!(actions[0].body, json!({"id":ID,"action":"shutdown"}));
    assert_eq!(actions[1].body, json!({"id":ID,"action":"start"}));
    let summary = vm.json_summary().to_string();
    assert!(!summary.contains("must-not-appear"));
    assert_eq!(vm.json_summary()["profiling_access"], "unverified");
}

#[test]
fn delete_only_removes_boot_volume_and_verifies_absence() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            token()
        } else if request.method == "PUT" {
            Reply::json(
                202,
                json!([{"instanceId":ID,"action":"delete","status":"success"}]),
            )
        } else if request.path.starts_with("/volumes/trash?") {
            Reply::json(200, json!([]))
        } else {
            Reply::json(404, json!({}))
        }
    });
    let vm: Instance = serde_json::from_value(instance("offline")).unwrap();
    let receipt = delete_selected(&server.client, &vm, Duration::from_secs(1)).unwrap();
    assert_eq!(receipt["verification"], "read_back_absent");
    assert_eq!(receipt["retained_volume_ids"], json!([EXTRA]));
    let request = server
        .requests()
        .into_iter()
        .find(|r| r.method == "PUT")
        .unwrap();
    assert_eq!(request.body["volume_ids"], json!([OS]));
    assert_eq!(request.body["delete_permanently"], true);
}

#[test]
fn retained_boot_volume_prevents_claiming_verified_deletion() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            token()
        } else if request.method == "PUT" {
            Reply::json(204, Value::Null)
        } else if request.path.starts_with("/volumes/") {
            Reply::json(200, json!({"id":OS,"status":"deleting"}))
        } else {
            Reply::json(404, json!({}))
        }
    });
    let vm: Instance = serde_json::from_value(instance("offline")).unwrap();
    let error = delete_selected(&server.client, &vm, Duration::from_millis(100)).unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::automation::AgentError>()
            .unwrap()
            .code,
        "deletion_unverified"
    );
}

#[test]
fn collection_pagination_is_complete_and_bad_metadata_fails() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            return token();
        }
        let page = if request.path.contains("page=1&") {
            1
        } else {
            2
        };
        Reply {
            status: 200,
            body: json!([page]),
            headers: "X-Total-Count: 2\r\n".into(),
        }
    });
    assert_eq!(server.client.list::<u32>("/images").unwrap(), vec![1, 2]);
}

#[test]
fn expired_token_refreshes_reads_but_never_replays_a_create() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    let server = Server::new(move |request| {
        if request.path == "/oauth2/token" {
            token()
        } else if count.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::json(401, json!({}))
        } else {
            Reply::json(200, json!([]))
        }
    });
    assert!(server.client.list::<Value>("/images").unwrap().is_empty());
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|r| r.path == "/oauth2/token")
            .count(),
        2
    );
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            token()
        } else {
            Reply::json(401, json!({}))
        }
    });
    assert!(create_vm(&server.client, &offer(), "ice-unknown", KEY).is_err());
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn connection_metadata_rejects_host_injection_and_quotes_key_paths() {
    let mut vm: Instance = serde_json::from_value(instance("running")).unwrap();
    let command =
        ssh::connection_command(&vm, Some(std::path::Path::new("/tmp/key with spaces"))).unwrap();
    assert!(command.contains("'/tmp/key with spaces'"));
    assert!(command.contains("'root@192.0.2.1'"));
    vm.ip = Some("-oProxyCommand=bad".into());
    assert!(ssh::connection_command(&vm, None).is_err());
}

#[test]
fn trashed_os_volume_is_not_permanent_deletion() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            token()
        } else if request.method == "PUT" {
            Reply::json(204, Value::Null)
        } else if request.path.starts_with("/volumes/trash?") {
            Reply::json(200, json!([{"id": OS}]))
        } else {
            Reply::json(404, json!({}))
        }
    });
    let vm: Instance = serde_json::from_value(instance("offline")).unwrap();
    let error = delete_selected(&server.client, &vm, Duration::from_secs(1)).unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::automation::AgentError>()
            .unwrap()
            .code,
        "storage_cleanup_unverified"
    );
}

#[test]
fn api_errors_do_not_echo_submitted_credentials() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            token()
        } else {
            Reply::json(429, json!({"message": "test-secret mock-token"}))
        }
    });
    let error = server.client.list::<Value>("/images").unwrap_err();
    let text = format!("{error:#}");
    assert!(!text.contains("test-secret"));
    assert!(!text.contains("mock-token"));
    assert_eq!(
        error
            .downcast_ref::<crate::automation::AgentError>()
            .unwrap()
            .details["http_status"],
        429
    );
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn mismatched_instance_receipt_cannot_target_another_vm() {
    let server = Server::new(|request| {
        if request.path == "/oauth2/token" {
            token()
        } else {
            let mut vm = instance("running");
            vm["id"] = json!(EXTRA);
            Reply::json(200, vm)
        }
    });
    let error = server
        .client
        .instance(ID, Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::automation::AgentError>()
            .unwrap()
            .code,
        "verda_response_mismatch"
    );
    assert!(
        server
            .requests()
            .iter()
            .all(|r| r.method == "GET" || r.path == "/oauth2/token")
    );
}
