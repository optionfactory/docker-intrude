mod cli;
mod docker;

use docker::DockerClient;
use nix::mount;
use nix::sched;
use nix::sched::setns;
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, kill, sigaction, signal};
use nix::sys::wait;
use nix::unistd;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::process::{Command, exit};
use std::sync::atomic::{AtomicBool, Ordering};

static TERMINATED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_term(_: libc::c_int) {
    TERMINATED.store(true, Ordering::Relaxed);
}

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
    let docker = DockerClient::new(config.verbose)?;
    docker.ping()?;

    if config.verbose {
        println!(":: Preparing Docker network holder ({}) ::", config.name);
    }

    let _cleanup_guard = docker.provision_network_holder(&config.name, &config.net, &config.ip)?;
    let pid = docker.get_container_pid(&config.name)?;

    let ns_path = format!("/proc/{pid}/ns/net");
    let ns_file = File::open(&ns_path).map_err(|e| format!("Failed to open namespace file {ns_path}: {e}"))?;

    let meta = ns_file
        .metadata()
        .map_err(|e| format!("Failed to read namespace file descriptor metadata: {e}"))?;
    let pid_owner_uid = meta.uid();

    if pid_owner_uid != docker.socket_uid {
        return Err(format!(
            "Target namespace is owned by UID {pid_owner_uid}, but the Docker socket is owned by UID {}. Aborting due to potential privilege escalation.",
            docker.socket_uid
        ));
    }

    let post_open_pid = docker.get_container_pid(&config.name)?;
    if pid != post_open_pid {
        return Err(format!(
            "Container state changed while attaching to namespace (PID changed from {pid} to {post_open_pid}). The container may have restarted or terminated."
        ));
    }

    if config.verbose {
        println!(":: Entering namespace ::");
    }

    match unsafe { unistd::fork() } {
        Ok(unistd::ForkResult::Parent { child }) => {
            drop(ns_file);

            unsafe {
                let _ = signal(Signal::SIGINT, SigHandler::SigIgn);
                let _ = signal(Signal::SIGQUIT, SigHandler::SigIgn);

                let action = SigAction::new(SigHandler::Handler(handle_term), SaFlags::empty(), SigSet::empty());
                let _ = sigaction(Signal::SIGTERM, &action);
            }

            loop {
                match wait::waitpid(child, None) {
                    Ok(wait::WaitStatus::Exited(_, code)) => return Ok(code),
                    Ok(wait::WaitStatus::Signaled(_, signal, _)) => {
                        return Err(format!("Child process terminated by signal: {:?}", signal));
                    }
                    Err(nix::errno::Errno::EINTR) => {
                        if TERMINATED.load(Ordering::Relaxed) {
                            let _ = kill(child, Signal::SIGTERM);
                            return Err("Process received SIGTERM, shutting down".to_string());
                        }
                        continue;
                    }
                    Err(e) => return Err(format!("Failed to harvest child process exit status: {e}")),
                    _ => return Err("Unexpected waitpid status".to_string()),
                }
            }
        }
        Ok(nix::unistd::ForkResult::Child) => {
            let run_child = move || -> Result<(), String> {
                setns(ns_file, nix::sched::CloneFlags::CLONE_NEWNET)
                    .map_err(|e| format!("Failed to setns. Ensure binary has CAP_SYS_ADMIN and CAP_SYS_PTRACE: {e}"))?;

                mount_resolve_conf()?;
                drop_capabilities(config.strict)?;
                let err = Command::new(&config.cmd[0]).args(&config.cmd[1..]).exec();
                Err(format!("Failed to exec target command: {err}"))
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

fn drop_capabilities(strict: bool) -> Result<(), String> {
    const SECBIT_NOROOT: libc::c_ulong = 0x01;
    let res = unsafe { libc::prctl(libc::PR_SET_SECUREBITS, SECBIT_NOROOT, 0, 0, 0) };
    if res != 0 {
        return Err(format!(
            "Failed to set SECBIT_NOROOT via prctl: {}",
            std::io::Error::last_os_error()
        ));
    }
    caps::clear(None, caps::CapSet::Effective).map_err(|e| format!("Failed to drop effective capabilities: {e}"))?;
    caps::clear(None, caps::CapSet::Permitted).map_err(|e| format!("Failed to drop permitted capabilities: {e}"))?;
    caps::clear(None, caps::CapSet::Inheritable)
        .map_err(|e| format!("Failed to drop inheritable capabilities: {e}"))?;
    caps::clear(None, caps::CapSet::Ambient).map_err(|e| format!("Failed to drop ambient capabilities: {e}"))?;
    if strict {
        caps::clear(None, caps::CapSet::Bounding).map_err(|e| format!("Failed to drop bounding capabilities: {e}"))?;
    }
    Ok(())
}

fn mount_resolve_conf() -> Result<(), String> {
    // unshare the mount namespace
    sched::unshare(sched::CloneFlags::CLONE_NEWNS).map_err(|e| format!("Failed to unshare mount namespace: {e}"))?;

    // prevent mount propagation back to the host
    mount::mount(
        Some("none"),
        "/",
        None::<&str>,
        mount::MsFlags::MS_REC | mount::MsFlags::MS_PRIVATE,
        None::<&str>,
    )
    .map_err(|e| format!("Failed to make root mount private: {e}"))?;

    let tmp_path = format!(
        "/dev/shm/docker-intrude-resolv-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    );

    let mut tmp_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp_path)
        .map_err(|e| format!("Failed to create secure shm file: {e}"))?;

    tmp_file
        .write_all(b"nameserver 127.0.0.11\noptions ndots:0\n")
        .map_err(|e| format!("Failed to write to shm file: {e}"))?;

    // bind mount the memory file over resolv.conf
    let mount_res = mount::mount(
        Some(tmp_path.as_str()),
        "/etc/resolv.conf",
        None::<&str>,
        mount::MsFlags::MS_BIND,
        None::<&str>,
    );
    // unlink the file from the host filesystem.
    let _ = std::fs::remove_file(&tmp_path);
    mount_res.map_err(|e| format!("Failed to bind mount resolv.conf: {e}"))?;
    Ok(())
}
