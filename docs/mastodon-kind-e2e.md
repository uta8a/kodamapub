# Mastodon kind E2E

This local environment runs kodamapub and Mastodon in a kind cluster. Mastodon is created by the Magout operator from <https://github.com/ushitora-anqou/magout>.

## Prerequisites

- Docker is running.
- `mise install` has installed `kind`, `kubectl`, and `helm`.
- Add hostnames for browser access while `kubectl port-forward` is running:

```text
127.0.0.1 kodamapub.e2e mastodon.e2e
```

## Start

```sh
mise install
mise run kind-mastodon-up
mise run kind-mastodon-seed
mise run kind-mastodon-port-forward
```

Keep the port-forward process running, then open:

- kodamapub: `https://kodamapub.e2e:8443`
- Mastodon: `https://mastodon.e2e:3001`

The certificate is generated under `tmp/kind-certs`. Browsers will warn because the local CA is not trusted by the OS.

## Automated E2E

```sh
mise run kind-mastodon-test
```

The automated flow creates a fresh kind cluster, deploys kodamapub and Magout-managed Mastodon, creates a temporary kodamapub actor, follows it from Mastodon, verifies a public post is visible in Mastodon, verifies unfollow, and deletes the cluster on exit.

## Test Accounts

- kodamapub: `alice` / `password`
- Mastodon: `e2e@uta8a.org` / `password`

## Manual UI Check

### Mastodon to kodamapub

1. Open Mastodon and sign in as `e2e`.
2. Search for `https://kodamapub.e2e/users/alice` or `@alice@kodamapub.e2e`.
3. Follow Alice from Mastodon.
4. Open kodamapub, sign in as `alice`, and create a public post.
5. Confirm the post appears in Mastodon.

### kodamapub to Mastodon

1. Open kodamapub and sign in as `alice`.
2. In the Follow panel, enter `acct:e2e@mastodon.e2e:3001`.
3. Click Follow.
4. Confirm Mastodon shows `alice@kodamapub.e2e` as a follower of `e2e`.
5. Click Unfollow and confirm that follower relationship disappears in Mastodon.

## Operations

```sh
mise run kind-mastodon-status
mise run kind-mastodon-logs
mise run kind-mastodon-down
```

## Notes

- The kind CoreDNS config rewrites `kodamapub.e2e` and `mastodon.e2e` to in-cluster services so ActivityPub delivery stays inside the cluster.
- kodamapub's ActivityPub public URL is `https://kodamapub.e2e`; the host browser still uses port-forwarded `https://kodamapub.e2e:8443` to avoid binding local port 443.
- `mastodon.e2e:3001` is terminated by a small nginx proxy, then forwarded to Mastodon's web service.
- `kodamapub.e2e:8443` is served by `kodamapub-edge` with the same local CA.
