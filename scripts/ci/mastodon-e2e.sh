#!/usr/bin/env bash
set -euo pipefail

compose() {
  docker compose -f compose.e2e.yaml "$@"
}

readonly E2E_CA_CERT="tmp/certs/kodamapub-e2e-ca.pem"

curl_request() {
  local label="$1"
  local resolve_host="$2"
  local resolve_port="$3"
  shift 3

  printf 'curl[%s]: %s\n' "$label" "$*" >&2
  curl --cacert "${E2E_CA_CERT}" \
    --resolve "${resolve_host}:${resolve_port}:127.0.0.1" \
    -fsS --retry 5 --retry-all-errors --retry-connrefused --retry-delay 1 "$@"
}

ensure_edge_certs() {
  local cert_dir="tmp/certs"
  local ca_cert_path="${cert_dir}/kodamapub-e2e-ca.pem"
  local ca_key_path="${cert_dir}/kodamapub-e2e-ca-key.pem"
  local cert_path="${cert_dir}/app.localhost.pem"
  local key_path="${cert_dir}/app.localhost-key.pem"

  mkdir -p "${cert_dir}"

  openssl req -x509 -newkey rsa:2048 -sha256 -nodes -days 3650 \
    -keyout "${ca_key_path}" \
    -out "${ca_cert_path}" \
    -subj "/CN=kodamapub-e2e-ca"

  openssl req -newkey rsa:2048 -sha256 -nodes \
    -keyout "${key_path}" \
    -out "${cert_dir}/app.localhost.csr" \
    -subj "/CN=edge"

  openssl x509 -req \
    -in "${cert_dir}/app.localhost.csr" \
    -CA "${ca_cert_path}" \
    -CAkey "${ca_key_path}" \
    -CAcreateserial \
    -out "${cert_path}" \
    -days 3650 \
    -sha256 \
    -extfile <(printf '%s\n' \
      'subjectAltName=DNS:edge,DNS:localhost,DNS:mastodon.e2e,IP:127.0.0.1' \
      'basicConstraints=CA:FALSE' \
      'keyUsage=digitalSignature,keyEncipherment' \
      'extendedKeyUsage=serverAuth')

  rm -f "${cert_dir}/app.localhost.csr" "${ca_cert_path}.srl"
}

wait_for_http() {
  local host="$1"
  local port="$2"
  local url="$3"
  local label="$4"
  local attempt

  for attempt in $(seq 1 60); do
    if curl_request "$label" "$host" "$port" -fsS "$url" >/dev/null; then
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
    if curl_request "mastodon instance" "mastodon.e2e" "3001" -fsS \
      https://mastodon.e2e:3001/api/v1/instance >/dev/null; then
      printf 'ready: %s\n' "mastodon instance"
      return 0
    fi

    sleep 5
  done

  printf 'timed out waiting for mastodon instance (internal /api/v1/instance)\n' >&2
  return 1
}

wait_for_mastodon_db() {
  local attempt

  for attempt in $(seq 1 60); do
    if compose exec -T mastodon-db pg_isready -U mastodon -d mastodon >/dev/null 2>&1; then
      return 0
    fi

    sleep 2
  done

  printf 'timed out waiting for mastodon db\n' >&2
  return 1
}

wait_for_mastodon_redis() {
  local attempt

  for attempt in $(seq 1 60); do
    if compose exec -T mastodon-redis redis-cli ping >/dev/null 2>&1; then
      return 0
    fi

    sleep 2
  done

  printf 'timed out waiting for mastodon redis\n' >&2
  return 1
}

