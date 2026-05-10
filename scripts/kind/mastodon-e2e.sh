#!/usr/bin/env bash
set -euo pipefail

CLUSTER_NAME="${KIND_CLUSTER_NAME:-kodamapub-e2e}"
NAMESPACE="${KIND_NAMESPACE:-kodamapub-e2e}"
MAGOUT_VERSION="${MAGOUT_VERSION:-0.1.33}"
MASTODON_IMAGE_TAG="${MASTODON_IMAGE_TAG:-v4.5.9}"
KODAMAPUB_ORIGIN="${KODAMAPUB_ORIGIN:-https://kodamapub.e2e}"
MASTODON_WEB_DOMAIN="${MASTODON_WEB_DOMAIN:-mastodon.e2e:3001}"
MASTODON_LOCAL_DOMAIN="${MASTODON_LOCAL_DOMAIN:-mastodon.e2e}"
CERT_DIR="tmp/kind-certs"
CA_CERT="${CERT_DIR}/kodamapub-e2e-ca.pem"
CA_KEY="${CERT_DIR}/kodamapub-e2e-ca-key.pem"
APP_CERT="${CERT_DIR}/app.localhost.pem"
APP_KEY="${CERT_DIR}/app.localhost-key.pem"
export KUBECONFIG="${KUBECONFIG:-tmp/kind-kubeconfig}"
export HELM_CACHE_HOME="${HELM_CACHE_HOME:-tmp/helm-cache}"
export HELM_CONFIG_HOME="${HELM_CONFIG_HOME:-tmp/helm-config}"
export HELM_DATA_HOME="${HELM_DATA_HOME:-tmp/helm-data}"

usage() {
  cat <<'USAGE'
Usage: scripts/kind/mastodon-e2e.sh <command>

Commands:
  up            Create/update kind cluster and deploy kodamapub + Magout-managed Mastodon
  seed          Create kodamapub alice/password and Mastodon e2e user
  test          Run the Mastodon interoperability E2E flow in kind
  port-forward  Forward UI ports: kodamapub 8443, Mastodon 3001
  status        Show pods, services, PVCs, and MastodonServer
  logs          Follow kodamapub and Mastodon logs
  down          Delete the kind cluster

Host DNS needed while port-forwarding:
  127.0.0.1 kodamapub.e2e mastodon.e2e
USAGE
}

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'missing required command: %s\n' "$1" >&2
    exit 1
  fi
}

check_deps() {
  need docker
  need kind
  need kubectl
  need helm
  need openssl
  mkdir -p "$(dirname "${KUBECONFIG}")"
  mkdir -p "${HELM_CACHE_HOME}" "${HELM_CONFIG_HOME}" "${HELM_DATA_HOME}"
}

kind_cluster_exists() {
  kind get clusters | grep -Fxq "${CLUSTER_NAME}"
}

create_cluster() {
  if kind_cluster_exists; then
    kind export kubeconfig --name "${CLUSTER_NAME}" >/dev/null
    return
  fi

  cat <<EOF | kind create cluster --name "${CLUSTER_NAME}" --config -
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
  - role: control-plane
EOF
}

generate_certs() {
  mkdir -p "${CERT_DIR}"
  if [[ -s "${CA_CERT}" && -s "${CA_KEY}" && -s "${APP_CERT}" && -s "${APP_KEY}" ]]; then
    return
  fi

  openssl req -x509 -newkey rsa:2048 -sha256 -nodes -days 3650 \
    -keyout "${CA_KEY}" \
    -out "${CA_CERT}" \
    -subj "/CN=kodamapub-e2e-ca" >/dev/null 2>&1

  openssl req -newkey rsa:2048 -sha256 -nodes \
    -keyout "${APP_KEY}" \
    -out "${CERT_DIR}/app.localhost.csr" \
    -subj "/CN=kodamapub.e2e" >/dev/null 2>&1

  openssl x509 -req \
    -in "${CERT_DIR}/app.localhost.csr" \
    -CA "${CA_CERT}" \
    -CAkey "${CA_KEY}" \
    -CAcreateserial \
    -out "${APP_CERT}" \
    -days 3650 \
    -sha256 \
    -extfile <(printf '%s\n' \
      'subjectAltName=DNS:kodamapub.e2e,DNS:mastodon.e2e,DNS:edge,DNS:mastodon-proxy,DNS:localhost,IP:127.0.0.1' \
      'basicConstraints=CA:FALSE' \
      'keyUsage=digitalSignature,keyEncipherment' \
      'extendedKeyUsage=serverAuth') >/dev/null 2>&1

  rm -f "${CERT_DIR}/app.localhost.csr" "${CA_CERT}.srl"
}

