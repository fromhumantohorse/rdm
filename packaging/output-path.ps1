# Resolve and validate before a packager recursively clears its output directory.
function Initialize-ExtensionOutput([string]$Root, [string]$Output) {
    $outputPath = [IO.Path]::GetFullPath($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Output)).TrimEnd('\', '/')
    $rootPath = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
    $comparison = [StringComparison]::OrdinalIgnoreCase
    if (($rootPath + '\').StartsWith($outputPath + '\', $comparison) -or
        ($outputPath + '\').StartsWith($rootPath + '\extension\', $comparison)) {
        throw "Refusing output directory overlapping the repository/source: $outputPath"
    }
    # Reject junctions/symlinks in the path as well: their physical target could be the source tree.
    $cursor = $outputPath
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Refusing linked output path: $cursor" }
        }
        $cursor = Split-Path $cursor -Parent
    }
    if (Test-Path -LiteralPath $outputPath) { Remove-Item -LiteralPath $outputPath -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $outputPath | Out-Null
    return $outputPath
}