wait_for_follow_state() {
  local expected="$1"
  local remote_id="$2"
  local token="$3"
  local attempt

  for attempt in $(seq 1 40); do
    local relationship
    if ! relationship="$(curl_request "mastodon relationships" \
      "mastodon.e2e" "3001" \
      -H "Authorization: Bearer ${token}" \
      --get \
      --data-urlencode "id[]=${remote_id}" \
      https://mastodon.e2e:3001/api/v1/accounts/relationships)"; then
      sleep 3
      continue
    fi

    local value
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
  local attempt

  for attempt in $(seq 1 40); do
    local statuses
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

generate_mastodon_secrets() {
  local image="${MASTODON_IMAGE_TAG:-v4.5.9}"
  local mastodon_image="ghcr.io/mastodon/mastodon:${image}"
  local secret_key_base otp_secret encryption_output vapid_output

  secret_key_base="$(docker run --rm "${mastodon_image}" bin/rails secret)"
  otp_secret="$(docker run --rm "${mastodon_image}" bin/rails secret)"
  encryption_output="$(
    docker run --rm "${mastodon_image}" bundle exec rails db:encryption:init
  )"
  vapid_output="$(
    docker run --rm \
      -e SECRET_KEY_BASE="${secret_key_base}" \
      -e OTP_SECRET="${otp_secret}" \
      "${mastodon_image}" \
      bin/rake mastodon:webpush:generate_vapid_key
  )"

  export MASTODON_IMAGE_TAG="${image}"
  export MASTODON_SECRET_KEY_BASE="${secret_key_base}"
  export MASTODON_OTP_SECRET="${otp_secret}"
  export MASTODON_ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY="$(sed -n 's/^ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=//p' <<<"${encryption_output}")"
  export MASTODON_ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY="$(sed -n 's/^ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY=//p' <<<"${encryption_output}")"
  export MASTODON_ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT="$(sed -n 's/^ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT=//p' <<<"${encryption_output}")"
  export MASTODON_VAPID_PRIVATE_KEY="$(sed -n 's/^VAPID_PRIVATE_KEY=//p' <<<"${vapid_output}")"
  export MASTODON_VAPID_PUBLIC_KEY="$(sed -n 's/^VAPID_PUBLIC_KEY=//p' <<<"${vapid_output}")"
}

create_local_actor() {
  local username="$1"

  compose run --rm --no-deps cli-job \
    create-local-actor \
    --public-base-url https://edge \
    --username "${username}" \
    --display-name Alice \
    --summary "GitHub Actions E2E actor" \
    --password password >/dev/null
}

login_local_actor() {
  local cookie_jar="$1"
  local username="$2"
  local login_output

  login_output="$(
    curl_request "login local actor" "edge" "443" \
      -c "${cookie_jar}" \
      -H 'Origin: https://edge' \
      -H 'Content-Type: application/json' \
      -d '{
        "username": "'"${username}"'",
        "password": "password"
      }' \
      https://edge/api/login
  )"

  jq -r '.csrf_token' <<<"${login_output}"
}

create_local_post() {
  local cookie_jar="$1"
  local csrf_token="$2"
  local content_source="$3"
  local username="$4"

  curl_request "create local post" "edge" "443" -fsS -X POST \
    -b "${cookie_jar}" \
    -H 'Origin: https://edge' \
    -H "x-csrf-token: ${csrf_token}" \
    -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg content_source "${content_source}" '{
      content_source: $content_source,
      content_format: "Plaintext",
      visibility: "Public",
      in_reply_to: null
    }')" \
    "https://edge/api/users/${username}/posts" >/dev/null
}

create_mastodon_app() {
  compose exec -T mastodon-web bash -lc \
    'RAILS_ENV=production bundle exec rails runner '\''app = Doorkeeper::Application.find_or_initialize_by(name: "kodamapub-e2e"); app.redirect_uri = "urn:ietf:wg:oauth:2.0:oob"; app.scopes = "read:accounts read:follows read:statuses write:accounts write:follows"; app.website = "https://example.invalid"; app.save!; puts app.uid'\'''
}

create_mastodon_user() {
  local create_output modify_output password

  modify_output="$(
    compose exec -T mastodon-web bash -lc \
      'RAILS_ENV=production bin/tootctl accounts modify e2e --approve --reset-password' \
      2>/dev/null || true
  )"
  password="$(sed -n 's/^New password: //p' <<<"${modify_output}" | tail -n 1)"

  if [[ -z "${password}" ]]; then
    create_output="$(
      compose exec -T mastodon-web bash -lc \
        'RAILS_ENV=production bin/tootctl accounts create e2e --email e2e@uta8a.org --confirmed --force --approve'
    )"
    password="$(sed -n 's/^New password: //p' <<<"${create_output}" | tail -n 1)"
  fi

  if [[ -z "${password}" ]]; then
    printf 'failed to obtain Mastodon password from tootctl output\n' >&2
    if [[ -n "${modify_output:-}" ]]; then
      printf '%s\n' "${modify_output}" >&2
    fi
    if [[ -n "${create_output:-}" ]]; then
      printf '%s\n' "${create_output}" >&2
    fi
    return 1
  fi

  printf '%s\n' "${password}"
}