build_and_load_images() {
  local buildx_config="${BUILDX_CONFIG:-}"

  if [[ -z "${buildx_config}" && -z "${CI:-}" ]]; then
    buildx_config="tmp/buildx"
  fi

  if [[ -n "${buildx_config}" ]]; then
    mkdir -p "${buildx_config}"
    BUILDX_CONFIG="${buildx_config}" docker buildx bake --file docker-bake.hcl --load
  else
    docker buildx bake --file docker-bake.hcl --load
  fi

  kind load docker-image --name "${CLUSTER_NAME}" \
    kodamapub-edge:latest \
    kodamapub-server:latest \
    kodamapub-web:latest \
    kodamapub-cli-job:latest
}

generate_mastodon_env_file() {
  local env_file="tmp/kind-mastodon.env"
  local image="ghcr.io/mastodon/mastodon:${MASTODON_IMAGE_TAG}"
  local secret_key_base otp_secret encryption_output vapid_output

  mkdir -p tmp
  if [[ -s "${env_file}" ]]; then
    return
  fi

  secret_key_base="$(docker run --rm "${image}" bin/rails secret)"
  otp_secret="$(docker run --rm "${image}" bin/rails secret)"
  encryption_output="$(docker run --rm "${image}" bundle exec rails db:encryption:init)"
  vapid_output="$(
    docker run --rm \
      -e SECRET_KEY_BASE="${secret_key_base}" \
      -e OTP_SECRET="${otp_secret}" \
      "${image}" \
      bin/rake mastodon:webpush:generate_vapid_key
  )"

  {
    printf 'RAILS_ENV=production\n'
    printf 'NODE_ENV=production\n'
    printf 'LOCAL_DOMAIN=%s\n' "${MASTODON_LOCAL_DOMAIN}"
    printf 'WEB_DOMAIN=%s\n' "${MASTODON_WEB_DOMAIN}"
    printf 'LOCAL_HTTPS=false\n'
    printf 'ALLOWED_PRIVATE_ADDRESSES=10.0.0.0/8,172.16.0.0/12,192.168.0.0/16\n'
    printf 'SSL_CERT_FILE=/certs/kodamapub-e2e-ca.pem\n'
    printf 'DB_HOST=mastodon-db\n'
    printf 'DB_PORT=5432\n'
    printf 'DB_NAME=mastodon\n'
    printf 'DB_USER=mastodon\n'
    printf 'DB_PASS=mastodon\n'
    printf 'REDIS_HOST=mastodon-redis\n'
    printf 'REDIS_PORT=6379\n'
    printf 'DB_POOL=5\n'
    printf 'DEFAULT_LOCALE=en\n'
    printf 'FORCE_DEFAULT_LOCALE=true\n'
    printf 'RAILS_SERVE_STATIC_FILES=true\n'
    printf 'DISABLE_AUTOMATIC_SWITCHING_TO_APPROVED_REGISTRATIONS=true\n'
    printf 'SECRET_KEY_BASE=%s\n' "${secret_key_base}"
    printf 'OTP_SECRET=%s\n' "${otp_secret}"
    printf 'ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=%s\n' "$(sed -n 's/^ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=//p' <<<"${encryption_output}")"
    printf 'ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY=%s\n' "$(sed -n 's/^ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY=//p' <<<"${encryption_output}")"
    printf 'ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT=%s\n' "$(sed -n 's/^ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT=//p' <<<"${encryption_output}")"
    printf 'VAPID_PRIVATE_KEY=%s\n' "$(sed -n 's/^VAPID_PRIVATE_KEY=//p' <<<"${vapid_output}")"
    printf 'VAPID_PUBLIC_KEY=%s\n' "$(sed -n 's/^VAPID_PUBLIC_KEY=//p' <<<"${vapid_output}")"
  } >"${env_file}"
}

