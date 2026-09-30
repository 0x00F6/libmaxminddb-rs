SHELL := /usr/bin/env bash
CARGO ?= cargo
DOCKER ?= docker
RUSTUP ?= rustup
TARGET_DIR ?= target
FUZZ_CORPUS ?= $(TARGET_DIR)/fuzz-corpus
GITHUB_REPO ?= 0x00F6/libmaxminddb-rs
BENCH_DOCKER_CONTEXT ?= default
SCRIPTS_MANIFEST := scripts/Cargo.toml
WORKSPACE_FLAGS := --workspace --all-features
COVERAGE_DIR := $(TARGET_DIR)/coverage

.DEFAULT_GOAL := help

# Shared terminal-aware log formatter. Call with emoji, ANSI color code, and text.
define log
{ if [[ -t 1 && -z "$${NO_COLOR:-}" ]]; then printf '\033[1;$(2)m$(1) %s\033[0m\n' "$(3)"; else printf '%s %s\n' '$(1)' "$(3)"; fi; }
endef

# Print an actionable fix after an error, using the same terminal styling.
define explain_error
$(call log,❌,31,$(1)) >&2; printf '   Fix: %s\n' '$(2)' >&2
endef

define print_logo
echo -e "\033[0;97m┌─\033[0;37m──\033[0;97m┐\033[0;37m \033[0;97m┌\033[0;37m───\033[0;90m┐\033[0;37m \033[0;97m┌─\033[0;37m───\033[0;90m┐\033[0;37m  \033[0;97m┌─\033[0;37m──\033[0;90m┬\033[0;37m───\033[0;90m┐\033[0;37m \033[0;97m┌───\033[0;37m──\033[0;90m┐\033[0;37m  \033[0;97m┌─\033[0;37m─┐┌──\033[0;90m┐\033[0;97m┌─\033[0;37m──\033[0;90m┬\033[0;37m───\033[0;90m┐\033[0;97m┌\033[0;37m───\033[0;90m┐\033[0;37m \033[0;97m┌─\033[0;37m─┐ ┌──\033[0;90m┐\033[0;97m┌─\033[0;37m─────\033[0;90m┐\033[0;37m \033[0;97m┌─\033[0;37m─────\033[0;90m┐\033[0;37m \033[0;97m┌─\033[0;37m───\033[0;90m┐\033[0;37m       \033[0;97m┌──\033[0;37m────\033[0;90m┐\033[0;37m \033[0;97m┌─\033[0;37m─────\033[0;90m┐\033[0;37m \033[0m"
echo -e "\033[0;37m│   \033[0;90m│\033[0;37m ├───\033[0;90m┐\033[0;37m │ \033[0;90m┌\033[0;37m─┐│  │ ─┐ ┌\033[0;90m─\033[0;37m \033[0;90m│\033[0;97m┌\033[0;90m┘\033[0;37m \033[0;90m┌─\033[0;37m┐ ┴┐ └┐ └\033[0;97m┘\033[0;37m \033[0;90m┌┘\033[0;37m│ ─┐ ┌\033[0;90m─\033[0;37m \033[0;90m│\033[0;37m├───\033[0;90m┐\033[0;37m │  └\033[0;90m┐\033[0;37m│  \033[0;90m│\033[0;37m│   \033[0;90m┐\033[0;37m  \033[0;90m└┐\033[0;37m│   \033[0;90m┐\033[0;37m  \033[0;90m└┐\033[0;37m│ \033[0;90m┌\033[0;37m─┐│       │  \033[0;90m──\033[0;97m┘\033[0;37m \033[0;90m│\033[0;97m┐\033[0;37m│   \033[0;97m──\033[0;37m─┴\033[0;90m┐\033[0m"
echo -e "\033[0;37m│   \033[0;90m│\033[0;37m \033[0;90m│\033[0;37m   \033[0;90m│\033[0;37m \033[0;90m│\033[0;37m └─\033[0;97m┘\033[0;90m└\033[0;37m─\033[0;97m┐\033[0;90m│\033[0;37m  \033[0;97m└\033[0;37m─\033[0;90m┘\033[0;37m  \033[0;90m││\033[0;37m  \033[0;90m└\033[0;37m─\033[0;97m┘\033[0;37m  \033[0;90m└┐\033[0;97m┌\033[0;37m─\033[0;90m─\033[0;37m  \033[0;90m─\033[0;37m─┐\033[0;90m│\033[0;37m  \033[0;97m└\033[0;37m─\033[0;90m┘\033[0;37m  \033[0;90m││\033[0;37m   \033[0;90m│\033[0;37m \033[0;90m│\033[0;37m  \033[0;90m┌\033[0;37m└\033[0;97m┤\033[0;37m  \033[0;90m││\033[0;37m   │   \033[0;90m││\033[0;37m   │   \033[0;90m││\033[0;37m └─\033[0;97m┘\033[0;90m└\033[0;37m─\033[0;97m┐┌──\033[0;37m─\033[0;97m┐\033[0;90m│\033[0;37m  \033[0;97m┌\033[0;37m─┐  \033[0;90m│├─\033[0;37m───\033[0;90m┐\033[0;37m  \033[0;90m│\033[0m"
echo -e "\033[0;90m│\033[0;37m   \033[0;90m└┐│\033[0;37m  ─\033[0;90m┴┐│\033[0;37m  \033[0;90m└─\033[0;97m┘\033[0;37m \033[0;90m││\033[0;37m  \033[0;90m│\033[0;37m \033[0;97m│\033[0;37m  \033[0;90m││\033[0;37m  \033[0;90m┌─┐\033[0;37m   \033[0;90m│\033[0;37m│  ┌┐  \033[0;90m││\033[0;37m  \033[0;90m│\033[0;37m \033[0;97m│\033[0;37m  \033[0;90m││\033[0;37m  ─\033[0;90m┴┐│\033[0;37m  \033[0;90m│└\033[0;37m┐  \033[0;90m││\033[0;37m   \033[0;97m┘\033[0;37m  \033[0;90m┌┘│\033[0;37m   \033[0;97m┘\033[0;37m  \033[0;90m┌┘│\033[0;37m  \033[0;90m└─\033[0;97m┘\033[0;37m \033[0;90m│\033[0;37m└\033[0;90m───\033[0;37m┘\033[0;90m│\033[0;37m  ││   \033[0;90m││\033[0;37m   \033[0;90m─┘\033[0;37m  \033[0;90m│\033[0m"
echo -e "\033[0;90m└────┘└────┘└──────┘└──┘ \033[0;37m└\033[0;90m──┘└──┘\033[0;37m \033[0;90m└───┘└──┘\033[0;97m└\033[0;90m──┘└──┘ \033[0;37m└\033[0;90m──┘└────┘└─\033[0;37m─┘ └\033[0;90m──┘└──────┘\033[0;37m \033[0;90m└──────┘\033[0;37m \033[0;90m└──────┘\033[0;37m     \033[0;90m└──┘└───┘└───────┘\033[0m"
endef