create_mastodon_user_token() {
  local client_id="$1"

  compose exec -T \
    -e CLIENT_ID="${client_id}" \
    -e USER_EMAIL="e2e@uta8a.org" \
    mastodon-web \
    bash -lc \
    'RAILS_ENV=production bundle exec rails runner '\''app = Doorkeeper::Application.find_by!(uid: ENV.fetch("CLIENT_ID")); user = User.where(email: ENV.fetch("USER_EMAIL")).order(created_at: :desc).first!; token = Doorkeeper::AccessToken.create_for(application: app, resource_owner: user, scopes: "read:accounts read:follows read:statuses write:accounts write:follows"); puts token.token'\'''
}

lookup_remote_account_id() {
  local username="$1"

  compose exec -T mastodon-web bash -lc \
    'RAILS_ENV=production bundle exec rails runner '\''account = ActivityPub::FetchRemoteAccountService.new.call("https://edge/users/'"${username}"'"); puts account.id'\'''
}

follow_remote_account() {
  local token="$1"
  local remote_id="$2"

  curl_request "mastodon follow" "mastodon.e2e" "3001" -X POST \
    -H "Authorization: Bearer ${token}" \
    https://mastodon.e2e:3001/api/v1/accounts/"${remote_id}"/follow >/dev/null
}

unfollow_remote_account() {
  local token="$1"
  local remote_id="$2"

  curl_request "mastodon unfollow" "mastodon.e2e" "3001" -X POST \
    -H "Authorization: Bearer ${token}" \
    https://mastodon.e2e:3001/api/v1/accounts/"${remote_id}"/unfollow >/dev/null
}

assert_no_server_errors() {
  local logs
  logs="$(compose logs --no-color --timestamps server delivery-worker || true)"

  if grep -Eq 'request failed|missing signature header|signature verification failed|invalid remote resource|UNIQUE constraint failed' <<<"${logs}"; then
    printf 'server logs contained an unexpected error:\n' >&2
    grep -En 'request failed|missing signature header|signature verification failed|invalid remote resource|UNIQUE constraint failed' <<<"${logs}" >&2
    return 1
  fi
}

main() {
  local client_id user_token remote_account_id
  local local_cookie_jar post_content csrf_token local_username

  ensure_edge_certs
  generate_mastodon_secrets

  local_username="alice-$(openssl rand -hex 8)"

  trap 'status=$?; if [ "$status" -ne 0 ]; then compose logs --no-color --timestamps server mastodon-web mastodon-sidekiq mastodon-db mastodon-redis >&2 || true; fi; compose down -v --remove-orphans >/dev/null 2>&1 || true; exit "$status"' EXIT

  compose up -d mastodon-db mastodon-redis edge server delivery-worker >/dev/null
  wait_for_mastodon_db
  wait_for_mastodon_redis
  compose run --rm --no-deps -e RAILS_ENV=production mastodon-web bundle exec rails db:prepare >/dev/null
  compose up -d mastodon-web mastodon-sidekiq mastodon-proxy >/dev/null

  wait_for_http "edge" "443" "https://edge/health" "kodamapub edge"
  wait_for_mastodon_instance

  create_local_actor "${local_username}"
  local_cookie_jar="$(mktemp)"
  post_content="Mastodon E2E kodamapub post"
  csrf_token="$(login_local_actor "${local_cookie_jar}" "${local_username}")"

  client_id="$(create_mastodon_app)"

  create_mastodon_user
  user_token="$(create_mastodon_user_token "${client_id}")"

  if [[ -z "${user_token}" || "${user_token}" == "null" ]]; then
    printf 'failed to obtain Mastodon user token\n' >&2
    return 1
  fi

  remote_account_id="$(lookup_remote_account_id "${local_username}")"
  if [[ -z "${remote_account_id}" || "${remote_account_id}" == "null" ]]; then
    printf 'failed to resolve remote account id for %s@edge\n' "${local_username}" >&2
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

  assert_no_server_errors
}

main "$@"
