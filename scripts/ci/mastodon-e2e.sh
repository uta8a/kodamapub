#!/usr/bin/env bash
set -euo pipefail

compose() {
  docker compose -f compose.yaml -f compose.e2e.yaml "$@"
}

wait_for_http() {
  local url="$1"
  local label="$2"
  local attempt

  for attempt in $(seq 1 60); do
    if curl -kfsS "$url" >/dev/null; then
      printf 'ready: %s\n' "$label"
      return 0
    fi

    sleep 5
  done

  printf 'timed out waiting for %s (%s)\n' "$label" "$url" >&2
  return 1
}

wait_for_follow_state() {
  local expected="$1"
  local remote_id="$2"
  local token="$3"
  local attempt

  for attempt in $(seq 1 40); do
    local relationship
    if ! relationship="$(curl -fsS \
      -H "Authorization: Bearer ${token}" \
      --get \
      --data-urlencode "id[]=${remote_id}" \
      http://127.0.0.1/api/v1/accounts/relationships)"; then
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
  compose run --rm --no-deps cli-job \
    create-local-actor \
    --public-base-url http://edge \
    --username alice \
    --display-name Alice \
    --summary "GitHub Actions E2E actor" \
    --password password >/dev/null
}

create_mastodon_app() {
  curl -fsS -X POST \
    -H 'Content-Type: application/json' \
    -d '{
      "client_name": "kodamapub-e2e",
      "redirect_uris": ["urn:ietf:wg:oauth:2.0:oob"],
      "scopes": "read write",
      "website": "https://example.invalid"
    }' \
    http://127.0.0.1/api/v1/apps
}

create_mastodon_user() {
  local create_output password

  create_output="$(
    compose exec -T mastodon-web bash -lc \
      'RAILS_ENV=production bin/tootctl accounts create e2e --email e2e@example.com --confirmed --role Owner'
  )"
  password="$(sed -n 's/^New password: //p' <<<"${create_output}" | tail -n 1)"

  if [[ -z "${password}" ]]; then
    printf 'failed to parse Mastodon password from tootctl output\n' >&2
    printf '%s\n' "${create_output}" >&2
    return 1
  fi

  printf '%s\n' "${password}"
}

request_user_token() {
  local client_id="$1"
  local client_secret="$2"
  local password="$3"

  curl -fsS -X POST \
    --data-urlencode "grant_type=password" \
    --data-urlencode "client_id=${client_id}" \
    --data-urlencode "client_secret=${client_secret}" \
    --data-urlencode "username=e2e" \
    --data-urlencode "password=${password}" \
    --data-urlencode "scope=read write" \
    http://127.0.0.1/oauth/token
}

lookup_remote_account_id() {
  local token="$1"

  curl -fsS \
    -H "Authorization: Bearer ${token}" \
    --get \
    --data-urlencode "q=alice@edge" \
    --data-urlencode "resolve=true" \
    --data-urlencode "limit=1" \
    http://127.0.0.1/api/v1/accounts/search | jq -r '.[0].id'
}

follow_remote_account() {
  local token="$1"
  local remote_id="$2"

  curl -fsS -X POST \
    -H "Authorization: Bearer ${token}" \
    http://127.0.0.1/api/v1/accounts/"${remote_id}"/follow >/dev/null
}

unfollow_remote_account() {
  local token="$1"
  local remote_id="$2"

  curl -fsS -X POST \
    -H "Authorization: Bearer ${token}" \
    http://127.0.0.1/api/v1/accounts/"${remote_id}"/unfollow >/dev/null
}

run_delivery_jobs() {
  compose run --rm --no-deps cli-job run-deliveries --limit 100 >/dev/null
}

assert_no_server_errors() {
  local logs
  logs="$(compose logs --no-color --timestamps server || true)"

  if grep -Eq 'request failed|missing signature header|signature verification failed|invalid remote resource|UNIQUE constraint failed' <<<"${logs}"; then
    printf 'server logs contained an unexpected error:\n' >&2
    grep -En 'request failed|missing signature header|signature verification failed|invalid remote resource|UNIQUE constraint failed' <<<"${logs}" >&2
    return 1
  fi
}

main() {
  local app_json client_id client_secret password user_token remote_account_id

  : "${PUBLIC_BASE_URL:=http://edge}"
  export PUBLIC_BASE_URL
  generate_mastodon_secrets

  trap 'status=$?; if [ "$status" -ne 0 ]; then compose logs --no-color --timestamps server mastodon-web mastodon-sidekiq mastodon-db mastodon-redis >&2 || true; fi; compose down -v --remove-orphans >/dev/null 2>&1 || true; exit "$status"' EXIT

  compose up -d --build mastodon-db mastodon-redis edge server web >/dev/null
  compose run --rm --no-deps mastodon-web bundle exec rails db:migrate >/dev/null
  compose up -d --build mastodon-web mastodon-sidekiq >/dev/null

  wait_for_http https://127.0.0.1/api/health "kodamapub server via edge"
  wait_for_http http://127.0.0.1:3001/api/v1/instance "mastodon instance"

  create_local_actor

  app_json="$(create_mastodon_app)"
  client_id="$(jq -r '.client_id' <<<"${app_json}")"
  client_secret="$(jq -r '.client_secret' <<<"${app_json}")"

  password="$(create_mastodon_user)"
  user_token="$(request_user_token "${client_id}" "${client_secret}" "${password}" | jq -r '.access_token')"

  if [[ -z "${user_token}" || "${user_token}" == "null" ]]; then
    printf 'failed to obtain Mastodon user token\n' >&2
    return 1
  fi

  remote_account_id="$(lookup_remote_account_id "${user_token}")"
  if [[ -z "${remote_account_id}" || "${remote_account_id}" == "null" ]]; then
    printf 'failed to resolve remote account id for alice@edge\n' >&2
    return 1
  fi

  for _ in 1 2; do
    follow_remote_account "${user_token}" "${remote_account_id}"
    run_delivery_jobs
    wait_for_follow_state true "${remote_account_id}" "${user_token}"

    unfollow_remote_account "${user_token}" "${remote_account_id}"
    wait_for_follow_state false "${remote_account_id}" "${user_token}"
  done

  assert_no_server_errors
}

main "$@"
