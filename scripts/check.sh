#!/usr/bin/env bash
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
BOLD='\033[1m'
RESET='\033[0m'

passed=0
failed=0
failures=()

run() {
    local name="$1"
    shift
    printf "${BOLD}▶ %s${RESET}\n" "$name"
    if "$@" > /dev/null 2>&1; then
        printf "${GREEN}  ✓ passed${RESET}\n"
        passed=$((passed + 1))
    else
        printf "${RED}  ✗ failed${RESET}\n"
        failed=$((failed + 1))
        failures+=("$name")
    fi
}

run "fmt"         cargo fmt --all -- --check
run "clippy"      cargo clippy --all-targets -- -D warnings
run "test"        cargo test --all-targets
run "audit"       cargo audit
run "trivy"       trivy fs --severity HIGH,CRITICAL --exit-code 1 --scanners vuln --quiet .

run "pinned-deps" python3 -c "
import tomllib, sys
with open('Cargo.toml', 'rb') as f:
    cargo = tomllib.load(f)
errors = []
for section in ('dependencies', 'dev-dependencies'):
    deps = cargo.get(section, {})
    for name, spec in deps.items():
        ver = spec if isinstance(spec, str) else spec.get('version', '')
        if ver and not ver.startswith('='):
            errors.append(f'{section}.{name} = \"{ver}\"')
if errors:
    sys.exit(1)
"

echo ""
printf "${BOLD}Results: ${GREEN}%d passed${RESET}" "$passed"
if [ "$failed" -gt 0 ]; then
    printf ", ${RED}%d failed${RESET}" "$failed"
    echo ""
    for f in "${failures[@]}"; do
        printf "  ${RED}✗ %s${RESET}\n" "$f"
    done
    exit 1
fi
echo ""
