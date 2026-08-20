# Git repository specific implementations
_log := '
_jv_has() {
  local cat="$1"
  local v="${JUST_LOG:-0}"
  case "$v" in
    1|all) return 0 ;;
    0|"") return 1 ;;
  esac
  v="${v//startend/start,end}"
  echo ",$v," | grep -q ",$cat,"
}
log_info()   { _jv_has info   && echo "$*" || true; }
log_start()  { _jv_has start  && echo "▶ $*" || true; }
log_end()    { _jv_has end    && echo "✔ $*" || true; }
log_status() { _jv_has status && echo "$*" || true; }
log_warn()   { echo "⚠️  $*" >&2; }
log_error()  { echo "❌ $*" >&2; }
log_startend() {
  local msg="$1"; shift
  local rc
  _jv_has start && echo "▶ $msg" || true
  rc=0; "$@" || rc=$?
  _jv_has end && echo "✔ $msg complete" || true
  return $rc
}
'

# Devbox auto-detection: run impl target directly if in devbox,
# re-exec via devbox run if not, or fail with doctor diagnostic.
_devbox target *args:
    #!/usr/bin/env bash
    {{_log}}
    if [ "${DEVBOX_SHELL_ENABLED:-0}" = "1" ]; then
        exec just "{{target}}" {{args}}
    elif command -v devbox >/dev/null 2>&1; then
        exec devbox run -- just "{{target}}" {{args}}
    else
        log_error "devbox not found in PATH."
        log_warn "Running doctor to diagnose environment issues..."
        just doctor 2>/dev/null || true
        exit 1
    fi

# =============================================================================
# Normal targets - Developer interface
# =============================================================================

default:
    @just --list

bootstrap:
    @just _devbox bootstrap_impl

build:
    @just _devbox build_impl

test:
    @just _devbox test_impl

lint:
    @just _devbox lint_impl

typecheck:
    @just _devbox typecheck_impl

dev:
    @just _devbox dev_impl

doctor:
    @just _devbox doctor_impl

clean:
    @just _devbox clean_impl

clean-all:
    @just _devbox clean_all_impl

# Development setup (OPTIONAL)
setup:
    #!/usr/bin/env bash
    set -euo pipefail
    {{_log}}
    # Project-specific setup
    # Configure versioned git hooks (submodule-integrity pre-commit check).
    # scripts/hooks/pre-commit prevents submodules from being tracked as
    # trees (040000) instead of gitlinks (160000). See the hook file header
    # for coexistence notes with the pre-commit framework.
    git config core.hooksPath scripts/hooks
    log_end "Git hooks path configured (core.hooksPath=scripts/hooks)"
    log_end "Development environment ready"

# Git repository specific targets
init-repo:
    #!/usr/bin/env bash
    set -euo pipefail
    {{_log}}
    log_start "Initializing git repository"
    git init
    log_end "Git repository initialized"

setup-remote:
    #!/usr/bin/env bash
    set -euo pipefail
    {{_log}}
    log_start "Setting up remote repository"
    if [ -z "$(git remote get-url origin 2>/dev/null)" ]; then
        log_info "No remote configured. Add one with:"
        echo "   git remote add origin <your-repo-url>"
        echo "   git remote -v"
    else
        log_end "Remote already configured:"
        git remote -v
    fi

create-gh-pages:
    #!/usr/bin/env bash
    set -euo pipefail
    {{_log}}
    log_start "Creating gh-pages branch"
    if ! git rev-parse --verify gh-pages >/dev/null 2>&1; then
        git checkout --orphan gh-pages
        git rm -rf . 2>/dev/null || true
        echo "# dnshub Documentation" > README.md
        git add README.md
        git commit -m "Initial gh-pages commit"
        git checkout main 2>/dev/null || git checkout -b main
        log_end "gh-pages branch created"
    else
        log_info "gh-pages branch already exists"
    fi

check-git-hooks:
    #!/usr/bin/env bash
    set -euo pipefail
    {{_log}}
    log_start "Checking git hooks"
    if [ -d .git/hooks ]; then
        find .git/hooks -name "*" -type f -exec echo "  - {}" \;
        log_end "Git hooks found"
    else
        log_warn "No git hooks found"
    fi

