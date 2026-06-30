use std::env;
use std::fs::File;
use std::net::IpAddr;
use std::process::{Command, Stdio, exit};

const SLOTH_IMAGE: &str = "optionfactory/sloth:226";
const ALLOWED_DOCKER_PATHS: &str =
    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/snap/bin:/run/current-system/sw/bin";

struct ContainerGuard {
    name: String,
}

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "--", &self.name]) 
            .env("PATH", ALLOWED_DOCKER_PATHS)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

struct Config {
    name: String,
    net: String,
    ip: String,
    quiet: bool,
    cmd: Vec<String>,
}

fn main() {
    let config = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            eprintln!("Usage:");
            eprintln!("  docker-intrude --name <NAME> --net <NET> --ip <IP> [-q] -- <CMD...>");
            eprintln!("  docker-intrude --help | -h");
            eprintln!("  docker-intrude --version | -V");
            exit(1);
        }
    };

    match execute_in_namespace(config) {
        Ok(code) => {
            if code != 0 {
                exit(code);
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            exit(1);
        }
    }
}

fn execute_quietly(cmd: &mut Command, err_msg: &str) -> Result<(), String> {
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| format!("Failed to execute: {err_msg}"))?
        .success()
        .then_some(())
        .ok_or_else(|| err_msg.to_string())
}

