# Shared by extension packagers. Sets `out` to an absolute, physical output path before cleanup.
prepare_output() {
    [ -n "$out" ] && [ ! -L "$out" ] || { echo "Refusing empty or symlink output directory" >&2; exit 1; }
    mkdir -p "$out"
    out=$(cd "$out" && pwd -P)
    # A typo such as '.', '../..', or 'extension' must never erase the checkout/source.
    case "$root/" in "$out/"*) echo "Refusing output directory containing the repository: $out" >&2; exit 1 ;; esac
    case "$out/" in "$root/extension/"*) echo "Refusing output inside the extension source: $out" >&2; exit 1 ;; esac
    rm -rf "$out"
    mkdir -p "$out"
}
