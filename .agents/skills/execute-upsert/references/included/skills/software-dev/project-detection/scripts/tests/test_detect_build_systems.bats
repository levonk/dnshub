#!/usr/bin/env bats
# test_detect_build_systems.bats — unit tests for detect-build-systems.sh
# Tests build system detection (npm, cargo, maven, swift, etc.) and
# environment wrapper detection (devbox, nix, mise) via cli-tool-discovery.sh.
#
# Run via:
#   bats scripts/tests/test_detect_build_systems.bats
#
# Creates temp scenarios under /tmp/skill-test/project-detection/{scenario}/

SCRIPT_DIR="$(cd "$(dirname "${BATS_TEST_FILENAME:-${BASH_SOURCE[0]}}")" && pwd)"
DETECT_SCRIPT="$SCRIPT_DIR/../detect-build-systems.sh"
EXTRACT_SCRIPT="$SCRIPT_DIR/../extract-build-targets.sh"
CLI_TOOL_DISCOVERY="$SCRIPT_DIR/../cli-tool-discovery.sh"
TEST_BASE="/tmp/skill-test/project-detection"

# Speed up cli-tool-discovery.sh probes and skip installs so tests don't hang
# on devbox run -- (which blocks when the temp dir's devbox.json has no real
# nix packages). 2-second probe timeout is enough for detection; install
# fallbacks are unnecessary in test contexts.
export DEVBOX_PROBE_TIMEOUT_SECS=2
export CLTOOL_PROBE_TIMEOUT_SECS=2
export CLTOOL_INSTALL_DISABLED=1

assert_contains() {
    local needle="$1" haystack="$2"
    [[ "$haystack" == *"$needle"* ]] || {
        echo "expected '$needle' in: '$haystack'" >&3
        return 1
    }
}
assert_not_contains() {
    local needle="$1" haystack="$2"
    [[ "$haystack" != *"$needle"* ]] || {
        echo "did not expect '$needle' in: '$haystack'" >&3
        return 1
    }
}

# Create a scenario dir with specific files. Echoes the dir path.
# Config files that require valid content for wrapper detection (devbox.json,
# flake.nix, shell.nix, etc.) are written with minimal valid content instead
# of empty `touch`. Build-system files (package.json, Cargo.toml, etc.) are
# still `touch`ed — detection only checks file existence for those.
setup_scenario() {
    local scenario="$1"; shift
    local dir="$TEST_BASE/$scenario"
    rm -rf "$dir"
    mkdir -p "$dir"
    for f in "$@"; do
        mkdir -p "$dir/$(dirname "$f")"
        case "$f" in
        devbox.json)
            printf '{"packages":[]}\n' > "$dir/$f"
            ;;
        flake.nix|shell.nix|flox.nix)
            printf '{ description = "test"; }\n' > "$dir/$f"
            ;;
        .envrc)
            printf 'export PATH=$PATH:$PWD/bin\n' > "$dir/$f"
            ;;
        mise.toml|.mise.toml|.mise/config.toml)
            printf '[tools]\n' > "$dir/$f"
            ;;
        *)
            touch "$dir/$f"
            ;;
        esac
    done
    echo "$dir"
}

# Run detect-build-systems.sh from a given dir. Echoes detected systems.
# Unsets DEVBOX_SHELL_ENABLED and IN_NIX_SHELL so cli-tool-discovery.sh
# doesn't short-circuit on "already inside devbox/nix" when bats itself
# runs via `devbox run --`.
run_detect() {
    local dir="$1"
    (cd "$dir" && env -u DEVBOX_SHELL_ENABLED -u IN_NIX_SHELL bash "$DETECT_SCRIPT" "$dir") 2>/dev/null || true
}

# Run extract-build-targets.sh show from a given dir.
run_show_targets() {
    local dir="$1"
    (cd "$dir" && bash "$EXTRACT_SCRIPT" show "$dir") 2>/dev/null || true
}

