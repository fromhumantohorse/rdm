#!/bin/sh
# Safari's background worker is a plain script: inline modules in dependency order.
set -eu
root=$1
output=$2
{
    sed 's/^export //' "$root/extension/public-suffix-data.js"
    sed '/^import /d; s/^export //' "$root/extension/shared.js"
    sed '/^import /d' "$root/extension/background.js"
} >"$output"