configure_coredns() {
  local corefile="tmp/kind-coredns.Corefile"
  local patched="tmp/kind-coredns-patched.Corefile"

  mkdir -p tmp
  kubectl -n kube-system get configmap coredns -o jsonpath='{.data.Corefile}' >"${corefile}"
  if grep -q 'kodamapub-e2e rewrites' "${corefile}"; then
    return
  fi

  awk '
    /^\.:53 \{/ {
      print
      print "    # kodamapub-e2e rewrites"
      print "    rewrite name exact kodamapub.e2e edge.kodamapub-e2e.svc.cluster.local"
      print "    rewrite name exact mastodon.e2e mastodon-proxy.kodamapub-e2e.svc.cluster.local"
      next
    }
    { print }
  ' "${corefile}" >"${patched}"

  kubectl -n kube-system create configmap coredns \
    --from-file=Corefile="${patched}" \
    --dry-run=client -o yaml | kubectl apply -f -
  kubectl -n kube-system rollout restart deployment/coredns
  kubectl -n kube-system rollout status deployment/coredns --timeout=120s
}

create_namespace_and_secrets() {
  kubectl create namespace "${NAMESPACE}" --dry-run=client -o yaml | kubectl apply -f -

  kubectl -n "${NAMESPACE}" create secret generic kodamapub-e2e-certs \
    --from-file=app.localhost.pem="${APP_CERT}" \
    --from-file=app.localhost-key.pem="${APP_KEY}" \
    --from-file=kodamapub-e2e-ca.pem="${CA_CERT}" \
    --dry-run=client -o yaml | kubectl apply -f -

  kubectl -n "${NAMESPACE}" create secret generic mastodon-env \
    --from-env-file=tmp/kind-mastodon.env \
    --dry-run=client -o yaml | kubectl apply -f -
}

apply_kodamapub_and_mastodon_dependencies() {
  cat <<EOF | kubectl apply -f -
apiVersion: v1
kind: ConfigMap
metadata:
  name: mastodon-proxy
  namespace: ${NAMESPACE}
data:
  default.conf: |
    server {
        listen 3001 ssl;
        server_name _;
        ssl_certificate /certs/app.localhost.pem;
        ssl_certificate_key /certs/app.localhost-key.pem;
        location / {
            proxy_pass http://mastodon-web:3000;
            proxy_http_version 1.1;
            proxy_set_header Host \$http_host;
            proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
            proxy_set_header X-Forwarded-Host \$http_host;
            proxy_set_header X-Forwarded-Proto https;
            proxy_set_header Connection "";
        }
    }
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: kodamapub-data
  namespace: ${NAMESPACE}
spec:
  accessModes: ["ReadWriteOnce"]
  resources:
    requests:
      storage: 1Gi
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: mastodon-db-data
  namespace: ${NAMESPACE}
spec:
  accessModes: ["ReadWriteOnce"]
  resources:
    requests:
      storage: 5Gi
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: server
  namespace: ${NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: server}
  template:
    metadata:
      labels: {app: server}
    spec:
      containers:
        - name: server
          image: kodamapub-server:latest
          imagePullPolicy: Never
          ports:
            - containerPort: 3000
          env:
            - {name: BIND_ADDR, value: "0.0.0.0:3000"}
            - {name: DATABASE_URL, value: "sqlite:///data/kodamapub.db?mode=rwc"}
            - {name: PUBLIC_BASE_URL, value: "${KODAMAPUB_ORIGIN}"}
            - {name: KODAMAPUB_ALLOWED_ORIGINS, value: "https://kodamapub.e2e:8443"}
            - {name: KODAMAPUB_REMOTE_CA_CERT_PATH, value: "/certs/kodamapub-e2e-ca.pem"}
          volumeMounts:
            - {name: data, mountPath: /data}
            - {name: certs, mountPath: /certs, readOnly: true}
      volumes:
        - name: data
          persistentVolumeClaim: {claimName: kodamapub-data}
        - name: certs
          secret: {secretName: kodamapub-e2e-certs}
---
apiVersion: v1
kind: Service
metadata:
  name: server
  namespace: ${NAMESPACE}
spec:
  selector: {app: server}
  ports:
    - {name: http, port: 3000, targetPort: 3000}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  namespace: ${NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: web}
  template:
    metadata:
      labels: {app: web}
    spec:
      containers:
        - name: web
          image: kodamapub-web:latest
          imagePullPolicy: Never
          ports:
            - containerPort: 5173
          env:
            - {name: API_ORIGIN, value: "http://server:3000"}
            - {name: PORT, value: "5173"}
---
apiVersion: v1
kind: Service
metadata:
  name: web
  namespace: ${NAMESPACE}
spec:
  selector: {app: web}
  ports:
    - {name: http, port: 5173, targetPort: 5173}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: edge
  namespace: ${NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: edge}
  template:
    metadata:
      labels: {app: edge}
    spec:
      containers:
        - name: edge
          image: kodamapub-edge:latest
          imagePullPolicy: Never
          ports:
            - containerPort: 443
          env:
            - {name: UPSTREAM_WEB_ADDR, value: "web:5173"}
            - {name: UPSTREAM_WEB_HOST, value: "web"}
            - {name: UPSTREAM_ADDR, value: "server:3000"}
            - {name: UPSTREAM_HOST, value: "server"}
          volumeMounts:
            - {name: certs, mountPath: /certs, readOnly: true}
      volumes:
        - name: certs
          secret: {secretName: kodamapub-e2e-certs}
---
apiVersion: v1
kind: Service
metadata:
  name: edge
  namespace: ${NAMESPACE}
spec:
  selector: {app: edge}
  ports:
    - {name: https, port: 443, targetPort: 443}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: delivery-worker
  namespace: ${NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: delivery-worker}
  template:
    metadata:
      labels: {app: delivery-worker}
    spec:
      containers:
        - name: delivery-worker
          image: kodamapub-cli-job:latest
          imagePullPolicy: Never
          args: ["run-delivery-worker", "--limit", "100", "--interval-seconds", "1"]
          env:
            - {name: DATABASE_URL, value: "sqlite:///data/kodamapub.db?mode=rwc"}
            - {name: KODAMAPUB_REMOTE_CA_CERT_PATH, value: "/certs/kodamapub-e2e-ca.pem"}
          volumeMounts:
            - {name: data, mountPath: /data}
            - {name: certs, mountPath: /certs, readOnly: true}
      volumes:
        - name: data
          persistentVolumeClaim: {claimName: kodamapub-data}
        - name: certs
          secret: {secretName: kodamapub-e2e-certs}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: mastodon-db
  namespace: ${NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: mastodon-db}
  template:
    metadata:
      labels: {app: mastodon-db}
    spec:
      containers:
        - name: postgres
          image: postgres:16-alpine
          ports:
            - containerPort: 5432
          env:
            - {name: POSTGRES_DB, value: mastodon}
            - {name: POSTGRES_USER, value: mastodon}
            - {name: POSTGRES_PASSWORD, value: mastodon}
          volumeMounts:
            - {name: data, mountPath: /var/lib/postgresql/data}
      volumes:
        - name: data
          persistentVolumeClaim: {claimName: mastodon-db-data}
---
apiVersion: v1
kind: Service
metadata:
  name: mastodon-db
  namespace: ${NAMESPACE}
spec:
  selector: {app: mastodon-db}
  ports:
    - {name: postgres, port: 5432, targetPort: 5432}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: mastodon-redis
  namespace: ${NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: mastodon-redis}
  template:
    metadata:
      labels: {app: mastodon-redis}
    spec:
      containers:
        - name: redis
          image: redis:7-alpine
          ports:
            - containerPort: 6379
---
apiVersion: v1
kind: Service
metadata:
  name: mastodon-redis
  namespace: ${NAMESPACE}
spec:
  selector: {app: mastodon-redis}
  ports:
    - {name: redis, port: 6379, targetPort: 6379}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: mastodon-proxy
  namespace: ${NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: mastodon-proxy}
  template:
    metadata:
      labels: {app: mastodon-proxy}
    spec:
      containers:
        - name: nginx
          image: nginx:1.27-alpine
          ports:
            - containerPort: 3001
          volumeMounts:
            - {name: certs, mountPath: /certs, readOnly: true}
            - {name: config, mountPath: /etc/nginx/conf.d, readOnly: true}
      volumes:
        - name: certs
          secret: {secretName: kodamapub-e2e-certs}
        - name: config
          configMap: {name: mastodon-proxy}
---
apiVersion: v1
kind: Service
metadata:
  name: mastodon-proxy
  namespace: ${NAMESPACE}
spec:
  selector: {app: mastodon-proxy}
  ports:
    - {name: https, port: 3001, targetPort: 3001}
EOF
}

install_magout() {
  helm repo add magout https://ushitora-anqou.github.io/magout >/dev/null
  helm repo update magout >/dev/null
  helm upgrade --install magout-cluster-wide magout/magout-cluster-wide \
    --version "0.1.10" \
    --namespace "${NAMESPACE}" \
    --create-namespace

  local values="tmp/kind-magout-values.yaml"
  cat >"${values}" <<EOF
fullnameOverride: mastodon
serverName: ${MASTODON_WEB_DOMAIN}
mastodonVersion:
  image: ghcr.io/mastodon/mastodon:${MASTODON_IMAGE_TAG}
  streamingImage: ghcr.io/mastodon/mastodon-streaming:${MASTODON_IMAGE_TAG}
mastodonServer:
  web: &mastodonPod
    replicas: 1
    envFrom:
      - secretRef:
          name: mastodon-env
    volumeMounts:
      - name: certs
        mountPath: /certs
        readOnly: true
    volumes:
      - name: certs
        secret:
          secretName: kodamapub-e2e-certs
  sidekiq: *mastodonPod
  streaming: *mastodonPod
gateway:
  enabled: true
  service:
    type: ClusterIP
    port: 8080
EOF

  helm upgrade --install mastodon magout/magout \
    --version "${MAGOUT_VERSION}" \
    --namespace "${NAMESPACE}" \
    --values "${values}"
}

restart_cert_consumers() {
  kubectl -n "${NAMESPACE}" rollout restart deployment/edge deployment/server deployment/delivery-worker deployment/mastodon-proxy
  for deployment in mastodon-web mastodon-sidekiq mastodon-streaming; do
    if kubectl -n "${NAMESPACE}" get deployment "${deployment}" >/dev/null 2>&1; then
      kubectl -n "${NAMESPACE}" rollout restart "deployment/${deployment}"
    fi
  done
}

wait_for_rollouts() {
  kubectl -n "${NAMESPACE}" rollout status deployment/mastodon-db --timeout=180s
  kubectl -n "${NAMESPACE}" rollout status deployment/mastodon-redis --timeout=180s
  kubectl -n "${NAMESPACE}" rollout status deployment/server --timeout=180s
  kubectl -n "${NAMESPACE}" rollout status deployment/web --timeout=180s
  kubectl -n "${NAMESPACE}" rollout status deployment/edge --timeout=180s
  kubectl -n "${NAMESPACE}" rollout status deployment/delivery-worker --timeout=180s
  kubectl -n "${NAMESPACE}" rollout status deployment/mastodon-operator --timeout=180s
  for deployment in mastodon-web mastodon-sidekiq mastodon-streaming mastodon-gateway; do
    for _ in $(seq 1 60); do
      if kubectl -n "${NAMESPACE}" get deployment "${deployment}" >/dev/null 2>&1; then
        break
      fi
      sleep 2
    done
    kubectl -n "${NAMESPACE}" rollout status "deployment/${deployment}" --timeout=600s
  done
  kubectl -n "${NAMESPACE}" rollout status deployment/mastodon-proxy --timeout=180s
}

seed_local_actor() {
  kubectl -n "${NAMESPACE}" delete job kodamapub-seed-alice --ignore-not-found
  cat <<EOF | kubectl apply -f -
apiVersion: batch/v1
kind: Job
metadata:
  name: kodamapub-seed-alice
  namespace: ${NAMESPACE}
spec:
  backoffLimit: 1
  template:
    spec:
      restartPolicy: Never
      containers:
        - name: cli
          image: kodamapub-cli-job:latest
          imagePullPolicy: Never
          args:
            - create-local-actor
            - --public-base-url
            - "${KODAMAPUB_ORIGIN}"
            - --username
            - alice
            - --display-name
            - Alice
            - --summary
            - kind Mastodon E2E actor
            - --password
            - password
          env:
            - {name: DATABASE_URL, value: "sqlite:///data/kodamapub.db?mode=rwc"}
          volumeMounts:
            - {name: data, mountPath: /data}
      volumes:
        - name: data
          persistentVolumeClaim: {claimName: kodamapub-data}
EOF
  kubectl -n "${NAMESPACE}" wait --for=condition=complete job/kodamapub-seed-alice --timeout=120s
}

seed_mastodon_user() {
  local password

  password="$(create_mastodon_user)"

  printf 'Mastodon user: e2e@uta8a.org\n'
  printf 'Mastodon password: %s\n' "${password}"
}

mastodon_web_pod() {
  local pod

  pod="$(kubectl -n "${NAMESPACE}" get pod \
    -l app.kubernetes.io/component=web,app.kubernetes.io/part-of=mastodon \
    -o jsonpath='{.items[0].metadata.name}')"
  printf '%s\n' "${pod}"
}

yaml_single_quote() {
  printf "'%s'" "$(sed "s/'/''/g" <<<"$1")"
}

run_kodamapub_cli_job() {
  local job_name="$1"
  shift

  kubectl -n "${NAMESPACE}" delete job "${job_name}" --ignore-not-found >/dev/null
  kubectl -n "${NAMESPACE}" wait --for=delete "job/${job_name}" --timeout=60s >/dev/null 2>&1 || true
  {
    cat <<EOF
apiVersion: batch/v1
kind: Job
metadata:
  name: ${job_name}
  namespace: ${NAMESPACE}
spec:
  backoffLimit: 1
  template:
    spec:
      restartPolicy: Never
      containers:
        - name: cli
          image: kodamapub-cli-job:latest
          imagePullPolicy: Never
          args:
EOF
    local arg
    for arg in "$@"; do
      printf '            - %s\n' "$(yaml_single_quote "${arg}")"
    done
    cat <<'EOF'
          env:
            - {name: DATABASE_URL, value: "sqlite:///data/kodamapub.db?mode=rwc"}
            - {name: KODAMAPUB_REMOTE_CA_CERT_PATH, value: "/certs/kodamapub-e2e-ca.pem"}
          volumeMounts:
            - {name: data, mountPath: /data}
            - {name: certs, mountPath: /certs, readOnly: true}
      volumes:
        - name: data
          persistentVolumeClaim: {claimName: kodamapub-data}
        - name: certs
          secret: {secretName: kodamapub-e2e-certs}
EOF
  } | kubectl apply -f - >/dev/null

  if ! kubectl -n "${NAMESPACE}" wait --for=condition=complete "job/${job_name}" --timeout=180s >/dev/null; then
    kubectl -n "${NAMESPACE}" logs "job/${job_name}" --all-containers >&2 || true
    return 1
  fi
}

create_test_local_actor() {
  local username="$1"

  run_kodamapub_cli_job "kodamapub-create-${username}" \
    create-local-actor \
    --public-base-url "${KODAMAPUB_ORIGIN}" \
    --username "${username}" \
    --display-name Alice \
    --summary "GitHub Actions E2E actor" \
    --password password
}

create_mastodon_app() {
  local pod
  pod="$(mastodon_web_pod)"

  kubectl -n "${NAMESPACE}" exec "${pod}" -- bash -lc \
    'RAILS_ENV=production bundle exec rails runner '\''app = Doorkeeper::Application.find_or_initialize_by(name: "kodamapub-e2e"); app.redirect_uri = "urn:ietf:wg:oauth:2.0:oob"; app.scopes = "read:accounts read:follows read:statuses write:accounts write:follows"; app.website = "https://example.invalid"; app.save!; puts app.uid'\'''
}

create_mastodon_user() {
  local pod password
  pod="$(mastodon_web_pod)"

  password="$(
    kubectl -n "${NAMESPACE}" exec "${pod}" -- \
      bash -lc 'RAILS_ENV=production bin/tootctl accounts modify e2e --approve --reset-password' \
      2>/dev/null | sed -n 's/^New password: //p' | tail -n 1 || true
  )"

  if [[ -z "${password}" ]]; then
    password="$(
      kubectl -n "${NAMESPACE}" exec "${pod}" -- \
        bash -lc 'RAILS_ENV=production bin/tootctl accounts create e2e --email e2e@uta8a.org --confirmed --force --approve' \
        | sed -n 's/^New password: //p' | tail -n 1
    )"
  fi

  if [[ -z "${password}" ]]; then
    printf 'failed to obtain Mastodon password from tootctl output\n' >&2
    return 1
  fi

  printf '%s\n' "${password}"
}