# --- build system detection tests ---

@test "npm: detects package.json" {
    local dir; dir="$(setup_scenario npm package.json)"
    local out; out="$(run_detect "$dir")"
    assert_contains "npm" "$out"
}

@test "cargo: detects Cargo.toml" {
    local dir; dir="$(setup_scenario cargo Cargo.toml)"
    local out; out="$(run_detect "$dir")"
    assert_contains "cargo" "$out"
}

@test "go: detects go.mod" {
    local dir; dir="$(setup_scenario go go.mod)"
    local out; out="$(run_detect "$dir")"
    assert_contains "go" "$out"
}

@test "maven: detects pom.xml" {
    local dir; dir="$(setup_scenario maven pom.xml)"
    local out; out="$(run_detect "$dir")"
    assert_contains "maven" "$out"
}

@test "gradle: detects build.gradle" {
    local dir; dir="$(setup_scenario gradle build.gradle)"
    local out; out="$(run_detect "$dir")"
    assert_contains "gradle" "$out"
}

@test "python: detects pyproject.toml" {
    local dir; dir="$(setup_scenario python pyproject.toml)"
    local out; out="$(run_detect "$dir")"
    assert_contains "python" "$out"
}

@test "swift: detects Package.swift" {
    local dir; dir="$(setup_scenario swift Package.swift)"
    local out; out="$(run_detect "$dir")"
    assert_contains "swift" "$out"
}

@test "make: detects Makefile" {
    local dir; dir="$(setup_scenario make Makefile)"
    local out; out="$(run_detect "$dir")"
    assert_contains "make" "$out"
}

@test "just: detects Justfile" {
    local dir; dir="$(setup_scenario just Justfile)"
    local out; out="$(run_detect "$dir")"
    assert_contains "just" "$out"
}

@test "dotnet: detects *.csproj" {
    local dir; dir="$(setup_scenario dotnet MyProject.csproj)"
    local out; out="$(run_detect "$dir")"
    assert_contains "dotnet" "$out"
}

@test "ruby: detects Gemfile" {
    local dir; dir="$(setup_scenario ruby Gemfile)"
    local out; out="$(run_detect "$dir")"
    assert_contains "ruby" "$out"
}

@test "elixir: detects mix.exs" {
    local dir; dir="$(setup_scenario elixir mix.exs)"
    local out; out="$(run_detect "$dir")"
    assert_contains "elixir" "$out"
}

@test "docker: detects Dockerfile" {
    local dir; dir="$(setup_scenario docker Dockerfile)"
    local out; out="$(run_detect "$dir")"
    assert_contains "docker" "$out"
}

@test "multi: detects npm, cargo, make" {
    local dir; dir="$(setup_scenario multi package.json Cargo.toml Makefile)"
    local out; out="$(run_detect "$dir")"
    assert_contains "npm" "$out"
    assert_contains "cargo" "$out"
    assert_contains "make" "$out"
}

@test "empty: no false positives" {
    local dir; dir="$(setup_scenario empty)"
    local out; out="$(run_detect "$dir")"
    # Should be empty or just whitespace
    assert_not_contains "npm" "$out"
    assert_not_contains "cargo" "$out"
}

# --- environment wrapper detection tests (via cli-tool-discovery.sh) ---

# Helper: check if devbox is functional (returns 0=yes, 1=no). No skip —
# callers decide whether to skip or branch on the result.
# Reuses cli-tool-discovery.sh's --detect-wrapper mode, which probes devbox
# with `devbox run -- true` (hang-safe via devbox_run_timed). No duplicated
# probe logic here — the include's hardened timeout/kill path is the single
# source of truth for devbox functionality checks.
devbox_is_functional() {
    command -v devbox >/dev/null 2>&1 || return 1
    local probe_dir; probe_dir="$(mktemp -d)"
    printf '{"packages":[]}\n' > "$probe_dir/devbox.json"
    local result
    result="$(cd "$probe_dir" && env -u DEVBOX_SHELL -u IN_DEVBOX_SHELL \
        DEVBOX_PROBE_TIMEOUT_SECS=5 bash "$CLI_TOOL_DISCOVERY" --detect-wrapper 2>/dev/null)" || true
    rm -rf "$probe_dir"
    [[ "$result" == "WRAPPER: devbox run --" ]]
}

