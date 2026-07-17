use std::env;
use std::net::IpAddr;
use std::process::exit;

pub struct Config {
    pub name: String,
    pub net: String,
    pub ip: String,
    pub quiet: bool,
    pub cmd: Vec<String>,
}

pub fn parse_args() -> Result<Config, String> {
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
        return Err(format!("Invalid IP format ('{ip_val}')")); 
    }
    if !is_valid_docker_identifier(&name_val) || !is_valid_docker_identifier(&net_val) {
        return Err("Invalid network or container identifier syntax".to_string());
    }

    Ok(Config { name: name_val, net: net_val, ip: ip_val, quiet, cmd })
}

fn is_valid_docker_identifier(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

fn print_help() {
    println!("docker-intrude - Run commands directly within a specific Docker network namespace");
    println!();
    println!("Usage:");
    println!("  docker-intrude --name <NAME> --net <NET> --ip <IP> [-q] -- <CMD...>");
}