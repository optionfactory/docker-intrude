# docker-intrude

Linux statically-linked utility that executes local host binaries inside a specific Docker network namespace. 

It spins up a temporary container with a static IP, uses `nsenter` to attach your command to its network, and runs it under your current user.

## Security & Trust Model
By default, `docker-intrude` drops Effective, Permitted, Inheritable, and Ambient capabilities, but preserves the Capability Bounding Set so that tools requiring Linux file capabilities (like `/bin/ping` or `gdb`) continue to function.

If your command doesn't need file capabilities, or if you prefer maximum privilege isolation, 
pass the `--strict` flag to clear the Bounding Set as well:

```bash
docker-intrude --name my-project --net dev-net --ip 172.18.0.22 --strict -- ping 172.18.0.1
```

## How It Works & DNS Resolution
`docker-intrude` provisions a temporary network container to hold a specific Docker network namespace open. 

It then uses kernel namespaces (`setns`) to attach your host command to that network.

Host binary would still read the host's `/etc/resolv.conf` and fail to resolve names in most distros.
To address this without modifying your host filesystem, `docker-intrude` overrides the configuration:

1. It unshares the mount namespace (`CLONE_NEWNS`) to isolate filesystem changes from the host.
2. It marks the root mount as private (`MS_PRIVATE | MS_REC`) to prevent mount propagation.
3. It creates a temporary file (`/dev/shm`) and writes Docker's embedded DNS configuration (`nameserver 127.0.0.11\noptions ndots:0\n`) to it.
4. It bind-mounts this file directly over `/etc/resolv.conf` and unlinks the source file from `/dev/shm`.


## Prerequisites
- **Linux only** (relies on native kernel namespaces).
- **Docker** running and accessible.

## Installation
Install the latest pre-compiled binary via curl:
```bash
curl -sSL \
  [https://github.com/optionfactory/docker-intrude/releases/latest/download/docker-intrude-linux-amd64-musl](https://github.com/optionfactory/docker-intrude/releases/latest/download/docker-intrude-linux-amd64-musl) \
  | sudo tee /usr/local/bin/docker-intrude > /dev/null \
  && sudo chown root:docker /usr/local/bin/docker-intrude \
  && sudo chmod 750 /usr/local/bin/docker-intrude \
  && sudo setcap cap_sys_admin,cap_sys_ptrace+ep /usr/local/bin/docker-intrude
```

## Build from Source
Ensure you have Rust installed, then clone the repository and build:

```bash
git clone [https://github.com/optionfactory/docker-intrude](https://github.com/optionfactory/docker-intrude)
cd docker-intrude
make build-release 
make install
```

## Usage
```bash
docker-intrude --name <name> --net <network> --ip <ip-address> [-v] -- <command...>
```

## Example
Run a local Maven project inside the dev-net Docker network:
```bash
docker-intrude --name my-project --net dev-net --ip 172.18.0.22 -- ./mvn spring-boot:run
```

## Options
- `--name`, `-n` : Name of the temporary Docker container.
- `--net` : The Docker network to join.
- `--ip` : The IP address to assign to the container.
- `--verbose`, `-v` : Enable detailed setup and status logging.
- `--` : Separates wrapper arguments from the command being executed.