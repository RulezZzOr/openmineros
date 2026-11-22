BOARD ?= s19-xil
MODEL ?= s19j-pro
VERSION ?= 0.1.0
DIST_DIR ?= dist

BUILDER_IMAGE ?= openmineros-builder

.PHONY: bootstrap fmt clippy test run-control-plane image builder ramdisk verify-repro clean

bootstrap:
	cargo fetch

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

run-control-plane:
	cargo run -p openmineros-control-plane -- --board $(BOARD) --model $(MODEL)

image:
	./scripts/build-image.sh "$(BOARD)" "$(MODEL)" "$(VERSION)" "$(DIST_DIR)"

builder:
	docker build -t $(BUILDER_IMAGE) -f image/builder.Dockerfile image

ramdisk: builder
	docker run --rm -v "$(CURDIR)":/src -w /src \
		-v omo-cargo-registry:/usr/local/cargo/registry \
		-v omo-target:/target -e CARGO_TARGET_DIR=/target \
		-e SOURCE_COMMIT="$$(git rev-parse --short=12 HEAD)$$(git diff --quiet HEAD || echo -dirty)" \
		-e SOURCE_DATE_EPOCH="$$(git log -1 --format=%ct)" \
		$(BUILDER_IMAGE) \
		./scripts/build-ramdisk.sh "$(BOARD)" "$(MODEL)" "$(VERSION)" "$(DIST_DIR)"

verify-repro:
	./scripts/verify-repro.sh "$(BOARD)" "$(MODEL)" "$(VERSION)"

clean:
	cargo clean
	rm -rf "$(DIST_DIR)"