# Helper: skip if devbox is not functional. Wraps devbox_is_functional with
# a skip call for tests that require devbox to work.
skip_unless_devbox_functional() {
    devbox_is_functional || skip "devbox not functional on this platform"
}

@test "devbox: detects devbox.json" {
    skip_unless_devbox_functional
    local dir; dir="$(setup_scenario devbox devbox.json)"
    local out; out="$(run_detect "$dir")"
    assert_contains "devbox" "$out"
}

@test "nix: detects flake.nix" {
    command -v nix >/dev/null 2>&1 || skip "nix not installed"
    nix --version >/dev/null 2>&1 || skip "nix not functional on this platform"
    local dir; dir="$(setup_scenario nix flake.nix)"
    local out; out="$(run_detect "$dir")"
    assert_contains "nix" "$out"
}

@test "mise: detects mise.toml" {
    command -v mise >/dev/null 2>&1 || skip "mise not installed"
    local dir; dir="$(setup_scenario mise mise.toml)"
    local out; out="$(run_detect "$dir")"
    assert_contains "mise" "$out"
}

@test "no wrapper when tool missing" {
    # If devbox isn't installed or not functional, devbox.json should not
    # trigger detection. This tests both cases:
    #   - devbox functional → "devbox" in output (detection works)
    #   - devbox broken/missing → "devbox" NOT in output (no false positive)
    local dir; dir="$(setup_scenario no-devbox-tool devbox.json)"
    local out; out="$(run_detect "$dir")"
    if devbox_is_functional; then
        assert_contains "devbox" "$out"
    else
        assert_not_contains "devbox" "$out"
    fi
}

# --- extract-build-targets tests ---

@test "swift targets: shows build and test" {
    local dir; dir="$(setup_scenario swift-targets Package.swift)"
    local out; out="$(run_show_targets "$dir")"
    assert_contains "build" "$out"
    assert_contains "test" "$out"
}

@test "cargo targets: shows build and test" {
    local dir; dir="$(setup_scenario cargo-targets Cargo.toml)"
    local out; out="$(run_show_targets "$dir")"
    assert_contains "build" "$out"
    assert_contains "test" "$out"
}

@test "makefile targets: shows build, test, lint" {
    local dir
    dir="$TEST_BASE/makefile-targets"
    rm -rf "$dir"; mkdir -p "$dir"
    cat > "$dir/Makefile" <<'EOF'
build:
	echo build
test:
	echo test
lint:
	echo lint
EOF
    local out; out="$(run_show_targets "$dir")"
    assert_contains "build" "$out"
    assert_contains "test" "$out"
    assert_contains "lint" "$out"
}

@test "maven targets: shows compile/test" {
    local dir; dir="$(setup_scenario maven-targets pom.xml)"
    local out; out="$(run_show_targets "$dir")"
    assert_contains "test" "$out"
}

@test "gradle targets: shows build/test" {
    local dir; dir="$(setup_scenario gradle-targets build.gradle)"
    local out; out="$(run_show_targets "$dir")"
    assert_contains "test" "$out"
}

@test "devbox targets: shows build and test" {
    local dir
    dir="$TEST_BASE/devbox-targets"
    rm -rf "$dir"; mkdir -p "$dir"
    cat > "$dir/devbox.json" <<'EOF'
{
  "packages": ["python3@latest"],
  "scripts": {
    "build": "echo build",
    "test": "echo test"
  }
}
EOF
    local out; out="$(run_show_targets "$dir")"
    assert_contains "build" "$out"
    assert_contains "test" "$out"
}
