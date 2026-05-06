#!/usr/bin/env bash
set -euo pipefail

sudo install -m 755 /dev/stdin /etc/letsencrypt/renewal-hooks/deploy/restart-kodamapub-edge.sh <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

cd /opt/kodamapub
docker compose restart edge
EOF