.PHONY: help build build-release tests check coverage compatibility fuzz fmt fmt-check clippy audit doc doc-open \
        bench-reader bench-writer bench-performance bench-compare bench-compare-docker bench-micro \
        flamegraph callgrind perf perf-tree upgrade-deps publish-check release clean --force

# `##@ Emoji Section|ANSI-code` groups help entries; `target: ## text` documents them.
##@ 🛠️ Development|96
help: ## 💡 Show this command list (also the default)
	@$(call print_logo)
	@ansi=0; if [[ -t 1 && -z "$${NO_COLOR:-}" ]]; then ansi=1; fi; \
	awk -v ansi="$$ansi" '\
		BEGIN { esc=sprintf("%c", 27); reset=(ansi ? esc "[0m" : "") } \
		/^##@ / { split(substr($$0, 5), group, "|"); section=group[1]; if (!(section in seen)) { order[++count]=section; seen[section]=1; colors[section]=(ansi ? esc "[1;" group[2] "m" : "") }; next } \
		/^[[:alnum:]_.-]+:.*## / { target=$$1; sub(/:.*/, "", target); description=$$0; sub(/^.*## /, "", description); label=target; if (target=="release") label="release X.Y.Z"; lines[section]=lines[section] sprintf("  %s%-29s%s %s\n", colors[section], "make " label, reset, description) } \
		END { for (i=1; i<=count; i++) { section=order[i]; printf "\n%s━━ %s ━━━━━━━━━━━━━━━━━━━━━━━━━━━%s\n%s", colors[section], section, reset, lines[section] } }' $(MAKEFILE_LIST)

