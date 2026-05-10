#!/usr/bin/env bash
set -euo pipefail

exec scripts/kind/mastodon-e2e.sh test
