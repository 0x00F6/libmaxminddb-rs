SHELL := /usr/bin/env bash
CARGO ?= cargo
FUZZ_CORPUS ?= target/fuzz-corpus
GITHUB_REPO ?= 0x00F6/libmaxminddb-rs
BENCH_DOCKER_CONTEXT ?= default

.PHONY: build build-release test test-all coverage code-coverage compatibility bench bench-compare bench-compare-docker bench-performance micro-benchmark \
        bench-reader bench-writer fmt fmt-check clippy doc doc-open audit clean check flamegraph callgrind perf perf-tree fuzz \
        publish-check publish release

build:
	$(CARGO) build --all-features

build-release:
	$(CARGO) build --release --all-features

test:
	@mkdir -p target
	@rm -f target/test_results.xml target/test_output.log target/test-results.json
	@set -o pipefail; \
	if command -v cargo-nextest >/dev/null 2>&1; then \
		echo "==> cargo nextest run --all-features --profile ci --no-fail-fast"; \
		cargo nextest run --all-features --profile ci --no-fail-fast 2>&1 | tee target/test_output.log; \
	else \
		echo "==> $(CARGO) test --all-features"; \
		$(CARGO) test --all-features 2>&1 | tee target/test_output.log; \
	fi; \
	status=$$?; \
	$(CARGO) run --release --manifest-path scripts/Cargo.toml --bin generate_test_report || true; \
	echo; \
	echo "Test report: file://$(CURDIR)/test-report/index.html"; \
	exit $$status

test-all:
	$(CARGO) test --workspace --all-features
	$(CARGO) test --doc --all-features
	$(CARGO) test --no-default-features --features reader
	$(CARGO) test --no-default-features --features writer
	$(CARGO) test --no-default-features --features reader,writer,derive
	$(CARGO) test --manifest-path tools/rust-competitor-bench/Cargo.toml --features ours
	$(CARGO) test --manifest-path scripts/Cargo.toml
	$(MAKE) code-coverage

# Uses the immutable fixtures from bench-compare and its --ipv6-absent-only run.
bench-performance:
	$(CARGO) bench --bench reader --bench lookup_borrowed

fuzz:
	@if ! $(CARGO) fuzz --version >/dev/null 2>&1; then \
		echo "cargo-fuzz and a nightly Rust toolchain are required" >&2; exit 2; \
	fi
	@mkdir -p $(FUZZ_CORPUS)/reader_from_bytes
	$(CARGO) run --quiet --release --manifest-path tools/gen-bench-data/Cargo.toml -- $(FUZZ_CORPUS)/reader_from_bytes/valid.mmdb
	$(CARGO) +nightly fuzz run reader_from_bytes $(FUZZ_CORPUS)/reader_from_bytes -- -max_total_time=30

coverage:
	@if ! $(CARGO) llvm-cov --version >/dev/null 2>&1; then \
		echo "cargo-llvm-cov is required: cargo install cargo-llvm-cov" >&2; exit 2; \
	fi
	$(CARGO) llvm-cov --all-features --workspace --html --output-dir target/coverage
	$(CARGO) llvm-cov report --json --summary-only --output-path target/coverage/summary.json -p libmaxminddb-rs -p libmaxminddb-rs-derive
	@$(CARGO) test --workspace --all-features --lib --tests -- --list > target/coverage/test-list.txt
	@echo "HTML coverage report: file://$(CURDIR)/target/coverage/html/index.html"
	@$(CARGO) run --quiet --manifest-path scripts/Cargo.toml --bin update_readme_coverage -- target/coverage/summary.json target/coverage/test-list.txt

code-coverage: coverage

compatibility:
	@mkdir -p target
	$(CARGO) run --quiet --release --example generate_compat_db -- target/libmaxminddb-rs-compat.mmdb
	@if command -v mmdblookup >/dev/null 2>&1; then \
		echo "==> Reading libmaxminddb-rs output with libmaxminddb/mmdblookup..."; \
		mmdblookup --file target/libmaxminddb-rs-compat.mmdb --ip 203.0.113.7 country iso_code; \
	else \
		echo "ℹ️ mmdblookup is not installed; writer->libmaxminddb check skipped."; \
	fi
	@if [ -n "$${MMDB_COMPAT_DB:-}" ]; then \
		echo "==> Reading external MMDB with libmaxminddb-rs..."; \
		$(CARGO) test --test compatibility --features reader -- --nocapture; \
	else \
		echo "ℹ️ Set MMDB_COMPAT_DB to run an external MMDB reader compatibility test."; \
	fi

bench: bench-compare

bench-compare:
	@mkdir -p target
	$(CARGO) run --release --manifest-path scripts/Cargo.toml --bin run_benchmarks_compare