fn execute_in_namespace(config: Config) -> Result<i32, String> {
    let mut info_cmd = Command::new("docker");
    info_cmd.arg("info").env("PATH", ALLOWED_DOCKER_PATHS);
    execute_quietly(
        &mut info_cmd,
        "Docker is not installed, or the Docker daemon is not running/accessible.",
    )?;

    if !config.quiet {
        println!(":: Preparing Docker network holder ({}) ::", config.name);
    }

    let _ = Command::new("docker")
        .args(["rm", "-f", "--", &config.name])
        .env("PATH", ALLOWED_DOCKER_PATHS)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let mut run_cmd = Command::new("docker");
    run_cmd
        .arg("run")
        .arg("-d")
        .arg("--rm")
        .arg("--name")
        .arg(&config.name)
        .arg("--network")
        .arg(&config.net)
        .arg("--ip")
        .arg(&config.ip)
        .arg(SLOTH_IMAGE)
        .env("PATH", ALLOWED_DOCKER_PATHS);

    execute_quietly(&mut run_cmd, "Failed to start docker container.")?;

    let inspect_output = Command::new("docker")
        .args(["inspect", "-f", "{{.State.Pid}}", "--", &config.name])
        .env("PATH", ALLOWED_DOCKER_PATHS)
        .output()
        .map_err(|_| "Failed to execute docker inspect")?;

    if !inspect_output.status.success() {
        return Err("Failed to inspect docker container for PID.".to_string());
    }

    let pid = String::from_utf8_lossy(&inspect_output.stdout).trim().to_string();

    if !config.quiet {
        println!(":: Entering namespace ::");
    }

    match unsafe { nix::unistd::fork() } {
        Ok(nix::unistd::ForkResult::Parent { child }) => {
            if let Err(e) = caps::clear(None, caps::CapSet::Effective) {
                eprintln!("Warning: Failed to drop effective capabilities in parent: {e}");
            }
            if let Err(e) = caps::clear(None, caps::CapSet::Permitted) {
                eprintln!("Warning: Failed to drop permitted capabilities in parent: {e}");
            }

            let _cleanup_guard = ContainerGuard {
                name: config.name.clone(),
            };

            match nix::sys::wait::waitpid(child, None) {
                Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => Ok(code),
                Ok(nix::sys::wait::WaitStatus::Signaled(_, signal, _)) => {
                    Err(format!("Child process terminated by signal: {:?}", signal))
                }
                _ => Err("Failed to harvest child process exit status".to_string()),
            }
        }
        Ok(nix::unistd::ForkResult::Child) => {
            let run_child = || -> Result<(), String> {
                let ns_path = format!("/proc/{pid}/ns/net");
                let ns_file =
                    File::open(&ns_path).map_err(|e| format!("Failed to open namespace file {ns_path}: {e}"))?;

                nix::sched::setns(ns_file, nix::sched::CloneFlags::CLONE_NEWNET)
                    .map_err(|e| format!("setns failed. Ensure binary has CAP_SYS_ADMIN and CAP_SYS_PTRACE: {e}"))?;

                if let Err(e) = caps::clear(None, caps::CapSet::Effective) {
                    return Err(format!("Failed to drop effective caps in child: {e}"));
                }
                if let Err(e) = caps::clear(None, caps::CapSet::Permitted) {
                    return Err(format!("Failed to drop permitted caps in child: {e}"));
                }

                let status = Command::new(&config.cmd[0])
                    .args(&config.cmd[1..])
                    .status()
                    .map_err(|e| format!("Failed to run target command: {e}"))?;

                std::process::exit(status.code().unwrap_or(1));
            };

            if let Err(e) = run_child() {
                eprintln!("Namespace Error: {e}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
        Err(e) => Err(format!("Process fork failed: {e}")),
    }
}

fn print_help() {
    println!("docker-intrude - Run commands directly within a specific Docker network namespace");
    println!();
    println!("Usage:");
    println!("  docker-intrude --name <NAME> --net <NET> --ip <IP> [-q] -- <CMD...>");
    println!("  docker-intrude --help | -h");
    println!("  docker-intrude --version | -V");
    println!();
    println!("Options:");
    println!("  -n, --name <NAME>  Name of the temporary network holder container");
    println!("      --net <NET>    Docker network name to connect to");
    println!("      --ip <IP>      Static IP address to assign to the container");
    println!("  -q, --quiet        Suppress informational output messages");
    println!("  -V, --version      Print version information");
    println!("  -h, --help         Print this help menu");
    println!();
    println!("Arguments:");
    println!("  -- <CMD...>        The command (and its arguments) to execute inside the namespace");
}

fn is_valid_docker_identifier(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

fn parse_args() -> Result<Config, String> {
    let args_vec: Vec<String> = env::args().skip(1).collect();

    if args_vec.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        exit(0);
    }
    if args_vec.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("docker-intrude {}", env!("CARGO_PKG_VERSION"));
        exit(0);
    }

    let mut args = args_vec.into_iter();
    let mut name = None;
    let mut net = None;
    let mut ip = None;
    let mut quiet = false;
    let mut cmd = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--name" | "-n" => name = Some(args.next().ok_or("Missing value for --name")?),
            "--net" => net = Some(args.next().ok_or("Missing value for --net")?),
            "--ip" => ip = Some(args.next().ok_or("Missing value for --ip")?),
            "--quiet" | "-q" => quiet = true,
            "--" => {
                cmd.extend(args);
                break;
            }
            other => {
                if other.starts_with('-') {
                    return Err(format!("Unknown flag: {other}"));
                } else {
                    cmd.push(other.to_string());
                    cmd.extend(args);
                    break;
                }
            }
        }
    }

    if cmd.is_empty() {
        return Err("Missing command to execute".to_string());
    }

    let name_val = name.ok_or("Missing required flag: --name")?;
    let net_val = net.ok_or("Missing required flag: --net")?;
    let ip_val = ip.ok_or("Missing required flag: --ip")?;

    if ip_val.parse::<IpAddr>().is_err() {
        return Err(format!("Invalid IP format provided ('{ip_val}')"));
    }

    if !is_valid_docker_identifier(&name_val) {
        return Err(format!("Invalid container name ('{name_val}')."));
    }

    if !is_valid_docker_identifier(&net_val) {
        return Err(format!("Invalid network name ('{net_val}')."));
    }

    Ok(Config {
        name: name_val,
        net: net_val,
        ip: ip_val,
        quiet,
        cmd,
    })
}