##@ 🛠️ Development|96
# Upgrade every tracked Cargo manifest, test the result, and restore files on failure.
upgrade-deps: ## 📦 Upgrade dependencies and test; restore manifests on failure
	@CARGO="$(CARGO)" bash scripts/upgrade-deps.sh

##@ 🔨 Build|94
# Compile every feature in the development profile.
build: ## 🔨 Compile all features
	$(CARGO) build --all-features

# Compile every feature with release optimizations.
build-release: ## 🚀 Compile all features with release optimizations
	$(CARGO) build --release --all-features

##@ 🧪 Test|92
# Run workspace, documentation, feature-isolation, tooling, and coverage tests.
# Save the primary test output for the generated HTML report.
tests: ## 🧪 Run the complete test suite and coverage
	@$(call log,🧪,36,Running the full test suite)
	@mkdir -p $(TARGET_DIR)
	@rm -f $(TARGET_DIR)/test_results.xml $(TARGET_DIR)/test_output.log $(TARGET_DIR)/test-results.json
	@set -o pipefail; \
	if command -v cargo-nextest >/dev/null 2>&1; then \
		$(call log,🚀,36,Running workspace tests with nextest); \
		$(CARGO) nextest run $(WORKSPACE_FLAGS) --profile ci --no-fail-fast 2>&1 | tee $(TARGET_DIR)/test_output.log; \
	else \
		$(call log,🚀,36,Running workspace tests with cargo); \
		$(CARGO) test $(WORKSPACE_FLAGS) 2>&1 | tee $(TARGET_DIR)/test_output.log; \
	fi; \
	status=$$?; \
	$(CARGO) run --release --manifest-path $(SCRIPTS_MANIFEST) --bin generate_test_report || true; \
	$(call log,📄,32,Test report: file://$(CURDIR)/test-report/index.html); \
	exit $$status

	@$(call log,📚,36,Running documentation tests)
	$(CARGO) test --doc --all-features
	@$(call log,🧩,36,Testing isolated feature combinations)
	$(CARGO) test --no-default-features --features reader
	$(CARGO) test --no-default-features --features writer
	$(CARGO) test --no-default-features --features reader,writer,derive
	@$(call log,🔬,36,Testing benchmark tooling)
	$(CARGO) test --manifest-path tools/rust-competitor-bench/Cargo.toml --features ours
	$(CARGO) test --manifest-path $(SCRIPTS_MANIFEST)
	@$(call log,📊,36,Generating coverage report)
	$(MAKE) coverage

# Seed and run the parser fuzzer for a bounded interval.
fuzz: ## 🐛 Fuzz parser inputs for 30 seconds
	@missing=0; \
	if ! $(CARGO) fuzz --version >/dev/null 2>&1; then \
		$(call explain_error,cargo-fuzz is missing,cargo install cargo-fuzz); missing=1; \
	fi; \
	if ! command -v $(RUSTUP) >/dev/null 2>&1; then \
		$(call explain_error,rustup is missing,Install rustup from https://rustup.rs then run: rustup toolchain install nightly); missing=1; \
	elif ! RUSTUP_AUTO_INSTALL=0 $(RUSTUP) run nightly rustc --version >/dev/null 2>&1; then \
		$(call explain_error,the nightly Rust toolchain is missing,rustup toolchain install nightly); missing=1; \
	fi; \
	if [[ "$$missing" -ne 0 ]]; then exit 2; fi
	@mkdir -p $(FUZZ_CORPUS)/reader_from_bytes
	$(CARGO) run --quiet --release --manifest-path tools/gen-bench-data/Cargo.toml -- $(FUZZ_CORPUS)/reader_from_bytes/valid.mmdb
	$(CARGO) +nightly fuzz run reader_from_bytes $(FUZZ_CORPUS)/reader_from_bytes -- -max_total_time=30

# Generate instrumented coverage and refresh the README summary.
coverage: ## 📊 Generate HTML coverage and refresh README coverage summary
	@if ! $(CARGO) llvm-cov --version >/dev/null 2>&1; then \
		$(call explain_error,cargo-llvm-cov is missing,cargo install cargo-llvm-cov --locked); exit 2; \
	fi
	$(CARGO) llvm-cov $(WORKSPACE_FLAGS) --html --output-dir $(COVERAGE_DIR)
	$(CARGO) llvm-cov report --json --summary-only --output-path $(COVERAGE_DIR)/summary.json -p libmaxminddb-rs -p libmaxminddb-rs-derive
	@$(CARGO) test $(WORKSPACE_FLAGS) --lib --tests -- --list > $(COVERAGE_DIR)/test-list.txt
	@$(call log,📊,32,HTML coverage report: file://$(CURDIR)/$(COVERAGE_DIR)/html/index.html)
	@$(CARGO) run --quiet --manifest-path $(SCRIPTS_MANIFEST) --bin update_readme_coverage -- $(COVERAGE_DIR)/summary.json $(COVERAGE_DIR)/test-list.txt

# Check writer output with mmdblookup and optional external reader input.
compatibility: ## 🔗 Check MMDB interoperability with external readers
	@mkdir -p $(TARGET_DIR)
	$(CARGO) run --quiet --release --example generate_compat_db -- $(TARGET_DIR)/libmaxminddb-rs-compat.mmdb
	@if command -v mmdblookup >/dev/null 2>&1; then \
		$(call log,🔎,36,Reading libmaxminddb-rs output with mmdblookup); \
		mmdblookup --file $(TARGET_DIR)/libmaxminddb-rs-compat.mmdb --ip 203.0.113.7 country iso_code; \
	else \
		$(call log,ℹ️,33,mmdblookup is not installed; compatibility check skipped); \
	fi
	@if [ -n "$${MMDB_COMPAT_DB:-}" ]; then \
		$(call log,🔎,36,Reading external MMDB with libmaxminddb-rs); \
		$(CARGO) test --test compatibility --features reader -- --nocapture; \
	else \
		$(call log,ℹ️,33,Set MMDB_COMPAT_DB to run the external reader compatibility test); \
	fi

# Check dependency advisories when cargo-audit is installed.
audit: ## 🔍 Check dependency advisories
	@if $(CARGO) audit --version >/dev/null 2>&1; then $(CARGO) audit; else $(call explain_error,cargo-audit is missing,cargo install cargo-audit); exit 2; fi

# Run the standard formatting, lint, test, and documentation gate.
check: fmt-check clippy tests doc ## ✅ Run formatting, lint, tests and documentation checks

##@ 📈 Benchmark|95
# Benchmark the reader with fixtures prepared by the comparison harness.
bench-performance: ## ⚡ Benchmark reader and borrowed lookups
	$(CARGO) bench --bench reader --bench lookup_borrowed

# Run the locally measured cross-implementation benchmark harness.
bench-compare: ## ⚖️ Compare this reader against other implementations
	@mkdir -p $(TARGET_DIR)
	$(CARGO) run --release --manifest-path $(SCRIPTS_MANIFEST) --bin run_benchmarks_compare

# Run detailed lookup scenarios and generate their report.
bench-micro: ## 🔬 Benchmark detailed lookup scenarios
	$(CARGO) run --release --manifest-path $(SCRIPTS_MANIFEST) --bin run_micro_benchmarks -- "$(CARGO)"

# Run only the reader Criterion suite.
bench-reader: ## 📖 Run the reader Criterion benchmark
	$(CARGO) bench --bench reader

# Run only the writer Criterion suite.
bench-writer: ## ✍️ Run the writer Criterion benchmark
	$(CARGO) bench --bench writer

# Profile lookup behavior and copy the generated SVG into docs.
flamegraph: ## 🔥 Record a lookup flame graph
	@mkdir -p $(TARGET_DIR)/flamegraph
	@$(call log,🔥,36,Profiling libmaxminddb-rs and generating FlameGraph)
	$(CARGO) bench --bench flamegraph --all-features
	@cp $(TARGET_DIR)/flamegraph/lookup-flamegraph.svg docs/images/lookup-flamegraph.svg
	@$(call log,✅,32,FlameGraph generation complete)
	@printf '   SVG:    %s\n   Lookup SVG: %s\n   Report: %s\n' $(TARGET_DIR)/flamegraph/flamegraph.svg docs/images/lookup-flamegraph.svg $(TARGET_DIR)/flamegraph/report.md

# Collect instruction counts with Valgrind when available.
callgrind: ## 🧮 Profile reader instructions with Valgrind
	@if command -v valgrind >/dev/null 2>&1; then valgrind --tool=callgrind $(CARGO) bench --bench reader; else $(call explain_error,Valgrind is missing,Install Valgrind with your package manager; on Ubuntu or Debian run: sudo apt install valgrind); exit 2; fi

# Collect a native CPU profile and explain common permission failures.
perf: ## ⏱️ Profile reader CPU usage with Linux perf
	@if command -v perf >/dev/null 2>&1; then \
		perf record --call-graph dwarf $(CARGO) bench --bench reader || { \
			EXIT_CODE=$$?; \
			if [ -f /proc/sys/kernel/perf_event_paranoid ] && [ "$$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo 0)" -gt 2 ]; then \
				$(call log,⚠️,33,Linux perf_event_paranoid blocks unprivileged profiling) >&2; \
				printf '   Current value: %s\n   To enable: sudo sysctl -w kernel.perf_event_paranoid=1\n' "$$(cat /proc/sys/kernel/perf_event_paranoid)" >&2; \
			fi; \
			exit $$EXIT_CODE; \
		}; \
	else \
		$(call explain_error,perf is missing,Install Linux perf with your package manager; on Ubuntu run: sudo apt install linux-tools-common linux-tools-generic); \
		exit 2; \
	fi

# Run the prepared-tree profiling tool.
perf-tree: ## 🌳 Profile prepared tree traversal
	@mkdir -p $(TARGET_DIR)
	$(CARGO) run --release --manifest-path $(SCRIPTS_MANIFEST) --bin perf_tree

##@ 🐳 Docker|93
# Run the comparison in Docker and print host-side artifact paths.
bench-compare-docker: ## 🐳 Run the comparison suite in Docker
	@mkdir -p $(TARGET_DIR)/docker-bench benchmark-report benchmarks/charts docs/images/benchmarks
	@BENCH_UID="$$(id -u)" BENCH_GID="$$(id -g)" $(DOCKER) --context "$(BENCH_DOCKER_CONTEXT)" compose run --build --rm bench-compare
	@host_root="$$(pwd -P)"; \
	$(call log,📦,32,Benchmark artifacts on the host); \
	printf '  HTML report: %s/benchmark-report/index.html\n' "$$host_root"; \
	printf '  SVG charts: %s/benchmarks/charts/\n' "$$host_root"; \
	printf '  README charts: %s/docs/images/benchmarks/\n' "$$host_root"; \
	printf '  JSON results: %s/$(TARGET_DIR)/docker-bench/benchmark-results/results.json\n' "$$host_root"; \
	printf '  CSV results: %s/$(TARGET_DIR)/docker-bench/benchmark-results/results.csv\n' "$$host_root"; \
	printf '  Raw results: %s/$(TARGET_DIR)/docker-bench/benchmark-results/results.jsonl\n' "$$host_root"; \
	printf '  README: %s/README.md\n' "$$host_root"

##@ 🎨 Lint / Format|91
# Apply rustfmt to the workspace.
fmt: ## 🎨 Format the workspace
	$(CARGO) fmt --all

# Check formatting without modifying files.
fmt-check: ## 📐 Check workspace formatting
	$(CARGO) fmt --all -- --check

# Lint all workspace targets with all features.
clippy: ## 🧹 Lint every workspace target
	$(CARGO) clippy $(WORKSPACE_FLAGS) --all-targets

##@ 📚 Documentation|97
# Build rustdoc and fail on documentation warnings.
doc: ## 📚 Build API documentation with warnings denied
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc $(WORKSPACE_FLAGS) --no-deps

# Build and open the local API documentation.
doc-open: ## 🌐 Build and open API documentation
	$(CARGO) doc $(WORKSPACE_FLAGS) --no-deps --open

##@ 🚀 Release|33
# The local patch lets Cargo verify the main crate before the derive crate has
# appeared on crates.io. Published manifests still depend on its registry version.
publish-check: fmt-check ## 📦 Validate both crate packages before publishing
	$(CARGO) clippy $(WORKSPACE_FLAGS) --all-targets -- -D warnings
	$(CARGO) test $(WORKSPACE_FLAGS)
	$(CARGO) test --doc --all-features
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --no-deps --all-features
	$(CARGO) package --manifest-path derive/Cargo.toml --allow-dirty
	$(CARGO) package --allow-dirty --config 'patch.crates-io.libmaxminddb-rs-derive.path="derive"'

# Make treats arguments after `--` as goals; accept this flag for release.
.PHONY: --force
--force:
	@:

# Accept `make release 0.2.1` as a version argument without treating it as a file.
ifneq ($(filter release,$(MAKECMDGOALS)),)
RELEASE_VERSION_GOAL := $(firstword $(filter-out release --force,$(MAKECMDGOALS)))
ifneq ($(RELEASE_VERSION_GOAL),)
.PHONY: $(RELEASE_VERSION_GOAL)
$(RELEASE_VERSION_GOAL):
	@:
endif
endif

# Use `make release 0.2.1 -- --force` to replace an existing version tag.
# Synchronize versions, then push and publish a committed release.
release: ## 🚀 Synchronize, tag, push and publish a version
	CARGO="$(CARGO)" GITHUB_REPO="$(GITHUB_REPO)" bash scripts/release.sh $(RELEASE_VERSION_GOAL) $(if $(filter --force,$(MAKECMDGOALS)),--force,)

##@ 🧹 Cleanup|90
# Remove Cargo outputs and generated reports, including nested tool targets.
clean: ## 🗑️ Remove build outputs and generated reports
	@if [ -d $(TARGET_DIR)/docker-bench ]; then \
		find $(TARGET_DIR)/docker-bench -type d ! -perm -u+w -exec chmod u+w {} +; \
	fi
	$(CARGO) clean
	@set -e -o pipefail; find derive scripts fuzz tools -type d -name target -prune -o -type f -name Cargo.toml -print0 | \
		while IFS= read -r -d '' manifest; do \
			crate_dir="$${manifest%/Cargo.toml}"; \
			if [ -d "$$crate_dir/target" ]; then \
				$(call log,🧹,36,Removing $$crate_dir/target); \
				rm -rf -- "$$crate_dir/target"; \
			fi; \
		done
	rm -rf $(COVERAGE_DIR) $(TARGET_DIR)/test-results.json $(TARGET_DIR)/test_results.xml $(TARGET_DIR)/test_output.log $(TARGET_DIR)/flamegraph test-report/ benchmark-report/
