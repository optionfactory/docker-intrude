# docker-intrude

Linux statically-linked utility that executes local host binaries inside a specific Docker network namespace. 

It spins up a temporary container with a static IP, uses `nsenter` to attach your command to its network, and runs it under your current user.

## Prerequisites
- **Linux only** (relies on native kernel namespaces).
- **Docker** running and accessible.

## Installation
Install the latest pre-compiled binary via curl:

```bash
curl -sSL \
 https://github.com/optionfactory/docker-intrude/releases/latest/download/docker-intrude-amd64-linux-musl \
 | sudo tee /usr/local/bin/docker-intrude > /dev/null \
 && sudo setcap cap_sys_admin,cap_sys_ptrace+ep /usr/local/bin/docker-intrude
```

## Build from Source
Ensure you have Rust installed, then clone the repository and build:

```bash
git clone https://github.com/optionfactory/docker-intrude
cd pinch
make build-release
sudo make install
```

## Usage

```bash
docker-intrude --name <name> --net <network> --ip <ip-address> [-q] -- <command...>
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
- `--quiet`, `-q` : Suppress setup/teardown logs.
- `--` : Separates wrapper arguments from the command being executed.