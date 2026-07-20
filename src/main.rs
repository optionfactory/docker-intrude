mod cli;
mod docker;

use docker::DockerClient;
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::process::{Command, exit};

fn main() {
    let config = match cli::parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
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
fn execute_in_namespace(config: cli::Config) -> Result<i32, String> {
    DockerClient::ping()?;

    if !config.quiet {
        println!(":: Preparing Docker network holder ({}) ::", config.name);
    }

    let _cleanup_guard = DockerClient::provision_network_holder(&config.name, &config.net, &config.ip)?;
    let pid = DockerClient::get_container_pid(&config.name)?;

    let ns_path = format!("/proc/{pid}/ns/net");
    let ns_file = File::open(&ns_path).map_err(|e| format!("Failed to open namespace file {ns_path}: {e}"))?;

    let meta = ns_file
        .metadata()
        .map_err(|e| format!("Failed to read namespace file descriptor metadata: {e}"))?;

    let pid_owner_uid = meta.uid();

    let real_uid = nix::unistd::getuid().as_raw();

    if real_uid != 0 && pid_owner_uid != real_uid {
        return Err(format!(
            "Target namespace is owned by UID {pid_owner_uid} but you are UID {real_uid}. Aborting to prevent privilege escalation."
        ));
    }

    if !config.quiet {
        println!(":: Entering namespace ::");
    }

    match unsafe { nix::unistd::fork() } {
        Ok(nix::unistd::ForkResult::Parent { child }) => {
            drop(ns_file);

            unsafe {
                let _ =
                    nix::sys::signal::signal(nix::sys::signal::Signal::SIGINT, nix::sys::signal::SigHandler::SigIgn);
                let _ =
                    nix::sys::signal::signal(nix::sys::signal::Signal::SIGQUIT, nix::sys::signal::SigHandler::SigIgn);
            }

            if let Err(e) = caps::clear(None, caps::CapSet::Effective) {
                eprintln!("Warning: Failed to drop effective capabilities in parent: {e}");
            }
            if let Err(e) = caps::clear(None, caps::CapSet::Permitted) {
                eprintln!("Warning: Failed to drop permitted capabilities in parent: {e}");
            }

            match nix::sys::wait::waitpid(child, None) {
                Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => Ok(code),
                Ok(nix::sys::wait::WaitStatus::Signaled(_, signal, _)) => {
                    Err(format!("Child process terminated by signal: {:?}", signal))
                }
                _ => Err("Failed to harvest child process exit status".to_string()),
            }
        }
        Ok(nix::unistd::ForkResult::Child) => {
            let run_child = move || -> Result<(), String> {
                nix::sched::setns(ns_file, nix::sched::CloneFlags::CLONE_NEWNET)
                    .map_err(|e| format!("Failed to setns. Ensure binary has CAP_SYS_ADMIN and CAP_SYS_PTRACE: {e}"))?;

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
