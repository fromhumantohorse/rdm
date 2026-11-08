# Shared by extension packagers. Sets `out` to an absolute, physical output path before cleanup.
prepare_output() {
    root=$(cd "$root" && pwd -P)
    [ -n "$out" ] && [ ! -L "$out" ] || { echo "Refusing empty or symlink output directory" >&2; exit 1; }
    mkdir -p "$out"
    out=$(cd "$out" && pwd -P)
    # A typo such as '.', '../..', or 'extension' must never erase the checkout/source.
    case "$root/" in "$out/"*) echo "Refusing output directory containing the repository: $out" >&2; exit 1 ;; esac
    # Only build-output folders may be replaced within the checkout: `.git`, `crates`, `docs`,
    # etc. are just as destructive to erase as `extension`.
    case "$out/" in
        "$root/"*)
            case "$out/" in
                "$root/target/"*|"$root/dist/"*) ;;
                *) echo "Refusing output inside repository sources: $out" >&2; exit 1 ;;
            esac ;;
    esac
    rm -rf "$out"
    mkdir -p "$out"
}
