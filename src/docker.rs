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

pub struct ContainerGuard {
    pub name: String,
}

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        let _ = DockerClient::query_socket("DELETE", &format!("/containers/{}?force=true", self.name), None);
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

pub struct DockerClient;

impl DockerClient {
    fn query_socket(method: &str, path: &str, json_body: Option<String>) -> Result<(u16, String), String> {
        let socket_path = resolve_socket_path();
        let real_uid = nix::unistd::getuid().as_raw();
        if let Ok(meta) = std::fs::metadata(&socket_path) {
            let owner_uid = meta.uid();
            if owner_uid != 0 && owner_uid != real_uid {
                return Err("Socket owner mismatch.".to_string());
            }
        } else {
            return Err(format!("Socket not found at {socket_path}"));
        }
        let mut stream = UnixStream::connect(&socket_path).map_err(|e| e.to_string())?;

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

    pub fn ping() -> Result<(), String> {
        let (status, _) = Self::query_socket("GET", "/_ping", None)?;
        if status != 200 {
            return Err("Docker daemon is not responding over the socket.".to_string());
        }
        Ok(())
    }

    pub fn provision_network_holder(name: &str, net: &str, ip: &str) -> Result<ContainerGuard, String> {
        let _ = Self::query_socket("DELETE", &format!("/containers/{name}?force=true"), None);

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

        let (create_status, create_body) = Self::query_socket(
            "POST",
            &format!("/containers/create?name={name}"),
            Some(serialized_body),
        )?;
        if create_status != 201 {
            return Err(format!("Docker allocation failure: {create_body}"));
        }

        let (start_status, start_body) = Self::query_socket("POST", &format!("/containers/{name}/start"), None)?;
        if start_status != 204 {
            return Err(format!("Docker start failure: {start_body}"));
        }

        Ok(ContainerGuard { name: name.to_string() })
    }

    pub fn get_container_pid(name: &str) -> Result<i32, String> {
        let (status, body) = Self::query_socket("GET", &format!("/containers/{name}/json"), None)?;
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
