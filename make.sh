#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
./jc-mobile/make.sh "$@"