bench-compare-docker:
	@mkdir -p target/docker-bench benchmark-report benchmarks/charts docs/images/benchmarks
	@BENCH_UID="$$(id -u)" BENCH_GID="$$(id -g)" docker --context "$(BENCH_DOCKER_CONTEXT)" compose run --build --rm bench-compare
	@host_root="$$(pwd -P)"; \
	printf '\nBenchmark artifacts on the host:\n'; \
	printf '  HTML report: %s/benchmark-report/index.html\n' "$$host_root"; \
	printf '  SVG charts: %s/benchmarks/charts/\n' "$$host_root"; \
	printf '  README charts: %s/docs/images/benchmarks/\n' "$$host_root"; \
	printf '  JSON results: %s/target/docker-bench/benchmark-results/results.json\n' "$$host_root"; \
	printf '  CSV results: %s/target/docker-bench/benchmark-results/results.csv\n' "$$host_root"; \
	printf '  Raw results: %s/target/docker-bench/benchmark-results/results.jsonl\n' "$$host_root"; \
	printf '  README: %s/README.md\n' "$$host_root"

micro-benchmark:
	$(CARGO) run --release --manifest-path scripts/Cargo.toml --bin run_micro_benchmarks -- "$(CARGO)"

bench-reader:
	$(CARGO) bench --bench reader

bench-writer:
	$(CARGO) bench --bench writer

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets --all-features

doc:
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --workspace --all-features --no-deps

doc-open:
	$(CARGO) doc --workspace --all-features --no-deps --open

audit:
	@if $(CARGO) audit --version >/dev/null 2>&1; then $(CARGO) audit; else echo "cargo-audit is not installed"; exit 2; fi

check: fmt-check clippy test doc

# The local patch lets Cargo verify the main crate before the derive crate has
# appeared on crates.io. Published manifests still depend on its registry version.
publish-check:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings
	$(CARGO) test --workspace --all-features
	$(CARGO) test --doc --all-features
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --no-deps --all-features
	$(CARGO) package --manifest-path derive/Cargo.toml --allow-dirty
	$(CARGO) package --allow-dirty --config 'patch.crates-io.libmaxminddb-rs-derive.path="derive"'

# The derive package must be available in the registry before the main crate.
# Run only after checking both package archives and the release notes.
publish: publish-check
	@git diff --quiet && git diff --cached --quiet && test -z "$$(git ls-files --others --exclude-standard)" || { echo "Commit all changes before publishing." >&2; exit 1; }
	$(CARGO) publish --manifest-path derive/Cargo.toml
	$(CARGO) publish --package libmaxminddb-rs

# Use `make release -- --force` to replace an existing version tag.
.PHONY: --force
--force:
	@:

release:
	GITHUB_REPO="$(GITHUB_REPO)" bash scripts/release.sh $(if $(filter --force,$(MAKECMDGOALS)),--force,)

flamegraph:
	@mkdir -p target/flamegraph
	@echo "==> Profiling libmaxminddb-rs and generating FlameGraph..."
	$(CARGO) bench --bench flamegraph --all-features
	@cp target/flamegraph/lookup-flamegraph.svg docs/images/lookup-flamegraph.svg
	@echo ""
	@echo "🔥 FlameGraph generation complete:"
	@echo "   SVG:    target/flamegraph/flamegraph.svg"
	@echo "   Lookup SVG: docs/images/lookup-flamegraph.svg"
	@echo "   Report: target/flamegraph/report.md"

callgrind:
	@if command -v valgrind >/dev/null 2>&1; then valgrind --tool=callgrind $(CARGO) bench --bench reader; else echo "valgrind is not installed"; exit 2; fi

perf:
	@if command -v perf >/dev/null 2>&1; then \
		perf record --call-graph dwarf $(CARGO) bench --bench reader || { \
			EXIT_CODE=$$?; \
			if [ -f /proc/sys/kernel/perf_event_paranoid ] && [ "$$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo 0)" -gt 2 ]; then \
				echo ""; \
				echo "⚠️  Linux kernel perf_event_paranoid is set to $$(cat /proc/sys/kernel/perf_event_paranoid)."; \
				echo "To allow unprivileged profiling, run:"; \
				echo "    sudo sysctl -w kernel.perf_event_paranoid=1"; \
				echo ""; \
			fi; \
			exit $$EXIT_CODE; \
		}; \
	else \
		echo "perf is not installed"; \
		exit 2; \
	fi

perf-tree:
	@mkdir -p target
	$(CARGO) run --release --manifest-path scripts/Cargo.toml --bin perf_tree

clean:
	@if [ -d target/docker-bench ]; then \
		find target/docker-bench -type d ! -perm -u+w -exec chmod u+w {} +; \
	fi
	$(CARGO) clean
	@set -e -o pipefail; find derive scripts fuzz tools -type d -name target -prune -o -type f -name Cargo.toml -print0 | \
		while IFS= read -r -d '' manifest; do \
			crate_dir="$${manifest%/Cargo.toml}"; \
			if [ -d "$$crate_dir/target" ]; then \
				echo "Removing $$crate_dir/target"; \
				rm -rf -- "$$crate_dir/target"; \
			fi; \
		done
	rm -rf target/coverage target/test-results.json target/test_results.xml target/test_output.log target/flamegraph test-report/ benchmark-report/
