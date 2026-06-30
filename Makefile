REPO_OWNER=optionfactory
REPO_NAME=docker-intrude


build:
	cargo build

build-release:
	cargo build --release --target x86_64-unknown-linux-musl

run:
	cargo run

install:
	cp target/x86_64-unknown-linux-musl/release/docker-intrude /usr/local/bin/docker-intrude
	sudo setcap cap_sys_admin,cap_sys_ptrace+ep /usr/local/bin/docker-intrude

publish-github: build-release
	$(eval version=$(shell cargo metadata --format-version=1 --no-deps | jq -r '.packages[0].version'))
	$(eval github_token=$(shell echo url=https://github.com/$(REPO_OWNER)/$(REPO_NAME) | git credential fill | grep '^password=' | sed 's/password=//'))
	$(eval release_id=$(shell curl -s -X POST \
		-H "Accept: application/vnd.github+json" \
		-H "Authorization: Bearer $(github_token)" \
		-H "X-GitHub-Api-Version: 2022-11-28" \
		https://api.github.com/repos/$(REPO_OWNER)/$(REPO_NAME)/releases \
	  	-d '{"tag_name":"v$(version)","target_commitish":"master","name":"v$(version)"}' | jq .id))
	@curl -X POST \
		-H "Accept: application/vnd.github+json" \
		-H "Authorization: Bearer $(github_token)" \
		-H "X-GitHub-Api-Version: 2022-11-28" \
		-H "Content-Type: application/octet-stream" \
		"https://uploads.github.com/repos/$(REPO_OWNER)/$(REPO_NAME)/releases/$(release_id)/assets?name=$(REPO_NAME)-amd64-linux-musl" \
  		--data-binary "@target/x86_64-unknown-linux-musl/release/$(REPO_NAME)"
