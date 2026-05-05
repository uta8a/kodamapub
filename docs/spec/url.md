# URL仕様

この文書は、2026-05-05 時点の `kodamapub` 実装に基づく URL 仕様です。

URL は次の 3 層に分けて扱います。

- `frontend`: ブラウザで人間が見る URL
- `server`: ActivityPub と WebFinger の公開 URL
- `backend api`: frontend と Hono proxy が内部的に利用する JSON / ActivityPub API

## Frontend

frontend URL は React SPA が解釈します。

```txt
/                       /home へ redirect
/home                   logged-in user home timeline
/@:username             profile and user posts
/@:username/:postId     post detail
```

補足:

- `/home` は現状 `VITE_DEFAULT_USERNAME` の local actor を home timeline として表示します
- `/@:username` は local actor の profile と投稿一覧を表示します
- `/@:username/:postId` はその actor 文脈での単一投稿画面です

## Server

server URL は外部公開される ActivityPub / WebFinger 用 URL です。
実際の backend 実装は `/api` 配下にありますが、公開 URL は Hono が proxy して維持します。

```txt
/.well-known/webfinger  WebFinger
/users/:username        ActivityPub actor JSON
/users/:username/outbox ActivityPub outbox
/users/:username/inbox  ActivityPub inbox
/posts/:postId          ActivityPub Note object
```

補足:

- `/.well-known/webfinger` は Hono から `/api/.well-known/webfinger` へ proxy されます
- `/users/:username` は Hono から `/api/users/:username` へ proxy されます
- `/users/:username/outbox` は Hono から `/api/users/:username/outbox` へ proxy されます
- `/users/:username/inbox` は Hono から `/api/users/:username/inbox` へ proxy されます
- `/posts/:postId` は ActivityPub 用 `Accept` header のときだけ Hono から `/api/posts/:postId` へ proxy されます

## Backend API

backend API は Rust server が直接提供する内部向け endpoint です。

```txt
/api/health
/api/.well-known/webfinger
/api/users/:username
/api/users/:username/posts
/api/users/:username/outbox
/api/users/:username/inbox
/api/posts/:postId
```

content negotiation は次の通りです。

- `GET /api/users/:username`
  - `Accept: application/activity+json` なら Actor JSON
  - それ以外は frontend 向け JSON
- `GET /api/posts/:postId`
  - `Accept: application/activity+json` なら Note object
  - それ以外は frontend 向け JSON

## 投稿 URL

`crates/domain/src/lib.rs` の `Post::new(new_post, public_base_url)` で、投稿 URL は次の形で生成されます。

```txt
{public_base_url}/posts/{post_id}
```

具体例:

```txt
https://example.invalid/posts/01974f87-2f40-7d11-a5aa-8a9d6d9d9a3b
```

補足:

- `post_id` は `Uuid::now_v7()` で生成されます
- `public_base_url` の末尾 `/` は除去されます
- そのため `https://example.invalid` と `https://example.invalid/` は同じ結果になります

## Actor / Outbox URL

local actor の canonical URL は次の形を前提にしています。

```txt
actor_url   = {public_base_url}/users/{username}
outbox_url  = {public_base_url}/users/{username}/outbox
inbox_url   = {public_base_url}/users/{username}/inbox
```

現状のフェーズ2実装で実際に公開しているのは次です。

- `actor_url`
- `outbox_url`

`inbox_url` は field として存在し、`/users/:username/inbox` で受信します。

## WebFinger

WebFinger は次の resource を受け付けます。

```txt
acct:{username}@{host}
```

例:

```txt
acct:alice@example.invalid
```

成功時は actor URL を `self` link として返します。

## 現時点で未実装の URL

以下は型や計画にはあるものの、まだ route としては実装されていません。

- `/users/:username/followers`
- `/users/:username/following`
- media attachment URL
- remote actor / remote object 用 URL

## 設計上のルール

- browser が見る URL と ActivityPub 公開 URL は分ける
- external に見せる ActivityPub URL は `/users/...` と `/posts/...` を使う
- Rust server の内部実装は `/api/...` に置く
- Hono が公開 URL と `/api/...` の橋渡しを担う
