# Day-to-day targets. CI runs `make check` and `make test`.
.PHONY: build test test-render bench lint fmt check layers check-cross doc pre-publish publish clean

build:
	cargo build

# Offscreen render tests draw on whatever GPU adapter is found (Vulkan, GL,
# or a software one) and skip, saying why, when there is none.
test:
	cargo test --all-features

# The same tests on GL through Mesa's software rasterizer, on a virtual
# display, where a missing adapter fails them instead of skipping. Mesa's
# EGL is named because a GPU vendor's EGL may be picked first and refuse to
# open a device without a window.
MESA_EGL_VENDOR ?= /usr/share/glvnd/egl_vendor.d/50_mesa.json

test-render:
	__EGL_VENDOR_LIBRARY_FILENAMES=$(MESA_EGL_VENDOR) LIBGL_ALWAYS_SOFTWARE=1 \
		WGPU_BACKEND=gl MISTY_REQUIRE_GPU=1 xvfb-run -a cargo test --all-features

# Frame timings of the heaviest views in a release build, on the same
# software GL as test-render so numbers compare across machines.
bench:
	__EGL_VENDOR_LIBRARY_FILENAMES=$(MESA_EGL_VENDOR) LIBGL_ALWAYS_SOFTWARE=1 \
		WGPU_BACKEND=gl xvfb-run -a cargo test --release -p misty --all-features \
		-- --ignored --nocapture --test-threads=1 bench_

lint:
	cargo clippy --all-targets --all-features -- -D warnings

fmt:
	cargo fmt --all

check:
	cargo fmt --all --check
	$(MAKE) lint
	$(MAKE) layers

# The module layering rules of tests/layers.rs: no module
# imports from a layer above its own, winit stays in the lower layers and
# the runtime, and the base layer imports no GPU, window, or other GUI
# crate.
layers:
	cargo test -p misty --test layers

# Type-checks the workspace, tests and examples included, for Windows and
# macOS. Each target must be installed for the toolchain used (`rustup
# target add`); TOOLCHAIN picks another one, such as `TOOLCHAIN=+nightly`.
# Built under target/cross so the host build is kept.
CROSS_TARGETS ?= x86_64-pc-windows-gnu aarch64-apple-darwin
TOOLCHAIN ?=

# Apple targets check every feature but `notify`: its macOS backend builds
# Objective-C in a build script, which needs Apple's own compiler.
APPLE_FEATURES := dialogs,global-hotkey,kdl,single-instance,testing,tray

check-cross:
	for target in $(CROSS_TARGETS); do \
		case $$target in \
			*-apple-*) features="--features $(APPLE_FEATURES)" ;; \
			*) features="--all-features" ;; \
		esac; \
		cargo $(TOOLCHAIN) check --all-targets $$features \
			--target $$target --target-dir target/cross || exit 1; \
	done

# Every feature, as docs.rs builds it (`[package.metadata.docs.rs]`).
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

# The version being released, read from Cargo.toml.
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

# Everything a release must pass, without uploading: a clean tree, a
# CHANGELOG section for VERSION, no tag for it yet, the checks, the tests
# (render tests included, failing without an adapter), the Windows and
# macOS builds, the docs as docs.rs builds them, and a dry-run publish of
# the packaged crate.
pre-publish:
	@test -z "$$(git status --porcelain)" || \
		{ echo "pre-publish: commit or stash the changes first"; exit 1; }
	@grep -q '^## \[$(VERSION)\]' CHANGELOG.md || \
		{ echo "pre-publish: add a '## [$(VERSION)]' section to CHANGELOG.md"; exit 1; }
	@! git rev-parse -q --verify "refs/tags/v$(VERSION)" >/dev/null || \
		{ echo "pre-publish: tag v$(VERSION) exists; bump the version in Cargo.toml"; exit 1; }
	$(MAKE) check
	$(MAKE) test-render
	$(MAKE) check-cross
	$(MAKE) doc
	cargo package --list
	cargo publish --dry-run

# Uploads VERSION to crates.io after pre-publish passes, then tags the
# commit and pushes the tag. A published version cannot be replaced, only
# yanked.
publish: pre-publish
	cargo publish
	git tag -a "v$(VERSION)" -m "misty $(VERSION)"
	git push origin "v$(VERSION)"

clean:
	cargo clean