create_mastodon_user_token() {
  local client_id="$1"
  local pod
  pod="$(mastodon_web_pod)"

  kubectl -n "${NAMESPACE}" exec "${pod}" -- bash -lc \
    'CLIENT_ID="$0" USER_EMAIL="$1" RAILS_ENV=production bundle exec rails runner '\''app = Doorkeeper::Application.find_by!(uid: ENV.fetch("CLIENT_ID")); user = User.where(email: ENV.fetch("USER_EMAIL")).order(created_at: :desc).first!; token = Doorkeeper::AccessToken.create_for(application: app, resource_owner: user, scopes: "read:accounts read:follows read:statuses write:accounts write:follows"); puts token.token'\''' \
    "${client_id}" "e2e@uta8a.org"
}

lookup_remote_account_id() {
  local username="$1"
  local pod
  pod="$(mastodon_web_pod)"

  kubectl -n "${NAMESPACE}" exec "${pod}" -- bash -lc \
    'RAILS_ENV=production bundle exec rails runner '\''account = ActivityPub::FetchRemoteAccountService.new.call("https://kodamapub.e2e/users/'"${username}"'"); puts account.id'\'''
}

start_port_forwards() {
  kubectl -n "${NAMESPACE}" port-forward service/edge 8443:443 >/tmp/kodamapub-edge-port-forward.log 2>&1 &
  EDGE_PORT_FORWARD_PID="$!"
  kubectl -n "${NAMESPACE}" port-forward service/mastodon-proxy 3001:3001 >/tmp/kodamapub-mastodon-port-forward.log 2>&1 &
  MASTODON_PORT_FORWARD_PID="$!"
}

