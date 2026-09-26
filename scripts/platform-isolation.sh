#!/usr/bin/env bash
# Proves the two halves of "only applicable OS backends compile into each
# build" (#79) with evidence from the build itself rather than from a
# reading of Cargo.toml:
#
#   deps <target-triple>   the resolved *normal* dependency graph for that
#                          target contains every dependency the target owns
#                          and none that another target owns.
#   linkage <binary>       the built binary links the libraries its own OS
#                          needs and carries no symbol from the other OS's
#                          backend.
#
# Run from the repository root. CI runs both on both targets
# (.github/workflows/ci.yml); a contributor can run either by hand.

set -euo pipefail

# Crates that may only ever appear in a macOS build. Apple framework
# bindings and the FSEvents replay backend.
MACOS_ONLY=(core-foundation core-foundation-sys fsevent-sys)

# Crates that may only ever appear in a Linux build. Empty today: the
# Linux backends in this repo are written against `libc`, which is shared.
# Kept as a list so #81's watcher crate has a place to be declared, and so
# the macOS side of the check is a real assertion rather than a no-op.
LINUX_ONLY=()

fail() {
    echo "platform isolation: $*" >&2
    exit 1
}

deps() {
    local target="${1:?usage: platform-isolation.sh deps <target-triple>}"
    local tree
    # `-e normal` drops dev- and build-dependencies: a test-only or
    # build-script-only crate is not linked into the artifact and is not
    # what this check is about.
    tree="$(cargo tree --workspace --locked --target "$target" -e normal)"

    local expected_present=() expected_absent=()
    case "$target" in
        *-apple-darwin)
            expected_present=("${MACOS_ONLY[@]}")
            expected_absent=("${LINUX_ONLY[@]+"${LINUX_ONLY[@]}"}")
            ;;
        *-linux-*)
            expected_present=("${LINUX_ONLY[@]+"${LINUX_ONLY[@]}"}")
            expected_absent=("${MACOS_ONLY[@]}")
            ;;
        *)
            fail "no isolation expectations recorded for target '$target'; add them here before claiming support"
            ;;
    esac

    local crate
    for crate in "${expected_present[@]+"${expected_present[@]}"}"; do
        grep -qE "(^|[[:space:]])${crate} v" <<<"$tree" ||
            fail "$crate is required for $target but is not in its dependency graph"
        echo "ok: $crate present for $target"
    done
    for crate in "${expected_absent[@]+"${expected_absent[@]}"}"; do
        if grep -qE "(^|[[:space:]])${crate} v" <<<"$tree"; then
            grep -nE "(^|[[:space:]])${crate} v" <<<"$tree" >&2
            fail "$crate must not be in the $target dependency graph"
        fi
        echo "ok: $crate absent for $target"
    done
}

linkage() {
    local bin="${1:?usage: platform-isolation.sh linkage <binary>}"
    [ -f "$bin" ] || fail "no binary at $bin"

    # Symbol tables are the ABI-level evidence. A binary that never
    # compiled the FSEvents backend cannot reference FSEventStream*.
    local symbols=""
    if command -v nm >/dev/null 2>&1; then
        symbols="$(nm "$bin" 2>/dev/null || true)"
    fi

    case "$(uname -s)" in
        Linux)
            local libs
            libs="$(ldd "$bin")"
            echo "$libs"
            # An Apple framework cannot load on Linux, so a reference to
            # one would mean the macOS backend leaked into this build.
            if grep -qiE 'CoreFoundation|CoreServices|\.framework|libSystem' <<<"$libs"; then
                fail "Linux binary references an Apple framework"
            fi
            # Everything this binary needs must come from glibc and the
            # loader. A surprise here is a new runtime requirement that
            # the documented Ubuntu 24.04 baseline has not agreed to.
            local unexpected
            unexpected="$(awk '{print $1}' <<<"$libs" |
                grep -vE '^(linux-vdso\.so\.1|/lib64/ld-linux-x86-64\.so\.2|libc\.so\.6|libm\.so\.6|libgcc_s\.so\.1|libdl\.so\.2|libpthread\.so\.0|librt\.so\.1|libutil\.so\.1|ld-linux-x86-64\.so\.2)$' |
                grep -v '^$' || true)"
            if [ -n "$unexpected" ]; then
                echo "$unexpected" >&2
                fail "Linux binary has runtime dependencies outside the documented glibc baseline"
            fi
            if [ -n "$symbols" ] && grep -qi 'FSEventStream' <<<"$symbols"; then
                fail "Linux binary carries FSEvents symbols"
            fi
            echo "ok: Linux binary is glibc-only and carries no Apple backend"
            ;;
        Darwin)
            local libs
            libs="$(otool -L "$bin")"
            echo "$libs"
            grep -q 'CoreFoundation' <<<"$libs" ||
                fail "macOS binary does not link CoreFoundation; the FSEvents backend is missing"
            # glibc/Linux-only sonames have no meaning in a Mach-O and
            # would mean a Linux backend was compiled in.
            if grep -qE 'libc\.so\.6|ld-linux|libinotify' <<<"$libs"; then
                fail "macOS binary references a Linux-only library"
            fi
            echo "ok: macOS binary links Apple frameworks and no Linux-only library"
            ;;
        *)
            fail "linkage check has no expectations for $(uname -s)"
            ;;
    esac
}

case "${1:-}" in
    deps) shift && deps "$@" ;;
    linkage) shift && linkage "$@" ;;
    *) fail "usage: platform-isolation.sh {deps <target-triple>|linkage <binary>}" ;;
esac