check-gitignore:
    #!/usr/bin/env bash
    set -euo pipefail
    {{_log}}
    log_start "Checking .gitignore"
    if [ -f .gitignore ]; then
        log_end ".gitignore found"
        wc -l .gitignore
    else
        log_warn "No .gitignore found"
    fi

# =============================================================================
# Implementation targets (private)
# =============================================================================

[private]
bootstrap_impl:
    #!/usr/bin/env bash
    set -euo pipefail
    {{_log}}
    # Internal bootstrap logic called by devbox init_hook
    if ! command -v nix >/dev/null 2>&1; then
      log_info "Installing Nix..."
      curl -L https://nixos.org/nix/install | sh -s -- --daemon --yes
      log_warn "Nix installed. Please restart your shell and run 'just bootstrap' again."
      exit 0
    fi
    if ! command -v direnv >/dev/null 2>&1; then
      log_info "Installing direnv via Nix..."
      nix profile add nixpkgs#direnv --extra-experimental-features "nix-command flakes"
      log_end "direnv installed"
    fi
    if ! command -v devbox >/dev/null 2>&1; then
      log_info "Installing devbox via Nix..."
      nix profile add nixpkgs#devbox --extra-experimental-features "nix-command flakes"
      log_end "devbox installed"
    fi
    if ! command -v just >/dev/null 2>&1; then
      log_info "Installing just via Nix..."
      nix profile add nixpkgs#just --extra-experimental-features "nix-command flakes"
      log_end "just installed"
    fi
    if [ -f "./.envrc" ]; then
      direnv allow "."
    fi
    log_end "Project bootstrap complete"

[private]
build_impl:
    #!/usr/bin/env bash
    {{_log}}
    log_info "No build target configured for generic git repository"
    log_info "Add language-specific build commands to your justfile if needed"

[private]
test_impl:
    #!/usr/bin/env bash
    {{_log}}
    log_info "No test target configured for generic git repository"
    log_info "Add language-specific test commands to your justfile if needed"

[private]
lint_impl:
    #!/usr/bin/env bash
    {{_log}}
    log_info "No lint target configured for generic git repository"
    log_info "Add language-specific lint commands to your justfile if needed"

[private]
typecheck_impl:
    #!/usr/bin/env bash
    {{_log}}
    log_info "No typecheck target configured for generic git repository"
    log_info "Add language-specific typecheck commands to your justfile if needed"

[private]
dev_impl:
    #!/usr/bin/env bash
    {{_log}}
    log_info "No dev target configured for generic git repository"
    log_info "Add language-specific dev commands to your justfile if needed"

[private]
doctor_impl:
    #!/usr/bin/env bash
    set -euo pipefail
    MISSING=0
    for tool in nix direnv devbox git just; do
      if command -v $tool >/dev/null 2>&1; then
        printf "[doctor] %-8s OK\n" "$tool"
      else
        echo "[doctor] MISSING: $tool"
        MISSING=$((MISSING+1))
      fi
    done
    if [ "$MISSING" -ne 0 ]; then
      echo "[doctor] Missing $MISSING required tool(s). Run 'just bootstrap' to install."
      exit 1
    fi
    # Check versioned git hooks path (submodule-integrity pre-commit).
    hooks_path="$(git config core.hooksPath 2>/dev/null || true)"
    if [ "$hooks_path" = "scripts/hooks" ] && [ -x scripts/hooks/pre-commit ]; then
      printf "[doctor] %-8s OK (core.hooksPath=scripts/hooks)\n" "hooks"
    else
      printf "[doctor] %-8s NOT CONFIGURED (run 'just setup' to enable)\n" "hooks"
    fi
    echo "[doctor] Environment looks good."

[private]
clean_impl:
    #!/usr/bin/env bash
    set -euo pipefail
    {{_log}}
    # Language-specific cleanup
    rm -rf build dist node_modules/.cache
    log_end "Build artifacts removed"

[private]
clean_all_impl:
    #!/usr/bin/env bash
    set -euo pipefail
    {{_log}}
    rm -rf build dist node_modules .cache
    log_end "All build artifacts and dependencies removed"