stop_port_forwards() {
  if [[ -n "${EDGE_PORT_FORWARD_PID:-}" ]]; then
    kill "${EDGE_PORT_FORWARD_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${MASTODON_PORT_FORWARD_PID:-}" ]]; then
    kill "${MASTODON_PORT_FORWARD_PID}" >/dev/null 2>&1 || true
  fi
}

curl_request() {
  local label="$1"
  local resolve_host="$2"
  local resolve_port="$3"
  shift 3

  printf 'curl[%s]: %s\n' "$label" "$*" >&2
  curl --cacert "${CA_CERT}" \
    --resolve "${resolve_host}:${resolve_port}:127.0.0.1" \
    -fsS --retry 5 --retry-all-errors --retry-connrefused --retry-delay 1 "$@"
}

wait_for_http() {
  local host="$1"
  local port="$2"
  local url="$3"
  local label="$4"
  local attempt

  for attempt in $(seq 1 60); do
    if curl_request "$label" "$host" "$port" "$url" >/dev/null; then
      printf 'ready: %s\n' "$label"
      return 0
    fi

    sleep 5
  done

  printf 'timed out waiting for %s (%s)\n' "$label" "$url" >&2
  return 1
}

wait_for_mastodon_instance() {
  local attempt

  for attempt in $(seq 1 60); do
    if curl_request "mastodon instance" "mastodon.e2e" "3001" \
      https://mastodon.e2e:3001/api/v1/instance >/dev/null; then
      printf 'ready: %s\n' "mastodon instance"
      return 0
    fi

    sleep 5
  done

  printf 'timed out waiting for mastodon instance\n' >&2
  return 1
}

