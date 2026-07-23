use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;

const SLOTH_IMAGE: &str = "optionfactory/sloth:226";

#[derive(serde::Serialize)]
struct HostConfig {
    #[serde(rename = "NetworkMode")]
    network_mode: String,
}

#[derive(serde::Serialize)]
struct IpamConfig {
    #[serde(rename = "IPv4Address")]
    ipv4_address: String,
}

#[derive(serde::Serialize)]
struct EndpointConfig {
    #[serde(rename = "IPAMConfig")]
    ipam_config: IpamConfig,
}

#[derive(serde::Serialize)]
struct NetworkingConfig {
    #[serde(rename = "EndpointsConfig")]
    endpoints_config: HashMap<String, EndpointConfig>,
}

#[derive(serde::Serialize)]
struct CreateContainerPayload {
    #[serde(rename = "Image")]
    image: String,
    #[serde(rename = "HostConfig")]
    host_config: HostConfig,
    #[serde(rename = "NetworkingConfig")]
    networking_config: NetworkingConfig,
}

#[derive(serde::Deserialize)]
struct ContainerState {
    #[serde(rename = "Pid")]
    pid: i32,
}

#[derive(serde::Deserialize)]
struct InspectResponse {
    #[serde(rename = "State")]
    state: ContainerState,
}

pub struct ContainerGuard<'a> {
    pub name: String,
    client: &'a DockerClient,
}

impl<'a> Drop for ContainerGuard<'a> {
    fn drop(&mut self) {
        let _ = self
            .client
            .query_socket("DELETE", &format!("/containers/{}?force=true", self.name), None);
    }
}

fn resolve_socket_path() -> String {
    if let Ok(docker_host) = std::env::var("DOCKER_HOST") {
        if let Some(path) = docker_host.strip_prefix("unix://") {
            return path.to_string();
        }
    }
    "/var/run/docker.sock".to_string()
}

pub struct DockerClient {
    socket_path: String,
    pub socket_uid: u32,
    verbose: bool,
}

impl DockerClient {
    pub fn new(verbose: bool) -> Result<Self, String> {
        let socket_path = resolve_socket_path();
        let real_uid = nix::unistd::getuid().as_raw();

        let meta = std::fs::metadata(&socket_path).map_err(|_| format!("Socket not found at {socket_path}"))?;

        let socket_uid = meta.uid();
        if socket_uid != 0 && socket_uid != real_uid {
            return Err("Socket owner mismatch.".to_string());
        }

        Ok(Self {
            socket_path,
            socket_uid,
            verbose
        })
    }

    fn query_socket(&self, method: &str, path: &str, json_body: Option<String>) -> Result<(u16, String), String> {
        let mut stream = UnixStream::connect(&self.socket_path).map_err(|e| e.to_string())?;

        let timeout = Some(std::time::Duration::from_secs(10));
        stream
            .set_read_timeout(timeout)
            .map_err(|e| format!("Failed to set read timeout: {e}"))?;
        stream
            .set_write_timeout(timeout)
            .map_err(|e| format!("Failed to set write timeout: {e}"))?;

        let mut request = format!("{method} {path} HTTP/1.0\r\nHost: localhost\r\nAccept: application/json\r\n");
        if let Some(ref body) = json_body {
            request.push_str("Content-Type: application/json\r\n");
            request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        request.push_str("\r\n");
        if let Some(body) = json_body {
            request.push_str(&body);
        }

        stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;

        let mut response = String::new();
        stream
            .take(4 * 1024 * 1024)
            .read_to_string(&mut response)
            .map_err(|e| e.to_string())?;

        let first_line = response.lines().next().ok_or("Empty response")?;
        let status_code = first_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or("Malformed HTTP response")?;

        let body = match response.find("\r\n\r\n") {
            Some(index) => response[index + 4..].to_string(),
            None => String::new(),
        };

        Ok((status_code, body))
    }

    pub fn ping(&self) -> Result<(), String> {
        let (status, _) = self.query_socket("GET", "/_ping", None)?;
        if status != 200 {
            return Err("Docker daemon is not responding over the socket.".to_string());
        }
        Ok(())
    }

    fn ensure_image_exists(&self) -> Result<(), String> {
        let (status, _) = self.query_socket("GET", &format!("/images/{}/json", SLOTH_IMAGE), None)?;
        if status == 200 {
            return Ok(());
        }
        if self.verbose {
            println!(
                ":: Image '{}' not found locally. Pulling from registry... ::",
                SLOTH_IMAGE
            );
        }

        let (pull_status, pull_body) =
            self.query_socket("POST", &format!("/images/create?fromImage={}", SLOTH_IMAGE), None)?;

        if pull_status != 200 {
            return Err(format!("Failed to pull Docker image '{}': {}", SLOTH_IMAGE, pull_body));
        }

        Ok(())
    }

    pub fn provision_network_holder(&self, name: &str, net: &str, ip: &str) -> Result<ContainerGuard<'_>, String> {
        self.ensure_image_exists()?;
        let _ = self.query_socket("DELETE", &format!("/containers/{name}?force=true"), None);

        let mut endpoints_map = HashMap::new();
        endpoints_map.insert(
            net.to_string(),
            EndpointConfig {
                ipam_config: IpamConfig {
                    ipv4_address: ip.to_string(),
                },
            },
        );

        let create_payload = CreateContainerPayload {
            image: SLOTH_IMAGE.to_string(),
            host_config: HostConfig {
                network_mode: net.to_string(),
            },
            networking_config: NetworkingConfig {
                endpoints_config: endpoints_map,
            },
        };

        let serialized_body = serde_json::to_string(&create_payload).map_err(|e| e.to_string())?;
        let (create_status, create_body) = self.query_socket(
            "POST",
            &format!("/containers/create?name={name}"),
            Some(serialized_body),
        )?;

        if create_status != 201 {
            return Err(format!("Docker allocation failure: {create_body}"));
        }

        let (start_status, start_body) = self.query_socket("POST", &format!("/containers/{name}/start"), None)?;
        if start_status != 204 {
            return Err(format!("Docker start failure: {start_body}"));
        }

        Ok(ContainerGuard {
            name: name.to_string(),
            client: self,
        })
    }

    pub fn get_container_pid(&self, name: &str) -> Result<i32, String> {
        let (status, body) = self.query_socket("GET", &format!("/containers/{name}/json"), None)?;
        if status != 200 {
            return Err(format!("Docker state inspection failure: {body}"));
        }

        let inspect_data: InspectResponse = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        if inspect_data.state.pid <= 0 {
            return Err("Container holder is not running. Ensure the image is valid and try again.".to_string());
        }

        Ok(inspect_data.state.pid)
    }
}