login_local_actor() {
  local cookie_jar="$1"
  local username="$2"
  local login_output

  login_output="$(
    curl_request "login local actor" "kodamapub.e2e" "8443" \
      -c "${cookie_jar}" \
      -H 'Origin: https://kodamapub.e2e:8443' \
      -H 'Content-Type: application/json' \
      -d '{
        "username": "'"${username}"'",
        "password": "password"
      }' \
      https://kodamapub.e2e:8443/api/login
  )"

  jq -r '.csrf_token' <<<"${login_output}"
}

create_local_post() {
  local cookie_jar="$1"
  local csrf_token="$2"
  local content_source="$3"
  local username="$4"

  curl_request "create local post" "kodamapub.e2e" "8443" -X POST \
    -b "${cookie_jar}" \
    -H 'Origin: https://kodamapub.e2e:8443' \
    -H "x-csrf-token: ${csrf_token}" \
    -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg content_source "${content_source}" '{
      content_source: $content_source,
      content_format: "Plaintext",
      visibility: "Public",
      in_reply_to: null
    }')" \
    "https://kodamapub.e2e:8443/api/users/${username}/posts" >/dev/null
}

follow_remote_account() {
  local token="$1"
  local remote_id="$2"

  curl_request "mastodon follow" "mastodon.e2e" "3001" -X POST \
    -H "Authorization: Bearer ${token}" \
    "https://mastodon.e2e:3001/api/v1/accounts/${remote_id}/follow" >/dev/null
}

unfollow_remote_account() {
  local token="$1"
  local remote_id="$2"

  curl_request "mastodon unfollow" "mastodon.e2e" "3001" -X POST \
    -H "Authorization: Bearer ${token}" \
    "https://mastodon.e2e:3001/api/v1/accounts/${remote_id}/unfollow" >/dev/null
}

run_delivery_jobs() {
  run_kodamapub_cli_job "kodamapub-deliver-$(date +%s)-${RANDOM}" \
    run-deliveries --limit 100 >/dev/null
}

wait_for_follow_state() {
  local expected="$1"
  local remote_id="$2"
  local token="$3"
  local attempt relationship value

  for attempt in $(seq 1 40); do
    run_delivery_jobs

    if ! relationship="$(curl_request "mastodon relationships" \
      "mastodon.e2e" "3001" \
      -H "Authorization: Bearer ${token}" \
      --get \
      --data-urlencode "id[]=${remote_id}" \
      https://mastodon.e2e:3001/api/v1/accounts/relationships)"; then
      sleep 3
      continue
    fi

    value="$(jq -r '.[0].following' <<<"${relationship}")"
    if [[ "${value}" == "${expected}" ]]; then
      return 0
    fi

    sleep 3
  done

  printf 'timed out waiting for relationship following=%s for account %s\n' "$expected" "$remote_id" >&2
  return 1
}

wait_for_remote_post_content() {
  local token="$1"
  local remote_id="$2"
  local expected_content="$3"
  local attempt statuses

  for attempt in $(seq 1 40); do
    run_delivery_jobs

    if ! statuses="$(
      curl_request "mastodon statuses" \
        "mastodon.e2e" "3001" \
        -H "Authorization: Bearer ${token}" \
        --get \
        --data-urlencode "limit=20" \
        "https://mastodon.e2e:3001/api/v1/accounts/${remote_id}/statuses"
    )"; then
      sleep 3
      continue
    fi

    if jq -e --arg expected "${expected_content}" '
      any(.[]; (.content // "") | contains($expected))
    ' <<<"${statuses}" >/dev/null; then
      return 0
    fi

    sleep 3
  done

  printf 'timed out waiting for mastodon to expose post content for account %s\n' "$remote_id" >&2
  return 1
}

run_repeated_post_delivery_case() {
  local token="$1"
  local username remote_id cookie_jar csrf_token first_post second_post

  username="alice-repeat-$(openssl rand -hex 8)"
  first_post="Mastodon E2E repeated post first $(openssl rand -hex 4)"
  second_post="Mastodon E2E repeated post second $(openssl rand -hex 4)"

  create_test_local_actor "${username}"
  cookie_jar="$(mktemp)"
  csrf_token="$(login_local_actor "${cookie_jar}" "${username}")"

  remote_id="$(lookup_remote_account_id "${username}")"
  if [[ -z "${remote_id}" || "${remote_id}" == "null" ]]; then
    printf 'failed to resolve remote account id for %s@kodamapub.e2e\n' "${username}" >&2
    return 1
  fi

  follow_remote_account "${token}" "${remote_id}"
  wait_for_follow_state true "${remote_id}" "${token}"

  create_local_post "${cookie_jar}" "${csrf_token}" "${first_post}" "${username}"
  wait_for_remote_post_content "${token}" "${remote_id}" "${first_post}"

  create_local_post "${cookie_jar}" "${csrf_token}" "${second_post}" "${username}"
  wait_for_remote_post_content "${token}" "${remote_id}" "${second_post}"

  unfollow_remote_account "${token}" "${remote_id}"
  wait_for_follow_state false "${remote_id}" "${token}"
}

assert_no_server_errors() {
  local logs
  logs="$(kubectl -n "${NAMESPACE}" logs deployment/server --all-containers --timestamps || true)"
  logs+=$'\n'
  logs+="$(kubectl -n "${NAMESPACE}" logs deployment/delivery-worker --all-containers --timestamps || true)"

  if grep -Eq 'request failed|missing signature header|signature verification failed|invalid remote resource|UNIQUE constraint failed' <<<"${logs}"; then
    printf 'server logs contained an unexpected error:\n' >&2
    grep -En 'request failed|missing signature header|signature verification failed|invalid remote resource|UNIQUE constraint failed' <<<"${logs}" >&2
    return 1
  fi
}

dump_debug_logs() {
  kubectl -n "${NAMESPACE}" get pods,svc,pvc,mastodonserver >&2 || true
  kubectl -n "${NAMESPACE}" logs deployment/server --all-containers --timestamps >&2 || true
  kubectl -n "${NAMESPACE}" logs deployment/delivery-worker --all-containers --timestamps >&2 || true
  kubectl -n "${NAMESPACE}" logs \
    -l app.kubernetes.io/part-of=mastodon \
    --all-containers \
    --max-log-requests=12 \
    --timestamps >&2 || true
}

cmd_test() {
  check_deps
  need curl
  need jq

  local client_id user_token remote_account_id
  local local_cookie_jar post_content csrf_token local_username

  trap 'status=$?; stop_port_forwards; if [ "$status" -ne 0 ]; then dump_debug_logs; fi; kind delete cluster --name "${CLUSTER_NAME}" >/dev/null 2>&1 || true; exit "$status"' EXIT

  cmd_up
  start_port_forwards
  wait_for_http "kodamapub.e2e" "8443" "https://kodamapub.e2e:8443/health" "kodamapub edge"
  wait_for_mastodon_instance

  local_username="alice-$(openssl rand -hex 8)"
  create_test_local_actor "${local_username}"
  local_cookie_jar="$(mktemp)"
  post_content="Mastodon E2E kodamapub post"
  csrf_token="$(login_local_actor "${local_cookie_jar}" "${local_username}")"

  client_id="$(create_mastodon_app)"
  create_mastodon_user >/dev/null
  user_token="$(create_mastodon_user_token "${client_id}")"

  if [[ -z "${user_token}" || "${user_token}" == "null" ]]; then
    printf 'failed to obtain Mastodon user token\n' >&2
    return 1
  fi

  remote_account_id="$(lookup_remote_account_id "${local_username}")"
  if [[ -z "${remote_account_id}" || "${remote_account_id}" == "null" ]]; then
    printf 'failed to resolve remote account id for %s@kodamapub.e2e\n' "${local_username}" >&2
    return 1
  fi

  for _ in 1 2; do
    follow_remote_account "${user_token}" "${remote_account_id}"
    wait_for_follow_state true "${remote_account_id}" "${user_token}"

    if [[ -z "${posted_once:-}" ]]; then
      create_local_post "${local_cookie_jar}" "${csrf_token}" "${post_content}" "${local_username}"
      wait_for_remote_post_content "${user_token}" "${remote_account_id}" "${post_content}"
      posted_once=true
    fi

    unfollow_remote_account "${user_token}" "${remote_account_id}"
    wait_for_follow_state false "${remote_account_id}" "${user_token}"
  done

  run_repeated_post_delivery_case "${user_token}"

  assert_no_server_errors
}

cmd_up() {
  check_deps
  create_cluster
  generate_certs
  build_and_load_images
  generate_mastodon_env_file
  configure_coredns
  create_namespace_and_secrets
  apply_kodamapub_and_mastodon_dependencies
  install_magout
  restart_cert_consumers
  wait_for_rollouts
}

cmd_seed() {
  check_deps
  seed_local_actor
  seed_mastodon_user
  printf 'kodamapub user: alice\n'
  printf 'kodamapub password: password\n'
}

cmd_port_forward() {
  check_deps
  printf 'kodamapub UI: https://kodamapub.e2e:8443\n'
  printf 'Mastodon UI:  https://mastodon.e2e:3001\n'
  printf 'Leave this process running while using the UIs.\n'
  kubectl -n "${NAMESPACE}" port-forward service/edge 8443:443 &
  kubectl -n "${NAMESPACE}" port-forward service/mastodon-proxy 3001:3001 &
  wait
}

cmd_status() {
  check_deps
  kubectl -n "${NAMESPACE}" get pods,svc,pvc,mastodonserver
}

cmd_logs() {
  check_deps
  kubectl -n "${NAMESPACE}" logs -f deployment/server --all-containers &
  kubectl -n "${NAMESPACE}" logs -f deployment/delivery-worker --all-containers &
  kubectl -n "${NAMESPACE}" logs -f \
    -l app.kubernetes.io/part-of=mastodon \
    --all-containers \
    --max-log-requests=12 &
  wait
}

cmd_down() {
  check_deps
  kind delete cluster --name "${CLUSTER_NAME}"
}

case "${1:-}" in
  up) cmd_up ;;
  seed) cmd_seed ;;
  test) cmd_test ;;
  port-forward) cmd_port_forward ;;
  status) cmd_status ;;
  logs) cmd_logs ;;
  down) cmd_down ;;
  -h|--help|help|"") usage ;;
  *)
    usage >&2
    exit 1
    ;;
esac
